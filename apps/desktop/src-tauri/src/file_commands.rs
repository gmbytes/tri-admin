use desktop_core::{
    DesktopError,
    files::{FileHandles, FileJobKind, FileSpec, SaveMode, SaveReceipt, SourceInfo, WRITE_BLOCK},
};
use std::sync::Arc;
use tauri::{
    AppHandle, State,
    ipc::{Channel, InvokeBody, Request, Response},
};
use tauri_plugin_dialog::{DialogExt, FilePath};
pub type SharedFiles = Arc<FileHandles>;
const MAX_WRITE_BATCH: usize = 1024 * 1024;
fn failure(code: &'static str, message: &'static str) -> DesktopError {
    DesktopError { code, message }
}
fn internal() -> DesktopError {
    failure("FILE_IO", "本地文件操作暂不可用")
}
fn selected(path: Option<FilePath>) -> Result<Option<std::path::PathBuf>, DesktopError> {
    path.map(|value| {
        value
            .into_path()
            .map_err(|_| failure("FILE_SOURCE_INVALID", "请选择本地文件"))
    })
    .transpose()
}
fn default_name(value: &str) -> String {
    let value = value
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default()
        .chars()
        .take(240)
        .map(|c| {
            if c.is_control() || "<>:\"/\\|?*".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect::<String>();
    let value = value.trim().trim_end_matches('.');
    if value.is_empty() {
        "download.bin".into()
    } else {
        value.into()
    }
}

#[tauri::command]
pub fn file_reserve(
    state: State<'_, SharedFiles>,
    kind: FileJobKind,
) -> Result<String, DesktopError> {
    state.reserve(kind)
}
#[tauri::command(rename_all = "camelCase")]
pub async fn file_select_source(
    app: AppHandle,
    state: State<'_, SharedFiles>,
    handle: String,
    progress: Channel<u64>,
) -> Result<Option<SourceInfo>, DesktopError> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let peer = state.clone();
        let watched = handle.clone();
        let mut last_progress = std::time::Instant::now();
        state.select_source(
            &handle,
            || selected(app.dialog().file().blocking_pick_file()),
            move |bytes| {
                if last_progress.elapsed() < std::time::Duration::from_millis(100) {
                    return;
                }
                last_progress = std::time::Instant::now();
                if progress.send(bytes).is_err() {
                    peer.cancel(&watched)
                }
            },
        )
    })
    .await
    .map_err(|_| internal())?
}
#[tauri::command(rename_all = "camelCase")]
pub async fn file_select_download(
    app: AppHandle,
    state: State<'_, SharedFiles>,
    handle: String,
    spec: FileSpec,
    name: String,
) -> Result<bool, DesktopError> {
    let state = state.inner().clone();
    let name = default_name(&name);
    tauri::async_runtime::spawn_blocking(move || {
        state.select_download(&handle, spec, SaveMode::Replace, || {
            selected(app.dialog().file().set_file_name(name).blocking_save_file())
        })
    })
    .await
    .map_err(|_| internal())?
}
#[tauri::command(rename_all = "camelCase")]
pub async fn file_read_chunk(
    state: State<'_, SharedFiles>,
    handle: String,
    index: u32,
) -> Result<Response, DesktopError> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        state.read_chunk(&handle, index).map(Response::new)
    })
    .await
    .map_err(|_| internal())?
}
#[tauri::command]
pub async fn file_write(
    state: State<'_, SharedFiles>,
    request: Request<'_>,
) -> Result<(), DesktopError> {
    let handle = request
        .headers()
        .get("x-tri-file-handle")
        .and_then(|value| value.to_str().ok())
        .filter(|value| value.len() == 36)
        .ok_or_else(|| failure("FILE_HANDLE_INVALID", "文件句柄无效"))?
        .to_owned();
    let InvokeBody::Raw(bytes) = request.body() else {
        return Err(failure("FILE_BLOCK_INVALID", "文件写入必须使用二进制内容"));
    };
    if bytes.len() > MAX_WRITE_BATCH {
        return Err(failure("FILE_BLOCK_INVALID", "文件写入批次超过 1 MiB"));
    }
    let bytes = bytes.clone();
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        for block in bytes.chunks(WRITE_BLOCK) {
            state.write(&handle, block)?;
        }
        Ok(())
    })
    .await
    .map_err(|_| internal())?
}
#[tauri::command(rename_all = "camelCase")]
pub async fn file_complete(
    state: State<'_, SharedFiles>,
    handle: String,
) -> Result<SaveReceipt, DesktopError> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || state.complete(&handle))
        .await
        .map_err(|_| internal())?
}
#[tauri::command(rename_all = "camelCase")]
pub async fn file_cancel(
    state: State<'_, SharedFiles>,
    handle: String,
) -> Result<(), DesktopError> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || state.cancel(&handle))
        .await
        .map_err(|_| internal())
}
#[tauri::command]
pub async fn file_cancel_all(state: State<'_, SharedFiles>) -> Result<(), DesktopError> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || state.cancel_all())
        .await
        .map_err(|_| internal())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn untrusted_display_names_do_not_become_dialog_paths() {
        assert_eq!(default_name("../../secret:data?.bin"), "secret_data_.bin");
        assert_eq!(default_name(" . "), "download.bin");
        assert_eq!(default_name("资料 文件.bin"), "资料 文件.bin");
        assert_eq!(default_name(".profile"), ".profile");
        assert!(default_name(&"a".repeat(1000)).len() <= 240);
    }
}
