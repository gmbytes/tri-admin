//! Test-only observer for a loopback, uncompressed MongoDB connection. Original
//! frames are forwarded unchanged; only a confirmed completion COMMIT reply is lost.
//! Protocol: https://www.mongodb.com/docs/manual/reference/mongodb-wire-protocol/
use mongodb::bson::{Bson, Document, oid::ObjectId};
use std::{
    collections::HashSet,
    io,
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::Mutex,
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Eq, PartialEq)]
struct Transaction {
    session: Vec<u8>,
    number: i64,
}
struct State {
    database: String,
    job: ObjectId,
    target: Mutex<Option<Transaction>>,
    armed: AtomicBool,
    dropped: AtomicUsize,
    commits: AtomicUsize,
    repeated_body: AtomicUsize,
    observed: CancellationToken,
}
pub struct MongoCommitFault {
    pub uri: String,
    state: Arc<State>,
    server: JoinHandle<()>,
}
impl MongoCommitFault {
    pub async fn start(uri: &str, database: &str, job: ObjectId) -> Self {
        assert!(database.starts_with("tri_admin_test_"));
        let mut proxied =
            url::Url::parse(uri).expect("fixture Mongo URI must be a single loopback host");
        assert_eq!(proxied.scheme(), "mongodb");
        let host: std::net::IpAddr = proxied
            .host_str()
            .unwrap()
            .parse()
            .expect("fixture Mongo host must be an IP");
        assert!(host.is_loopback());
        let upstream = SocketAddr::new(host, proxied.port().unwrap_or(27017));
        let options = proxied
            .query_pairs()
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect::<Vec<_>>();
        assert!(
            !options
                .iter()
                .any(|(key, value)| matches!(key.as_str(), "tls" | "ssl") && value != "false"),
            "wire fault fixture requires local plaintext MongoDB"
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        proxied.set_host(Some("127.0.0.1")).unwrap();
        proxied
            .set_port(Some(listener.local_addr().unwrap().port()))
            .unwrap();
        proxied.set_query(None);
        {
            let mut pairs = proxied.query_pairs_mut();
            for (key, value) in options {
                if !matches!(
                    key.as_str(),
                    "directConnection" | "compressors" | "maxPoolSize" | "socketTimeoutMS"
                ) {
                    pairs.append_pair(&key, &value);
                }
            }
            pairs
                .append_pair("directConnection", "true")
                .append_pair("maxPoolSize", "4");
        }
        assert!(
            mongodb::options::ClientOptions::parse(proxied.as_str())
                .await
                .is_ok(),
            "fault fixture Mongo URI must be accepted by the pinned driver"
        );
        let state = Arc::new(State {
            database: database.into(),
            job,
            target: Mutex::new(None),
            armed: AtomicBool::new(true),
            dropped: AtomicUsize::new(0),
            commits: AtomicUsize::new(0),
            repeated_body: AtomicUsize::new(0),
            observed: CancellationToken::new(),
        });
        let shared = state.clone();
        let server = tokio::spawn(async move {
            let mut connections = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    accepted=listener.accept()=>{
                        let (socket,_)=accepted.unwrap();
                        let shared=shared.clone();
                        connections.spawn(async move { let _=relay(socket,upstream,shared).await; });
                    }
                    result=connections.join_next(),if !connections.is_empty()=>{ result.unwrap().unwrap(); }
                }
            }
        });
        Self {
            uri: proxied.to_string(),
            state,
            server,
        }
    }
    pub async fn wait_for_lost_commit(&self) {
        tokio::time::timeout(
            std::time::Duration::from_secs(20),
            self.state.observed.cancelled(),
        )
        .await
        .expect("completion COMMIT success reply was not intercepted");
    }
    pub fn dropped(&self) -> usize {
        self.state.dropped.load(Ordering::SeqCst)
    }
    pub fn commits(&self) -> usize {
        self.state.commits.load(Ordering::SeqCst)
    }
    pub fn repeated_body(&self) -> usize {
        self.state.repeated_body.load(Ordering::SeqCst)
    }
}
impl Drop for MongoCommitFault {
    fn drop(&mut self) {
        self.server.abort();
    }
}
fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid fixture Mongo frame")
}
fn int32(data: &[u8], offset: usize) -> io::Result<i32> {
    Ok(i32::from_le_bytes(
        data.get(offset..offset + 4)
            .ok_or_else(invalid)?
            .try_into()
            .map_err(|_| invalid())?,
    ))
}
async fn frame(reader: &mut (impl AsyncRead + Unpin)) -> io::Result<Vec<u8>> {
    let mut length = [0; 4];
    reader.read_exact(&mut length).await?;
    let size = i32::from_le_bytes(length);
    if !(16..=64 * 1024 * 1024).contains(&size) {
        return Err(invalid());
    }
    let mut message = vec![0; size as usize];
    message[..4].copy_from_slice(&length);
    reader.read_exact(&mut message[4..]).await?;
    Ok(message)
}
fn document(data: &[u8], position: &mut usize, end: usize) -> io::Result<Document> {
    let size = usize::try_from(int32(data, *position)?).map_err(|_| invalid())?;
    let next = position.checked_add(size).ok_or_else(invalid)?;
    if size < 5 || next > end {
        return Err(invalid());
    }
    let result = Document::from_reader(&mut std::io::Cursor::new(&data[*position..next]))
        .map_err(|_| invalid())?;
    *position = next;
    Ok(result)
}
fn body(message: &[u8]) -> io::Result<Option<Document>> {
    match int32(message, 12)? {
        2012 => return Err(invalid()),
        2013 => (),
        _ => return Ok(None),
    }
    let flags = int32(message, 16)? as u32;
    let end = message.len() - if flags & 1 != 0 { 4 } else { 0 };
    let mut position = 20;
    let mut result = None;
    let mut sequences = Vec::new();
    while position < end {
        let kind = message[position];
        position += 1;
        match kind {
            0 => {
                if result.is_some() {
                    return Err(invalid());
                }
                result = Some(document(message, &mut position, end)?);
            }
            1 => {
                let start = position;
                let size = usize::try_from(int32(message, position)?).map_err(|_| invalid())?;
                let stop = start.checked_add(size).ok_or_else(invalid)?;
                if size < 5 || stop > end {
                    return Err(invalid());
                }
                position += 4;
                let terminator = message[position..stop]
                    .iter()
                    .position(|value| *value == 0)
                    .ok_or_else(invalid)?
                    + position;
                let name = std::str::from_utf8(&message[position..terminator])
                    .map_err(|_| invalid())?
                    .to_owned();
                position = terminator + 1;
                let mut values = Vec::new();
                while position < stop {
                    values.push(Bson::Document(document(message, &mut position, stop)?));
                }
                sequences.push((name, values));
            }
            _ => return Err(invalid()),
        }
    }
    let mut result = result.ok_or_else(invalid)?;
    for (name, values) in sequences {
        result.insert(name, Bson::Array(values));
    }
    Ok(Some(result))
}
fn transaction(command: &Document) -> Option<Transaction> {
    let Bson::Binary(session) = command.get_document("lsid").ok()?.get("id")? else {
        return None;
    };
    Some(Transaction {
        session: session.bytes.clone(),
        number: command.get_i64("txnNumber").ok()?,
    })
}
async fn observe(state: &State, command: &Document) -> bool {
    let completion = command.get_str("$db").ok() == Some(state.database.as_str())
        && command.get_str("update").ok() == Some("data_exports")
        && command.get_array("updates").ok().is_some_and(|updates| {
            updates.iter().any(|value| {
                value.as_document().is_some_and(|update| {
                    update
                        .get_document("q")
                        .ok()
                        .and_then(|query| query.get_object_id("_id").ok())
                        == Some(state.job)
                        && update
                            .get_document("u")
                            .ok()
                            .and_then(|update| update.get_document("$set").ok())
                            .and_then(|update| update.get_str("state").ok())
                            == Some("completed")
                })
            })
        });
    if completion {
        if state.dropped.load(Ordering::SeqCst) > 0 {
            state.repeated_body.fetch_add(1, Ordering::SeqCst);
        } else {
            *state.target.lock().await = transaction(command);
        }
    }
    if command.contains_key("commitTransaction") {
        let target = state.target.lock().await;
        if target.is_some() && *target == transaction(command) {
            state.commits.fetch_add(1, Ordering::SeqCst);
            return state.armed.swap(false, Ordering::SeqCst);
        }
    }
    false
}
async fn relay(socket: TcpStream, upstream: SocketAddr, state: Arc<State>) -> io::Result<()> {
    let remote = TcpStream::connect(upstream).await?;
    let (mut from_client, mut to_client) = socket.into_split();
    let (mut from_server, mut to_server) = remote.into_split();
    let pending = Mutex::new(HashSet::<i32>::new());
    let requests = async {
        loop {
            let message = frame(&mut from_client).await?;
            if let Some(command) = body(&message)?
                && observe(&state, &command).await
            {
                pending.lock().await.insert(int32(&message, 4)?);
            }
            to_server.write_all(&message).await?;
        }
        #[allow(unreachable_code)]
        Ok::<(), io::Error>(())
    };
    let responses = async {
        loop {
            let message = frame(&mut from_server).await?;
            if pending.lock().await.remove(&int32(&message, 8)?) {
                let response = body(&message)?.ok_or_else(invalid)?;
                let success = matches!(response.get("ok"),Some(Bson::Double(value)) if *value==1.0)
                    || matches!(response.get("ok"), Some(Bson::Int32(1) | Bson::Int64(1)));
                if !success {
                    return Err(invalid());
                }
                // The real database has replied success. Drop this response before
                // sending any of its bytes to the application, forcing COMMIT retry.
                state.dropped.fetch_add(1, Ordering::SeqCst);
                state.observed.cancel();
                return Err(io::Error::new(
                    io::ErrorKind::ConnectionReset,
                    "fixture lost COMMIT reply",
                ));
            }
            to_client.write_all(&message).await?;
        }
        #[allow(unreachable_code)]
        Ok::<(), io::Error>(())
    };
    tokio::try_join!(requests, responses)?;
    Ok(())
}
