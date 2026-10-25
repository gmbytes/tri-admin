use admin_api::{
    config::Config,
    modules::files::storage::{BlobSpec, VerifiedSource},
};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;
fn config(root: &std::path::Path) -> Config {
    Config::from_lookup(|key| match key {
        "TRI_JWT_SIGNING_KEY" => Some("fixture-signing-key-at-least-32-bytes-long".into()),
        "TRI_STORAGE_ROOT" => Some(root.to_string_lossy().into_owned()),
        _ => None,
    })
    .unwrap()
}
fn old(path: &std::path::Path) {
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(7200))
        .unwrap();
}
#[tokio::test]
async fn stale_owned_temps_are_removed_but_fresh_and_unrelated_files_are_preserved() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join(".tri-staging");
    std::fs::create_dir(&directory).unwrap();
    let stale = directory.join(format!("{}.tmp", uuid::Uuid::new_v4()));
    let fresh = directory.join(format!("{}.tmp", uuid::Uuid::new_v4()));
    std::fs::write(&stale, b"old").unwrap();
    old(&stale);
    std::fs::write(&fresh, b"fresh").unwrap();
    std::fs::write(directory.join("unrelated.txt"), b"preserve").unwrap();
    let result = VerifiedSource::sweep(&config(root.path())).await.unwrap();
    assert_eq!(result.removed, 1);
    assert!(!stale.exists());
    assert!(fresh.exists());
    assert_eq!(
        std::fs::read(directory.join("unrelated.txt")).unwrap(),
        b"preserve"
    );
}
#[tokio::test]
async fn active_old_sources_remain_protected_by_a_separate_lease() {
    let root = tempfile::tempdir().unwrap();
    let config = config(root.path());
    let source = VerifiedSource::stage(
        &config,
        b"data".as_slice(),
        BlobSpec::new(4, &format!("{:x}", Sha256::digest(b"data"))).unwrap(),
        CancellationToken::new(),
    )
    .await
    .unwrap();
    let directory = root.path().join(".tri-staging");
    let data = std::fs::read_dir(&directory)
        .unwrap()
        .map(Result::unwrap)
        .map(|entry| entry.path())
        .find(|path| path.extension().is_some_and(|value| value == "tmp"))
        .unwrap();
    old(&data);
    let result = VerifiedSource::sweep(&config).await.unwrap();
    assert_eq!(result.removed, 0);
    assert_eq!(result.active, 1);
    assert!(data.exists());
    drop(source);
    assert_eq!(std::fs::read_dir(directory).unwrap().count(), 0);
}

#[tokio::test]
async fn staging_leases_protect_a_live_process_and_release_after_its_death() {
    const CHILD_ROOT: &str = "TRI_STAGING_CRASH_TEST_ROOT";
    if let Some(root) = std::env::var_os(CHILD_ROOT) {
        let root = std::path::PathBuf::from(root).canonicalize().unwrap();
        assert!(
            root.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("tri_admin_staging_test_")
        );
        let _source = VerifiedSource::stage(
            &config(&root),
            b"data".as_slice(),
            BlobSpec::new(4, &format!("{:x}", Sha256::digest(b"data"))).unwrap(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
        std::fs::write(root.join("ready"), b"ready").unwrap();
        std::future::pending::<()>().await;
        return;
    }
    struct Child(std::process::Child);
    impl Drop for Child {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let root = tempfile::Builder::new()
        .prefix("tri_admin_staging_test_")
        .tempdir()
        .unwrap();
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "staging_leases_protect_a_live_process_and_release_after_its_death",
            "--nocapture",
        ])
        .env(CHILD_ROOT, root.path())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = Child(command.spawn().unwrap());
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while !root.path().join("ready").exists() {
            assert!(
                child.0.try_wait().unwrap().is_none(),
                "staging fixture exited before holding a lease"
            );
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let directory = root.path().join(".tri-staging");
    let data = std::fs::read_dir(&directory)
        .unwrap()
        .map(Result::unwrap)
        .map(|row| row.path())
        .find(|path| path.extension().is_some_and(|value| value == "tmp"))
        .unwrap();
    old(&data);
    let result = VerifiedSource::sweep(&config(root.path())).await.unwrap();
    assert_eq!(result.removed, 0);
    assert_eq!(result.active, 1);
    child.0.kill().unwrap();
    child.0.wait().unwrap();
    let result = VerifiedSource::sweep(&config(root.path())).await.unwrap();
    assert_eq!(result.removed, 1);
    assert_eq!(std::fs::read_dir(directory).unwrap().count(), 0);
    assert_eq!(std::fs::read(root.path().join("ready")).unwrap(), b"ready");
}
