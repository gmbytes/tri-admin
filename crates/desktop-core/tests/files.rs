use desktop_core::files::{
    CHUNK_SIZE, FileCancel, FileSpec, MAX_FILE_SIZE, NativeFileJournal, NativeSource, NativeWriter,
    SaveMode, WRITE_BLOCK,
};
use sha2::{Digest, Sha256};
use std::io::Write;

fn spec(data: &[u8]) -> FileSpec {
    FileSpec::new(data.len() as u64, format!("{:x}", Sha256::digest(data))).unwrap()
}
fn write_all(writer: &mut NativeWriter, data: &[u8]) {
    for block in data.chunks(WRITE_BLOCK) {
        writer.write(block).unwrap();
    }
}

#[test]
fn selected_source_is_hashed_and_read_in_fixed_chunks_without_exposing_its_path() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("source.bin");
    let data = vec![37u8; CHUNK_SIZE + 17];
    std::fs::write(&path, &data).unwrap();
    let cancel = FileCancel::default();
    let mut source = NativeSource::open(&path, &cancel).unwrap();
    assert_eq!(source.info().size, data.len() as u64);
    assert_eq!(source.info().sha256, spec(&data).sha256());
    let public = serde_json::to_string(source.info()).unwrap();
    assert!(!public.contains(directory.path().to_str().unwrap()));
    assert!(!public.contains("path"));
    assert_eq!(source.read_chunk(0).unwrap().len(), CHUNK_SIZE);
    assert_eq!(source.read_chunk(1).unwrap(), vec![37u8; 17]);
    assert_eq!(source.read_chunk(2).unwrap_err().code, "FILE_CHUNK_INVALID");
    cancel.cancel();
    assert_eq!(source.read_chunk(0).unwrap_err().code, "REQUEST_CANCELLED");
}

#[test]
fn source_changes_and_cancellation_during_hashing_are_detected() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("source.bin");
    std::fs::write(&path, vec![7u8; WRITE_BLOCK * 3]).unwrap();
    let cancel = FileCancel::default();
    let cancelled = cancel.clone();
    let failure = NativeSource::open_with_progress(&path, &cancel, move |bytes| {
        if bytes > 0 {
            cancelled.cancel()
        }
    })
    .err()
    .unwrap();
    assert_eq!(failure.code, "REQUEST_CANCELLED");
    let mut source = NativeSource::open(&path, &FileCancel::default()).unwrap();
    std::fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_len(1)
        .unwrap();
    assert_eq!(
        source.read_chunk(0).unwrap_err().code,
        "FILE_SOURCE_CHANGED"
    );
}

#[test]
fn verified_download_is_published_only_after_complete_and_leaves_no_temporary_file() {
    let journal_state = tempfile::tempdir().unwrap();
    let journal = NativeFileJournal::open(journal_state.path()).unwrap();

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("download.bin");
    let data = vec![93u8; WRITE_BLOCK * 3 + 17];
    let expected = spec(&data);
    let mut writer = NativeWriter::prepare(
        &path,
        expected.clone(),
        SaveMode::New,
        &FileCancel::default(),
        &journal,
    )
    .unwrap();
    write_all(&mut writer, &data);
    assert!(!path.exists());
    let result = writer.complete().unwrap();
    assert_eq!(result.size, data.len() as u64);
    assert_eq!(result.sha256, expected.sha256());
    assert!(!result.cleanup_pending);
    assert_eq!(std::fs::read(&path).unwrap(), data);
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
}

#[test]
fn cancelled_or_corrupt_download_does_not_replace_the_existing_file() {
    let journal_state = tempfile::tempdir().unwrap();
    let journal = NativeFileJournal::open(journal_state.path()).unwrap();

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("preserve.bin");
    std::fs::write(&path, b"original").unwrap();
    let expected = spec(b"replacement");
    let mut writer = NativeWriter::prepare(
        &path,
        expected.clone(),
        SaveMode::Replace,
        &FileCancel::default(),
        &journal,
    )
    .unwrap();
    writer.write(b"corruptions").unwrap();
    assert_eq!(writer.complete().unwrap_err().code, "FILE_INTEGRITY");
    assert_eq!(std::fs::read(&path).unwrap(), b"original");
    let cancel = FileCancel::default();
    let mut writer = NativeWriter::prepare(
        &path,
        expected.clone(),
        SaveMode::Replace,
        &cancel,
        &journal,
    )
    .unwrap();
    writer.write(b"replacement").unwrap();
    cancel.cancel();
    assert_eq!(writer.complete().unwrap_err().code, "REQUEST_CANCELLED");
    assert_eq!(std::fs::read(&path).unwrap(), b"original");
    let mut writer = NativeWriter::prepare(
        &path,
        expected,
        SaveMode::Replace,
        &FileCancel::default(),
        &journal,
    )
    .unwrap();
    writer.write(b"replacement").unwrap();
    writer.complete().unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"replacement");
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
}

#[test]
fn new_file_publish_never_overwrites_a_competing_file_and_abort_removes_only_its_own_temp() {
    let journal_state = tempfile::tempdir().unwrap();
    let journal = NativeFileJournal::open(journal_state.path()).unwrap();

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("new.bin");
    let mut writer = NativeWriter::prepare(
        &path,
        spec(b"data"),
        SaveMode::New,
        &FileCancel::default(),
        &journal,
    )
    .unwrap();
    writer.write(b"data").unwrap();
    std::fs::write(&path, b"competitor").unwrap();
    assert_eq!(writer.complete().unwrap_err().code, "FILE_EXISTS");
    assert_eq!(std::fs::read(&path).unwrap(), b"competitor");
    let next = directory.path().join("abort.bin");
    let mut writer = NativeWriter::prepare(
        &next,
        spec(b"data"),
        SaveMode::New,
        &FileCancel::default(),
        &journal,
    )
    .unwrap();
    writer.write(b"da").unwrap();
    writer.abort().unwrap();
    assert!(!next.exists());
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
}

#[test]
fn lengths_hashes_blocks_and_non_file_sources_are_rejected() {
    let journal_state = tempfile::tempdir().unwrap();
    let journal = NativeFileJournal::open(journal_state.path()).unwrap();

    assert!(FileSpec::new(MAX_FILE_SIZE + 1, "0".repeat(64)).is_err());
    assert!(FileSpec::new(1, "bad").is_err());
    let directory = tempfile::tempdir().unwrap();
    assert!(NativeSource::open(directory.path(), &FileCancel::default()).is_err());
    let mut writer = NativeWriter::prepare(
        &directory.path().join("small.bin"),
        spec(b"a"),
        SaveMode::New,
        &FileCancel::default(),
        &journal,
    )
    .unwrap();
    assert_eq!(
        writer.write(&vec![0; WRITE_BLOCK + 1]).unwrap_err().code,
        "FILE_BLOCK_INVALID"
    );
    writer.abort().unwrap();
    let writer = NativeWriter::prepare(
        &directory.path().join("short.bin"),
        spec(b"ab"),
        SaveMode::New,
        &FileCancel::default(),
        &journal,
    )
    .unwrap();
    assert_eq!(writer.complete().unwrap_err().code, "FILE_INTEGRITY");
    let mut file = std::fs::File::create(directory.path().join("unrelated.txt")).unwrap();
    file.write_all(b"preserve").unwrap();
    drop(file);
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
}

#[test]
fn empty_files_and_invalid_deserialized_specs_follow_the_same_contract() {
    let journal_state = tempfile::tempdir().unwrap();
    let journal = NativeFileJournal::open(journal_state.path()).unwrap();

    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("empty.bin");
    std::fs::write(&source, []).unwrap();
    let mut empty = NativeSource::open(&source, &FileCancel::default()).unwrap();
    assert_eq!(empty.info().size, 0);
    assert_eq!(empty.read_chunk(0).unwrap_err().code, "FILE_CHUNK_INVALID");
    let destination = directory.path().join("saved.bin");
    NativeWriter::prepare(
        &destination,
        spec(b""),
        SaveMode::New,
        &FileCancel::default(),
        &journal,
    )
    .unwrap()
    .complete()
    .unwrap();
    assert_eq!(std::fs::metadata(&destination).unwrap().len(), 0);
    let invalid: FileSpec =
        serde_json::from_value(serde_json::json!({"size":1,"sha256":"invalid"})).unwrap();
    assert!(
        NativeWriter::prepare(
            &directory.path().join("invalid.bin"),
            invalid,
            SaveMode::New,
            &FileCancel::default(),
            &journal
        )
        .is_err()
    );
    assert!(
        serde_json::from_value::<FileSpec>(
            serde_json::json!({"size":0,"sha256":"0".repeat(64),"path":"forbidden"})
        )
        .is_err()
    );
}

#[test]
#[ignore = "writes and verifies two 5 GiB files; run the explicit native disk gate after checking free space"]
fn native_large_disk_streams_five_gib() {
    let journal_state = tempfile::tempdir().unwrap();
    let journal = NativeFileJournal::open(journal_state.path()).unwrap();

    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .canonicalize()
        .unwrap();
    let root = repo.join(".superpowers/native-file-tests");
    std::fs::create_dir_all(&root).unwrap();
    let root = root.canonicalize().unwrap();
    assert!(root.starts_with(&repo));
    let directory = tempfile::Builder::new()
        .prefix("tri_admin_native_large_")
        .tempdir_in(root)
        .unwrap();
    let source_path = directory.path().join("source.bin");
    let target = directory.path().join("saved.bin");
    let mut source_file = std::fs::File::create(&source_path).unwrap();
    let block = vec![91u8; WRITE_BLOCK];
    let mut digest = Sha256::new();
    let started = std::time::Instant::now();
    for _ in 0..MAX_FILE_SIZE / WRITE_BLOCK as u64 {
        source_file.write_all(&block).unwrap();
        digest.update(&block)
    }
    source_file.sync_all().unwrap();
    drop(source_file);
    let expected = format!("{:x}", digest.finalize());
    eprintln!("native disk gate: source created, bytes={MAX_FILE_SIZE}");
    let mut source = NativeSource::open(&source_path, &FileCancel::default()).unwrap();
    assert_eq!(source.info().sha256, expected);
    eprintln!("native disk gate: source hash verified");
    let mut writer = NativeWriter::prepare(
        &target,
        FileSpec::new(MAX_FILE_SIZE, &expected).unwrap(),
        SaveMode::New,
        &FileCancel::default(),
        &journal,
    )
    .unwrap();
    let mut largest = 0;
    for index in 0..(MAX_FILE_SIZE / CHUNK_SIZE as u64) as u32 {
        let chunk = source.read_chunk(index).unwrap();
        largest = largest.max(chunk.len());
        write_all(&mut writer, &chunk)
    }
    assert!(!target.exists());
    let saved = writer.complete().unwrap();
    assert_eq!(saved.size, MAX_FILE_SIZE);
    assert_eq!(largest, CHUNK_SIZE);
    eprintln!("native disk gate: target committed");
    let verified = NativeSource::open(&target, &FileCancel::default()).unwrap();
    assert_eq!(verified.info().sha256, expected);
    assert_eq!(verified.info().size, MAX_FILE_SIZE);
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 2);
    eprintln!(
        "native disk gate: target re-read and verified, elapsedSeconds={}",
        started.elapsed().as_secs_f64()
    );
}
