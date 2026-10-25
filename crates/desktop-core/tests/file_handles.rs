use desktop_core::files::{FileHandles, FileJobKind, FileSpec, SaveMode};
use sha2::{Digest, Sha256};
use std::sync::Arc;
fn spec(bytes: &[u8]) -> FileSpec {
    FileSpec::new(bytes.len() as u64, format!("{:x}", Sha256::digest(bytes))).unwrap()
}

#[test]
fn reserved_handles_are_bounded_and_cancel_all_releases_the_capacity() {
    let handles_state = tempfile::tempdir().unwrap();
    let handles = FileHandles::open(handles_state.path()).unwrap();
    let mut ids = Vec::new();
    for _ in 0..32 {
        ids.push(handles.reserve(FileJobKind::Source).unwrap())
    }
    assert_eq!(
        handles.reserve(FileJobKind::Download).unwrap_err().code,
        "FILE_CAPACITY"
    );
    handles.cancel_all();
    for id in ids {
        assert_eq!(
            handles.read_chunk(&id, 0).unwrap_err().code,
            "FILE_HANDLE_INVALID"
        )
    }
    assert!(handles.reserve(FileJobKind::Download).is_ok());
}

#[test]
fn source_handles_expose_metadata_and_bytes_but_do_not_change_kind() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("source.bin");
    std::fs::write(&path, b"data").unwrap();
    let handles_state = tempfile::tempdir().unwrap();
    let handles = FileHandles::open(handles_state.path()).unwrap();
    let id = handles.reserve(FileJobKind::Source).unwrap();
    assert_eq!(handles.read_chunk(&id, 0).unwrap_err().code, "FILE_STATE");
    let info = handles
        .select_source(&id, || Ok(Some(path.clone())), |_| {})
        .unwrap()
        .unwrap();
    assert_eq!(info.size, 4);
    assert_eq!(handles.read_chunk(&id, 0).unwrap(), b"data");
    assert_eq!(handles.complete(&id).unwrap_err().code, "FILE_STATE");
    assert_eq!(handles.read_chunk(&id, 0).unwrap(), b"data");
    assert!(
        handles
            .select_download(&id, spec(b"data"), SaveMode::New, || Ok(Some(
                directory.path().join("other.bin")
            )))
            .is_err()
    );
    assert!(!directory.path().join("other.bin").exists());
    handles.cancel(&id);
    handles.cancel(&id);
    assert_eq!(
        handles.read_chunk(&id, 0).unwrap_err().code,
        "FILE_HANDLE_INVALID"
    );
}

#[test]
fn picker_cancellation_and_cancellation_while_hashing_do_not_leave_live_handles() {
    let handles_state = tempfile::tempdir().unwrap();
    let handles = Arc::new(FileHandles::open(handles_state.path()).unwrap());
    let id = handles.reserve(FileJobKind::Source).unwrap();
    assert!(
        handles
            .select_source(&id, || Ok(None), |_| {})
            .unwrap()
            .is_none()
    );
    assert_eq!(
        handles.read_chunk(&id, 0).unwrap_err().code,
        "FILE_HANDLE_INVALID"
    );
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("source.bin");
    std::fs::write(&path, vec![1; 128 * 1024]).unwrap();
    let id = handles.reserve(FileJobKind::Source).unwrap();
    let peer = handles.clone();
    let cancel_id = id.clone();
    let result = handles.select_source(&id, || Ok(Some(path)), move |_| peer.cancel(&cancel_id));
    assert_eq!(result.err().unwrap().code, "REQUEST_CANCELLED");
    for _ in 0..32 {
        assert!(handles.reserve(FileJobKind::Source).is_ok())
    }
}

#[test]
fn a_pending_picker_cannot_install_a_source_after_cancel_all() {
    let handles_state = tempfile::tempdir().unwrap();
    let handles = Arc::new(FileHandles::open(handles_state.path()).unwrap());
    let id = handles.reserve(FileJobKind::Source).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("source.bin");
    std::fs::write(&path, b"data").unwrap();
    let peer = handles.clone();
    let result = handles.select_source(
        &id,
        move || {
            peer.cancel_all();
            Ok(Some(path))
        },
        |_| {},
    );
    assert_eq!(result.err().unwrap().code, "REQUEST_CANCELLED");
    assert_eq!(
        handles.read_chunk(&id, 0).unwrap_err().code,
        "FILE_HANDLE_INVALID"
    );
}

#[test]
fn download_handles_commit_once_and_cancel_all_prevents_a_late_commit() {
    let directory = tempfile::tempdir().unwrap();
    let handles_state = tempfile::tempdir().unwrap();
    let handles = FileHandles::open(handles_state.path()).unwrap();
    let target = directory.path().join("saved.bin");
    let id = handles.reserve(FileJobKind::Download).unwrap();
    assert!(
        handles
            .select_download(&id, spec(b"data"), SaveMode::New, || Ok(Some(
                target.clone()
            )))
            .unwrap()
    );
    handles.write(&id, b"data").unwrap();
    let receipt = handles.complete(&id).unwrap();
    assert_eq!(receipt.size, 4);
    assert_eq!(std::fs::read(&target).unwrap(), b"data");
    assert_eq!(
        handles.complete(&id).unwrap_err().code,
        "FILE_HANDLE_INVALID"
    );
    let cancelled = directory.path().join("cancelled.bin");
    let id = handles.reserve(FileJobKind::Download).unwrap();
    handles
        .select_download(&id, spec(b"data"), SaveMode::New, || {
            Ok(Some(cancelled.clone()))
        })
        .unwrap();
    handles.write(&id, b"data").unwrap();
    handles.cancel_all();
    assert_eq!(
        handles.complete(&id).unwrap_err().code,
        "FILE_HANDLE_INVALID"
    );
    assert!(!cancelled.exists());
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
}
