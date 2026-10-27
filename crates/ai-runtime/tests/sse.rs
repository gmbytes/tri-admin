use bytes::Bytes;
use futures_util::{TryStreamExt, stream};
use tri_ai_runtime::{Error, Limits, frames};
fn chunks(
    parts: Vec<Vec<u8>>,
) -> impl futures_util::Stream<Item = Result<Bytes, std::io::Error>> + Send + 'static {
    stream::iter(parts.into_iter().map(|part| Ok(Bytes::from(part))))
}

#[tokio::test]
async fn unicode_bom_and_crlf_survive_every_possible_fragment_boundary() {
    let wire=b"\xef\xbb\xbfevent: response.output_text.delta\r\ndata: {\"delta\":\"\xe4\xbd\xa0\xe5\xa5\xbd\"}\r\n\r\n";
    for split in 0..=wire.len() {
        let events = frames(
            chunks(vec![wire[..split].to_vec(), wire[split..].to_vec()]),
            Limits::default(),
        )
        .unwrap()
        .try_collect::<Vec<_>>()
        .await
        .unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0].event.as_deref(),
            Some("response.output_text.delta")
        );
        assert_eq!(events[0].data.as_deref(), Some("{\"delta\":\"你好\"}"));
    }
}
#[tokio::test]
async fn multiline_data_and_comment_heartbeats_keep_sse_semantics() {
    let events = frames(
        chunks(vec![
            b"event: response.created\nevent: duplicate\ndata: {}\n\n".to_vec(),
        ]),
        Limits::default(),
    )
    .unwrap()
    .try_collect::<Vec<_>>()
    .await;
    assert!(events.is_err());
    let events = frames(
        chunks(vec![
            b": ping\n\nevent: response.created\ndata: {\ndata: \"type\":\"response.created\"}\n\n"
                .to_vec(),
        ]),
        Limits::default(),
    )
    .unwrap()
    .try_collect::<Vec<_>>()
    .await
    .unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].data.as_deref(),
        Some("{\n\"type\":\"response.created\"}")
    );
}
#[tokio::test]
async fn event_and_stream_limits_apply_before_the_parser_buffers_an_unterminated_line() {
    let events = frames(
        chunks(vec![vec![b'x'; 65537]]),
        Limits {
            max_event_bytes: 65536,
            ..Limits::default()
        },
    )
    .unwrap()
    .try_collect::<Vec<_>>()
    .await;
    assert!(matches!(events, Err(Error::Limit)));
    let events = frames(
        chunks(vec![b"data: a\n\ndata: b\n\n".to_vec()]),
        Limits {
            max_stream_bytes: 10,
            ..Limits::default()
        },
    )
    .unwrap()
    .try_collect::<Vec<_>>()
    .await;
    assert!(matches!(events, Err(Error::Limit)));
    let events = frames(
        chunks(vec![b"\n\n\n\n".to_vec()]),
        Limits {
            max_events: 2,
            ..Limits::default()
        },
    )
    .unwrap()
    .try_collect::<Vec<_>>()
    .await;
    assert!(matches!(events, Err(Error::Limit)));
}
#[tokio::test]
async fn invalid_utf8_and_transport_errors_are_static_and_unterminated_events_are_not_dispatched() {
    let events = frames(chunks(vec![b"data: \xff\n\n".to_vec()]), Limits::default())
        .unwrap()
        .try_collect::<Vec<_>>()
        .await;
    assert!(matches!(events, Err(Error::Protocol)));
    let source = stream::iter(vec![Err::<Bytes, _>(std::io::Error::other(
        "PRIVATE-UPSTREAM-MARKER",
    ))]);
    let error = frames(source, Limits::default())
        .unwrap()
        .try_collect::<Vec<_>>()
        .await
        .err()
        .unwrap();
    assert!(!format!("{error:?} {error}").contains("PRIVATE-UPSTREAM-MARKER"));
    let events = frames(
        chunks(vec![b"data: {\"type\":\"response.completed\"}".to_vec()]),
        Limits::default(),
    )
    .unwrap()
    .try_collect::<Vec<_>>()
    .await
    .unwrap();
    assert!(events.is_empty());
}
