use commands::SharedFiles;
use desktop_core::files::{FileHandles, FileJobKind, FileSpec, SaveMode, WRITE_BLOCK};
use serde_json::{Value, json};
use std::sync::Arc;
use tauri::ipc::InvokeBody;
use tauri::{
    ipc::{CallbackFn, InvokeResponseBody},
    test::{MockRuntime, get_ipc_response, mock_builder, mock_context, noop_assets},
    webview::InvokeRequest,
};
use tri_admin_desktop_lib::file_commands as commands;
fn fixture(files: SharedFiles) -> (tauri::App<MockRuntime>, tauri::WebviewWindow<MockRuntime>) {
    let app = mock_builder()
        .manage(files)
        .invoke_handler(tauri::generate_handler![
            commands::file_reserve,
            commands::file_read_chunk,
            commands::file_write,
            commands::file_complete,
            commands::file_cancel,
            commands::file_cancel_all
        ])
        .build(mock_context(noop_assets()))
        .unwrap();
    let window = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();
    (app, window)
}
fn request(command: &str, body: InvokeBody, handle: Option<&str>) -> InvokeRequest {
    let mut headers = tauri::http::HeaderMap::new();
    if let Some(handle) = handle {
        headers.insert("x-tri-file-handle", handle.parse().unwrap());
    }
    InvokeRequest {
        cmd: command.into(),
        callback: CallbackFn(0),
        error: CallbackFn(1),
        url: "http://tauri.localhost".parse().unwrap(),
        body,
        headers,
        invoke_key: tauri::test::INVOKE_KEY.to_string(),
    }
}
#[test]
fn mock_ipc_accepts_a_bounded_batch_and_keeps_block_order() {
    let files_state = tempfile::tempdir().unwrap();
    let files = Arc::new(FileHandles::open(files_state.path()).unwrap());
    let (_app, window) = fixture(files.clone());
    let directory = tempfile::tempdir().unwrap();
    let data = vec![41u8; WRITE_BLOCK * 2 + 17];
    let source = directory.path().join("source.bin");
    std::fs::write(&source, &data).unwrap();
    let source = desktop_core::files::NativeSource::open(
        &source,
        &desktop_core::files::FileCancel::default(),
    )
    .unwrap();
    let target = directory.path().join("saved.bin");
    let handle = files.reserve(FileJobKind::Download).unwrap();
    files
        .select_download(
            &handle,
            FileSpec::new(data.len() as u64, source.info().sha256.clone()).unwrap(),
            SaveMode::New,
            || Ok(Some(target.clone())),
        )
        .unwrap();
    get_ipc_response(
        &window,
        request("file_write", InvokeBody::Raw(data.clone()), Some(&handle)),
    )
    .unwrap();
    get_ipc_response(
        &window,
        request(
            "file_complete",
            InvokeBody::Json(json!({"handle":handle})),
            None,
        ),
    )
    .unwrap();
    assert_eq!(std::fs::read(target).unwrap(), data);
}
#[test]
fn mock_ipc_transfers_raw_source_and_download_bytes_without_opening_a_window() {
    let files_state = tempfile::tempdir().unwrap();
    let files = Arc::new(FileHandles::open(files_state.path()).unwrap());
    let (_app, window) = fixture(files.clone());
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source.bin");
    std::fs::write(&source, b"data").unwrap();
    let handle = get_ipc_response(
        &window,
        request(
            "file_reserve",
            InvokeBody::Json(json!({"kind":"source"})),
            None,
        ),
    )
    .unwrap()
    .deserialize::<String>()
    .unwrap();
    let info = files
        .select_source(&handle, || Ok(Some(source)), |_| {})
        .unwrap()
        .unwrap();
    assert_eq!(info.size, 4);
    let response = get_ipc_response(
        &window,
        request(
            "file_read_chunk",
            InvokeBody::Json(json!({"handle":handle,"index":0})),
            None,
        ),
    )
    .unwrap();
    match response {
        InvokeResponseBody::Raw(bytes) => assert_eq!(bytes, b"data"),
        _ => panic!("source chunk was JSON encoded"),
    }
    let target = directory.path().join("saved.bin");
    let writer = get_ipc_response(
        &window,
        request(
            "file_reserve",
            InvokeBody::Json(json!({"kind":"download"})),
            None,
        ),
    )
    .unwrap()
    .deserialize::<String>()
    .unwrap();
    let spec = FileSpec::new(4, info.sha256.clone()).unwrap();
    files
        .select_download(&writer, spec, SaveMode::New, || Ok(Some(target.clone())))
        .unwrap();
    get_ipc_response(
        &window,
        request(
            "file_write",
            InvokeBody::Raw(b"data".to_vec()),
            Some(&writer),
        ),
    )
    .unwrap();
    assert!(!target.exists());
    let receipt = get_ipc_response(
        &window,
        request(
            "file_complete",
            InvokeBody::Json(json!({"handle":writer})),
            None,
        ),
    )
    .unwrap()
    .deserialize::<Value>()
    .unwrap();
    assert_eq!(receipt["size"], 4);
    assert_eq!(receipt["sha256"], info.sha256);
    assert_eq!(std::fs::read(&target).unwrap(), b"data");
    get_ipc_response(
        &window,
        request("file_cancel_all", InvokeBody::Json(json!({})), None),
    )
    .unwrap();
    assert_eq!(
        files.read_chunk(&handle, 0).unwrap_err().code,
        "FILE_HANDLE_INVALID"
    );
}
#[test]
fn mock_ipc_rejects_non_binary_or_oversized_writes_and_redacts_disk_errors() {
    let files_state = tempfile::tempdir().unwrap();
    let files = Arc::new(FileHandles::open(files_state.path()).unwrap());
    let (_app, window) = fixture(files.clone());
    let directory = tempfile::tempdir().unwrap();
    let target = directory.path().join("unchanged.bin");
    std::fs::write(&target, b"original").unwrap();
    let writer = files.reserve(FileJobKind::Download).unwrap();
    let spec = FileSpec::new(4, "0".repeat(64)).unwrap();
    files
        .select_download(&writer, spec, SaveMode::Replace, || {
            Ok(Some(target.clone()))
        })
        .unwrap();
    let rejected = get_ipc_response(
        &window,
        request(
            "file_write",
            InvokeBody::Json(json!({"bytes":[1]})),
            Some(&writer),
        ),
    )
    .unwrap_err();
    assert_eq!(rejected["code"], "FILE_BLOCK_INVALID");
    let rejected = get_ipc_response(
        &window,
        request(
            "file_write",
            InvokeBody::Raw(vec![0; 1024 * 1024 + 1]),
            Some(&writer),
        ),
    )
    .unwrap_err();
    assert_eq!(rejected["code"], "FILE_BLOCK_INVALID");
    get_ipc_response(
        &window,
        request(
            "file_write",
            InvokeBody::Raw(b"data".to_vec()),
            Some(&writer),
        ),
    )
    .unwrap();
    let failure = get_ipc_response(
        &window,
        request(
            "file_complete",
            InvokeBody::Json(json!({"handle":writer})),
            None,
        ),
    )
    .unwrap_err();
    assert_eq!(failure["code"], "FILE_INTEGRITY");
    assert!(
        !failure
            .to_string()
            .contains(directory.path().to_str().unwrap())
    );
    assert_eq!(std::fs::read(&target).unwrap(), b"original");
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
}
