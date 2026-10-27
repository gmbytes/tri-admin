use crate::{Error, Limits};
use bytes::Bytes;
use futures_util::{Stream, StreamExt, stream::BoxStream};
use sse_stream::SseStream;

pub struct Frame {
    pub event: Option<String>,
    pub data: Option<String>,
    pub id: Option<String>,
}
impl std::fmt::Debug for Frame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Frame")
            .field("event_present", &self.event.is_some())
            .field("data_bytes", &self.data.as_ref().map(String::len))
            .finish()
    }
}
struct Budget {
    limits: Limits,
    total: usize,
    event: usize,
    line: usize,
    frames: u32,
    cr: bool,
    reset_cr: bool,
}
impl Budget {
    fn consume(&mut self, bytes: &[u8]) -> Result<(), Error> {
        self.total = self
            .total
            .checked_add(bytes.len())
            .filter(|size| *size <= self.limits.max_stream_bytes)
            .ok_or(Error::Limit)?;
        for byte in bytes {
            if self.cr && *byte == b'\n' {
                self.cr = false;
                if self.reset_cr {
                    self.event = 0;
                } else {
                    self.event = self
                        .event
                        .checked_add(1)
                        .filter(|size| *size <= self.limits.max_event_bytes)
                        .ok_or(Error::Limit)?;
                }
                continue;
            }
            self.cr = false;
            self.reset_cr = false;
            self.event = self
                .event
                .checked_add(1)
                .filter(|size| *size <= self.limits.max_event_bytes)
                .ok_or(Error::Limit)?;
            if matches!(*byte, b'\r' | b'\n') {
                let blank = self.line == 0;
                self.line = 0;
                if blank {
                    self.frames = self
                        .frames
                        .checked_add(1)
                        .filter(|count| *count <= self.limits.max_events)
                        .ok_or(Error::Limit)?;
                    self.event = 0;
                }
                if *byte == b'\r' {
                    self.cr = true;
                    self.reset_cr = blank;
                }
            } else {
                self.line += 1;
            }
        }
        Ok(())
    }
}
pub fn frames<S, E>(
    source: S,
    limits: Limits,
) -> Result<BoxStream<'static, Result<Frame, Error>>, Error>
where
    S: Stream<Item = Result<Bytes, E>> + Send + 'static,
    E: std::error::Error + Send + Sync + 'static,
{
    let limits = limits.validate()?;
    let budget = Budget {
        limits,
        total: 0,
        event: 0,
        line: 0,
        frames: 0,
        cr: false,
        reset_cr: false,
    };
    let bytes = source.scan((budget, false), |state, item| {
        if state.1 {
            return std::future::ready(None);
        }
        let result = item
            .map_err(|error| {
                (&error as &dyn std::error::Error)
                    .downcast_ref::<Error>()
                    .copied()
                    .unwrap_or(Error::Transport)
            })
            .and_then(|bytes| {
                state.0.consume(&bytes)?;
                Ok(bytes)
            });
        if result.is_err() {
            state.1 = true;
        }
        std::future::ready(Some(result))
    });
    Ok(SseStream::from_bytes_stream(bytes)
        .map(|item| match item {
            Ok(event) => Ok(Frame {
                event: event.event,
                data: event.data,
                id: event.id,
            }),
            Err(sse_stream::Error::Body(error)) => Err(error
                .downcast_ref::<Error>()
                .copied()
                .unwrap_or(Error::Transport)),
            Err(_) => Err(Error::Protocol),
        })
        .boxed())
}
