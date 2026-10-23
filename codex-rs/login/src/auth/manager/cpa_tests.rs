use super::*;
use pretty_assertions::assert_eq;
use std::cell::RefCell;
use std::fs::TryLockError;
use std::time::Duration;
use tokio::io::AsyncReadExt;

type LockObserver = Box<dyn FnOnce(&File)>;
thread_local! {
    // Test-only observation at the actual save's lock boundary, before reading JSON.
    static LOCK_OBSERVER: RefCell<Option<LockObserver>> = RefCell::new(None);
}

pub(super) fn before_lock(file: &File) {
    if let Some(observer) = LOCK_OBSERVER.with(|slot| slot.borrow_mut().take()) {
        observer(file);
    }
}

#[test]
#[ignore = "subprocess helper for cpa_shared_save_waits_for_latest_metadata"]
fn cpa_save_child() -> anyhow::Result<()> {
    let address = std::env::var("CODEX_TEST_CPA_LOCK_ADDRESS")?;
    let path = PathBuf::from(std::env::var_os("CODEX_TEST_CPA_CREDENTIAL").expect("fixture path"));
    LOCK_OBSERVER.with(|slot| {
        *slot.borrow_mut() = Some(Box::new(move |file| {
            assert!(matches!(file.try_lock(), Err(TryLockError::WouldBlock)));
            let mut signal = std::net::TcpStream::connect(address).expect("parent signal socket");
            signal.write_all(b"C").expect("report observed contention");
        }));
    });
    let result = CpaCredentialFile {
        path,
        credential_id: "fixture.json".into(),
    }
    .save_tokens(TokenData {
        access_token: "rotated-normal-token".into(),
        refresh_token: "rotated-normal-refresh".into(),
        ..Default::default()
    });
    match std::env::var("CODEX_TEST_CPA_LOCK_CASE")?.as_str() {
        "metadata" => result?,
        "disabled" => assert!(
            result
                .expect_err("disabled file")
                .to_string()
                .contains("disabled")
        ),
        "removed" => assert_eq!(
            result.expect_err("renamed file").kind(),
            io::ErrorKind::NotFound
        ),
        _ => anyhow::bail!("unknown fixture case"),
    }
    Ok(())
}

#[tokio::test]
async fn cpa_shared_save_waits_for_latest_metadata() -> anyhow::Result<()> {
    for case in ["metadata", "disabled", "removed"] {
        let root = tempfile::TempDir::new()?;
        let directory = root.path().join("credentials");
        std::fs::create_dir(&directory)?;
        let path = directory.join("Account Name.json");
        let original = json!({"type":"codex","codex_cli":{"enabled":true},
            "disabled":false,"access_token":"old-normal-token","siwc":{"access_token":"old-siwc"}});
        std::fs::write(&path, serde_json::to_vec(&original)?)?;
        // Exercise aliases of both the parent directory and the existing file on Unix.
        #[cfg(unix)]
        let child_path = {
            let alias = root.path().join("alias");
            std::os::unix::fs::symlink(&directory, &alias)?;
            let link = alias.join("linked.json");
            std::os::unix::fs::symlink(&path, &link)?;
            assert_eq!(
                canonical_credential_path(&link)?,
                std::fs::canonicalize(&path)?
            );
            assert_eq!(
                canonical_credential_path(&alias.join("missing.json"))?,
                std::fs::canonicalize(&directory)?.join("missing.json")
            );
            link
        };
        #[cfg(not(unix))]
        let child_path = path.clone();

        let canonical = std::fs::canonicalize(&path)?;
        let lock = CredentialLock::acquire(&canonical)?;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let child = tokio::process::Command::new(std::env::current_exe()?)
            .args([
                "--exact",
                "auth::manager::cpa::tests::cpa_save_child",
                "--ignored",
                "--nocapture",
            ])
            .env(
                "CODEX_TEST_CPA_LOCK_ADDRESS",
                listener.local_addr()?.to_string(),
            )
            .env("CODEX_TEST_CPA_CREDENTIAL", &child_path)
            .env("CODEX_TEST_CPA_LOCK_CASE", case)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;
        // Wait for a real WouldBlock from the save's lock descriptor. No timing guess
        // is used to infer that the child reached the critical section boundary.
        let (mut signal, _) =
            tokio::time::timeout(Duration::from_secs(15), listener.accept()).await??;
        let mut byte = [0];
        tokio::time::timeout(Duration::from_secs(15), signal.read_exact(&mut byte)).await??;
        assert_eq!(byte, *b"C");
        let mut latest = original.clone();
        latest["siwc"] =
            json!({"access_token":"new-siwc","nested":{"future":[true,{"keep":"exact"}]}});
        latest["use_siwc"] = json!(true);
        latest["allow_codex_client_siwc"] = json!(false);
        latest["codex_cli"]["future"] = json!({"keep":true});
        latest["disabled"] = json!(case == "disabled");
        let replacement = directory.join("replacement.tmp");
        std::fs::write(&replacement, serde_json::to_vec(&latest)?)?;
        std::fs::rename(&replacement, &path)?;
        if case == "removed" {
            std::fs::rename(&path, directory.join("renamed.json"))?;
        }
        drop(lock);
        let output =
            tokio::time::timeout(Duration::from_secs(15), child.wait_with_output()).await??;
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        if case == "removed" {
            assert!(!path.exists());
        } else {
            let saved: Value = serde_json::from_slice(&std::fs::read(&path)?)?;
            if case == "disabled" {
                assert_eq!(saved, latest);
            } else {
                for key in [
                    "siwc",
                    "use_siwc",
                    "allow_codex_client_siwc",
                    "codex_cli",
                    "disabled",
                ] {
                    assert_eq!(&saved[key], &latest[key]);
                }
                assert_eq!(saved["access_token"], "rotated-normal-token");
                assert_eq!(saved["refresh_token"], "rotated-normal-refresh");
                #[cfg(unix)]
                assert!(
                    std::fs::symlink_metadata(&child_path)?
                        .file_type()
                        .is_symlink()
                );
            }
        }
        // Atomic replacement must not replace/remove the shared sidecar inode.
        let sidecar = directory.join("Account Name.json.lock");
        let contender = OpenOptions::new().read(true).write(true).open(&sidecar)?;
        contender.try_lock()?;
        contender.unlock()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(sidecar)?.permissions().mode() & 0o777,
                0o600
            );
        }
    }
    Ok(())
}
