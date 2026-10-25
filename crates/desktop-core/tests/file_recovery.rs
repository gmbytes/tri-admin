use desktop_core::files::{FileCancel, FileSpec, NativeFileJournal, NativeWriter, SaveMode};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
fn spec() -> FileSpec {
    FileSpec::new(17, format!("{:x}", Sha256::digest(b"complete contents"))).unwrap()
}
fn root() -> tempfile::TempDir {
    let root = tempfile::Builder::new()
        .prefix("tri_admin_native_recovery_")
        .tempdir()
        .unwrap();
    std::fs::create_dir(root.path().join("downloads")).unwrap();
    std::fs::write(root.path().join("downloads/target.bin"), b"original").unwrap();
    std::fs::write(root.path().join("downloads/unrelated.txt"), b"preserve").unwrap();
    root
}
fn parts(root: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(root.join("downloads"))
        .unwrap()
        .map(Result::unwrap)
        .map(|row| row.path())
        .filter(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(".tri-admin-download-")
        })
        .collect()
}
struct Child(std::process::Child);
impl Child {
    fn stop(&mut self) {
        self.0.kill().unwrap();
        self.0.wait().unwrap();
    }
}
impl Drop for Child {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn child(root: &Path, count: usize) -> Child {
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "process_death_releases_the_lease_and_only_owned_parts_are_cleaned",
            "--nocapture",
        ])
        .env("TRI_NATIVE_RECOVERY_CHILD_ROOT", root)
        .env("TRI_NATIVE_RECOVERY_CHILD_COUNT", count.to_string())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = Child(command.spawn().unwrap());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !root.join("ready").exists() {
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "fixture exited before holding its journal lease"
        );
        assert!(
            std::time::Instant::now() < deadline,
            "fixture did not become ready"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    child
}
#[test]
fn process_death_releases_the_lease_and_only_owned_parts_are_cleaned() {
    if let Some(root) = std::env::var_os("TRI_NATIVE_RECOVERY_CHILD_ROOT") {
        let root = PathBuf::from(root).canonicalize().unwrap();
        assert!(root.starts_with(std::env::temp_dir().canonicalize().unwrap()));
        assert!(
            root.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("tri_admin_native_recovery_")
        );
        let count = std::env::var("TRI_NATIVE_RECOVERY_CHILD_COUNT")
            .unwrap()
            .parse::<usize>()
            .unwrap();
        assert!((1..=128).contains(&count));
        let journal = NativeFileJournal::open(&root.join("state")).unwrap();
        let mut writers = Vec::new();
        for index in 0..count {
            let name = if count == 1 {
                "target.bin".into()
            } else {
                format!("target-{index}.bin")
            };
            let mut writer = NativeWriter::prepare(
                &root.join("downloads").join(name),
                spec(),
                SaveMode::Replace,
                &FileCancel::default(),
                &journal,
            )
            .unwrap();
            writer.write(b"partial").unwrap();
            writers.push(writer)
        }
        std::fs::write(root.join("ready"), b"ready").unwrap();
        loop {
            std::thread::park()
        }
    }
    let root = root();
    let mut child = child(root.path(), 1);
    let journal = NativeFileJournal::open(&root.path().join("state")).unwrap();
    let active = journal.recover().unwrap();
    assert_eq!(active.active, 1);
    assert_eq!(active.cleaned, 0);
    assert_eq!(parts(root.path()).len(), 1);
    child.stop();
    let restarted = NativeFileJournal::open(&root.path().join("state")).unwrap();
    let result = restarted.recover().unwrap();
    assert_eq!(result.cleaned, 1);
    assert_eq!(result.pending, 0);
    assert!(parts(root.path()).is_empty());
    assert_eq!(
        std::fs::read(root.path().join("downloads/target.bin")).unwrap(),
        b"original"
    );
    assert_eq!(
        std::fs::read(root.path().join("downloads/unrelated.txt")).unwrap(),
        b"preserve"
    );
    assert!(
        !serde_json::to_string(&result)
            .unwrap()
            .contains(root.path().to_str().unwrap())
    );
}
#[test]
fn changed_file_identity_and_replaced_parent_are_not_deleted() {
    for replace_parent in [false, true] {
        let root = root();
        let mut child = child(root.path(), 1);
        child.stop();
        let part = parts(root.path()).pop().unwrap();
        if replace_parent {
            std::fs::rename(
                root.path().join("downloads"),
                root.path().join("old-downloads"),
            )
            .unwrap();
            std::fs::create_dir(root.path().join("downloads")).unwrap();
            std::fs::write(
                root.path()
                    .join("downloads")
                    .join(part.file_name().unwrap()),
                b"replacement",
            )
            .unwrap()
        } else {
            std::fs::rename(&part, root.path().join("downloads/moved-by-user.bin")).unwrap();
            std::fs::write(&part, b"replacement").unwrap()
        }
        let journal = NativeFileJournal::open(&root.path().join("state")).unwrap();
        let result = journal.recover().unwrap();
        assert_eq!(result.cleaned, 0);
        assert_eq!(result.pending, 1);
        assert_eq!(
            std::fs::read(
                root.path()
                    .join("downloads")
                    .join(part.file_name().unwrap())
            )
            .unwrap(),
            b"replacement"
        );
    }
}
#[test]
fn unproven_or_corrupt_journals_are_retained_without_deleting_user_files() {
    for corrupt in [false, true] {
        let root = root();
        let mut child = child(root.path(), 1);
        child.stop();
        let record = std::fs::read_dir(root.path().join("state"))
            .unwrap()
            .map(Result::unwrap)
            .map(|row| row.path())
            .find(|path| path.extension().is_some_and(|value| value == "journal"))
            .unwrap();
        let content = std::fs::read_to_string(&record).unwrap();
        let header = content.lines().next().unwrap();
        std::fs::write(
            &record,
            if corrupt {
                "corrupt".to_owned()
            } else {
                format!("{header}\n")
            },
        )
        .unwrap();
        let journal = NativeFileJournal::open(&root.path().join("state")).unwrap();
        let result = journal.recover().unwrap();
        assert_eq!(result.cleaned, 0);
        assert_eq!(parts(root.path()).len(), 1);
        if corrupt {
            assert_eq!(result.invalid, 1)
        } else {
            assert_eq!(result.pending, 1)
        }
    }
}
#[test]
fn successful_save_and_abort_retire_their_records_without_gc_touching_the_target() {
    let root = root();
    let journal = NativeFileJournal::open(&root.path().join("state")).unwrap();
    let mut writer = NativeWriter::prepare(
        &root.path().join("downloads/target.bin"),
        spec(),
        SaveMode::Replace,
        &FileCancel::default(),
        &journal,
    )
    .unwrap();
    writer.write(b"complete contents").unwrap();
    assert!(!writer.complete().unwrap().cleanup_pending);
    let mut writer = NativeWriter::prepare(
        &root.path().join("downloads/aborted.bin"),
        spec(),
        SaveMode::New,
        &FileCancel::default(),
        &journal,
    )
    .unwrap();
    writer.write(b"partial").unwrap();
    writer.abort().unwrap();
    let result = journal.recover().unwrap();
    assert_eq!(result.scanned, 0);
    assert_eq!(
        std::fs::read(root.path().join("downloads/target.bin")).unwrap(),
        b"complete contents"
    );
    assert!(parts(root.path()).is_empty());
}
#[test]
fn cleanup_is_bounded_and_continues_on_the_next_pass() {
    let root = root();
    let mut child = child(root.path(), 120);
    child.stop();
    let journal = NativeFileJournal::open(&root.path().join("state")).unwrap();
    let first = journal.recover().unwrap();
    assert_eq!(first.cleaned, 100);
    assert_eq!(parts(root.path()).len(), 20);
    let second = journal.recover().unwrap();
    assert_eq!(second.cleaned, 20);
    assert!(parts(root.path()).is_empty());
}

#[test]
fn a_replaced_temporary_file_is_neither_published_nor_deleted() {
    let root = root();
    let journal = NativeFileJournal::open(&root.path().join("state")).unwrap();
    let mut writer = NativeWriter::prepare(
        &root.path().join("downloads/target.bin"),
        spec(),
        SaveMode::Replace,
        &FileCancel::default(),
        &journal,
    )
    .unwrap();
    writer.write(b"complete contents").unwrap();
    let part = parts(root.path()).pop().unwrap();
    std::fs::rename(&part, root.path().join("downloads/kept-by-user.bin")).unwrap();
    std::fs::write(&part, b"unrelated replacement").unwrap();
    assert_eq!(writer.complete().unwrap_err().code, "FILE_INTEGRITY");
    assert_eq!(std::fs::read(&part).unwrap(), b"unrelated replacement");
    assert_eq!(
        std::fs::read(root.path().join("downloads/target.bin")).unwrap(),
        b"original"
    );
    assert_eq!(journal.recover().unwrap().pending, 1);
}
#[test]
fn saving_into_the_private_recovery_directory_is_rejected() {
    let root = root();
    let state = root.path().join("state");
    let journal = NativeFileJournal::open(&state).unwrap();
    let result = NativeWriter::prepare(
        &state.join("user-target.bin"),
        spec(),
        SaveMode::Replace,
        &FileCancel::default(),
        &journal,
    );
    assert!(result.is_err());
    assert_eq!(std::fs::read_dir(state).unwrap().count(), 0);
}
#[test]
fn in_place_temporary_changes_are_verified_from_disk_before_publication() {
    let root = root();
    let journal = NativeFileJournal::open(&root.path().join("state")).unwrap();
    let mut writer = NativeWriter::prepare(
        &root.path().join("downloads/target.bin"),
        spec(),
        SaveMode::Replace,
        &FileCancel::default(),
        &journal,
    )
    .unwrap();
    writer.write(b"complete contents").unwrap();
    let part = parts(root.path()).pop().unwrap();
    std::fs::write(part, b"changed contents!").unwrap();
    assert_eq!(writer.complete().unwrap_err().code, "FILE_INTEGRITY");
    assert_eq!(
        std::fs::read(root.path().join("downloads/target.bin")).unwrap(),
        b"original"
    );
}
#[test]
fn unavailable_directories_keep_a_ticket_and_are_retried_after_they_return() {
    let root = root();
    let mut child = child(root.path(), 1);
    child.stop();
    std::fs::rename(
        root.path().join("downloads"),
        root.path().join("temporarily-offline"),
    )
    .unwrap();
    let journal = NativeFileJournal::open(&root.path().join("state")).unwrap();
    assert_eq!(journal.recover().unwrap().pending, 1);
    assert_eq!(
        std::fs::read_dir(root.path().join("state"))
            .unwrap()
            .count(),
        1
    );
    std::fs::rename(
        root.path().join("temporarily-offline"),
        root.path().join("downloads"),
    )
    .unwrap();
    assert_eq!(journal.recover().unwrap().cleaned, 1);
    assert!(parts(root.path()).is_empty());
    assert_eq!(
        std::fs::read(root.path().join("downloads/target.bin")).unwrap(),
        b"original"
    );
}
