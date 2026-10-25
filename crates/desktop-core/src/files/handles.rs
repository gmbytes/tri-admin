use super::{FileCancel, FileSpec, NativeSource, NativeWriter, SaveMode, SaveReceipt, SourceInfo};
use crate::{DesktopError, error};
use serde::Deserialize;
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
};

#[derive(Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum FileJobKind {
    Source,
    Download,
}
enum State {
    Reserved(FileJobKind),
    Busy,
    Source(Box<NativeSource>),
    Download(Box<NativeWriter>),
}
struct Job {
    cancel: FileCancel,
    state: Mutex<State>,
}
#[derive(Default)]
pub struct FileHandles {
    jobs: Mutex<HashMap<String, Arc<Job>>>,
}
fn unavailable() -> DesktopError {
    error("FILE_STATE", "文件操作状态无效，请重新选择")
}
impl FileHandles {
    pub fn reserve(&self, kind: FileJobKind) -> Result<String, DesktopError> {
        let mut jobs = self.jobs.lock().map_err(|_| unavailable())?;
        if jobs.len() >= 32 {
            return Err(error(
                "FILE_CAPACITY",
                "最多同时打开 32 个文件操作，请关闭不用的文件",
            ));
        }
        let id = uuid::Uuid::new_v4().to_string();
        jobs.insert(
            id.clone(),
            Arc::new(Job {
                cancel: FileCancel::default(),
                state: Mutex::new(State::Reserved(kind)),
            }),
        );
        Ok(id)
    }
    fn get(&self, id: &str) -> Result<Arc<Job>, DesktopError> {
        self.jobs
            .lock()
            .map_err(|_| unavailable())?
            .get(id)
            .cloned()
            .ok_or_else(|| error("FILE_HANDLE_INVALID", "文件句柄已关闭，请重新选择"))
    }
    fn claim(&self, id: &str, kind: FileJobKind) -> Result<Arc<Job>, DesktopError> {
        let job = self.get(id)?;
        job.cancel.check()?;
        {
            let mut state = job.state.lock().map_err(|_| unavailable())?;
            if !matches!(&*state,State::Reserved(value)if *value==kind) {
                return Err(unavailable());
            }
            *state = State::Busy;
        }
        Ok(job)
    }
    pub fn select_source(
        &self,
        id: &str,
        picker: impl FnOnce() -> Result<Option<PathBuf>, DesktopError>,
        progress: impl FnMut(u64),
    ) -> Result<Option<SourceInfo>, DesktopError> {
        let job = self.claim(id, FileJobKind::Source)?;
        let result = (|| {
            job.cancel.check()?;
            let Some(path) = picker()? else {
                return Ok(None);
            };
            job.cancel.check()?;
            let source = NativeSource::open_with_progress(&path, &job.cancel, progress)?;
            let info = source.info().clone();
            job.cancel.publish(|| {
                let mut state = job.state.lock().map_err(|_| unavailable())?;
                if !matches!(*state, State::Busy) {
                    return Err(unavailable());
                }
                *state = State::Source(Box::new(source));
                Ok(())
            })?;
            Ok(Some(info))
        })();
        if !matches!(result, Ok(Some(_))) {
            self.cancel(id)
        }
        result
    }
    pub fn select_download(
        &self,
        id: &str,
        spec: FileSpec,
        mode: SaveMode,
        picker: impl FnOnce() -> Result<Option<PathBuf>, DesktopError>,
    ) -> Result<bool, DesktopError> {
        spec.validate()?;
        let job = self.claim(id, FileJobKind::Download)?;
        let result = (|| {
            job.cancel.check()?;
            let Some(path) = picker()? else {
                return Ok(false);
            };
            job.cancel.check()?;
            let writer = NativeWriter::prepare(&path, spec, mode, &job.cancel)?;
            job.cancel.publish(|| {
                let mut state = job.state.lock().map_err(|_| unavailable())?;
                if !matches!(*state, State::Busy) {
                    return Err(unavailable());
                }
                *state = State::Download(Box::new(writer));
                Ok(())
            })?;
            Ok(true)
        })();
        if !matches!(result, Ok(true)) {
            self.cancel(id)
        }
        result
    }
    pub fn read_chunk(&self, id: &str, index: u32) -> Result<Vec<u8>, DesktopError> {
        let job = self.get(id)?;
        job.cancel.check()?;
        let mut state = job.state.lock().map_err(|_| unavailable())?;
        match &mut *state {
            State::Source(source) => source.read_chunk(index),
            _ => Err(unavailable()),
        }
    }
    pub fn write(&self, id: &str, bytes: &[u8]) -> Result<(), DesktopError> {
        let job = self.get(id)?;
        job.cancel.check()?;
        let mut state = job.state.lock().map_err(|_| unavailable())?;
        match &mut *state {
            State::Download(writer) => writer.write(bytes),
            _ => Err(unavailable()),
        }
    }
    pub fn complete(&self, id: &str) -> Result<SaveReceipt, DesktopError> {
        let job = self.get(id)?;
        job.cancel.check()?;
        let writer = {
            let mut state = job.state.lock().map_err(|_| unavailable())?;
            match std::mem::replace(&mut *state, State::Busy) {
                State::Download(writer) => writer,
                other => {
                    *state = other;
                    return Err(unavailable());
                }
            }
        };
        let result = (*writer).complete();
        self.jobs
            .lock()
            .unwrap_or_else(|failure| failure.into_inner())
            .remove(id);
        result
    }
    /// Acknowledges cancellation; active disk calls retain their owned state until they return.
    pub fn cancel(&self, id: &str) {
        let job = self
            .jobs
            .lock()
            .unwrap_or_else(|failure| failure.into_inner())
            .remove(id);
        if let Some(job) = job {
            job.cancel.cancel()
        }
    }
    pub fn cancel_all(&self) {
        let jobs = self
            .jobs
            .lock()
            .unwrap_or_else(|failure| failure.into_inner())
            .drain()
            .map(|(_, job)| job)
            .collect::<Vec<_>>();
        for job in jobs {
            job.cancel.cancel()
        }
    }
}
impl Drop for FileHandles {
    fn drop(&mut self) {
        self.cancel_all()
    }
}
