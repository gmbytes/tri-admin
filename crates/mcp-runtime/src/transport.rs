use rmcp::{
    RoleClient,
    model::{ClientJsonRpcMessage, ServerJsonRpcMessage},
    transport::Transport,
};
use std::{
    io,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{ChildStdin, ChildStdout},
    sync::Mutex,
};
use tokio_util::sync::CancellationToken;
pub(crate) struct BoundedTransport {
    reader: BufReader<ChildStdout>,
    partial: Vec<u8>,
    writer: Arc<Mutex<Option<ChildStdin>>>,
    max: usize,
    stop: CancellationToken,
    fault: Arc<AtomicBool>,
}
impl BoundedTransport {
    pub fn new(
        reader: ChildStdout,
        writer: ChildStdin,
        max: usize,
        stop: CancellationToken,
        fault: Arc<AtomicBool>,
    ) -> Self {
        Self {
            reader: BufReader::new(reader),
            partial: Vec::new(),
            writer: Arc::new(Mutex::new(Some(writer))),
            max,
            stop,
            fault,
        }
    }
    fn fail(&self) {
        self.fault.store(true, Ordering::SeqCst);
        self.stop.cancel();
    }
}
impl Drop for BoundedTransport {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}
impl Transport<RoleClient> for BoundedTransport {
    type Error = io::Error;
    fn send(
        &mut self,
        item: ClientJsonRpcMessage,
    ) -> impl Future<Output = io::Result<()>> + Send + 'static {
        let writer = self.writer.clone();
        let max = self.max;
        let stop = self.stop.clone();
        let fault = self.fault.clone();
        async move {
            let mut bytes =
                serde_json::to_vec(&item).map_err(|_| io::Error::other("invalid MCP request"))?;
            if bytes.len() > max {
                fault.store(true, Ordering::SeqCst);
                stop.cancel();
                return Err(io::Error::other("MCP frame limit"));
            }
            bytes.push(b'\n');
            let mut slot = writer.lock().await;
            let writer = slot
                .as_mut()
                .ok_or_else(|| io::Error::other("MCP transport closed"))?;
            writer.write_all(&bytes).await?;
            writer.flush().await
        }
    }
    async fn receive(&mut self) -> Option<ServerJsonRpcMessage> {
        loop {
            let buffer = match self.reader.fill_buf().await {
                Ok(buffer) => buffer,
                Err(_) => {
                    self.fail();
                    return None;
                }
            };
            if buffer.is_empty() {
                self.stop.cancel();
                return None;
            }
            let end = buffer.iter().position(|b| *b == b'\n');
            let count = end.map_or(buffer.len(), |n| n + 1);
            if self.partial.len().saturating_add(count) > self.max + 1 {
                self.fail();
                return None;
            }
            self.partial.extend_from_slice(&buffer[..count]);
            self.reader.consume(count);
            if end.is_none() {
                continue;
            }
            let line = self.partial.strip_suffix(b"\n").unwrap_or(&self.partial);
            let line = line.strip_suffix(b"\r").unwrap_or(line);
            if line.is_empty() {
                self.partial.clear();
                continue;
            }
            let result = serde_json::from_slice(line);
            self.partial.clear();
            match result {
                Ok(message) => return Some(message),
                Err(_) => {
                    self.fail();
                    return None;
                }
            }
        }
    }
    async fn close(&mut self) -> io::Result<()> {
        self.stop.cancel();
        if let Some(mut writer) = self.writer.lock().await.take() {
            writer.shutdown().await?;
        }
        Ok(())
    }
}
