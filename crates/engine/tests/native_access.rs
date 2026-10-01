//! The daemon keeps localhost IPC but never opens the retired direct HTTP listener.
use std::{
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::Duration,
};
use zeron_engine::{Engine, EngineConfig, HarnessId};
use zeron_rpc::{connect_ws, methods};

struct Daemon(Child);
impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn legacy_direct_settings_do_not_open_a_listener_and_ipc_still_works() {
    // Run the actual daemon in an isolated child so its legacy environment
    // overrides cannot race other tests or alter the developer's environment.
    if let Some(root) = std::env::var_os("ZERON_TEST_DAEMON_ROOT") {
        Engine::new(EngineConfig {
            data_dir: PathBuf::from(root),
            edge_url: "http://127.0.0.1:1".into(),
            edge_token: None,
            ipc_port: std::env::var("ZERON_TEST_IPC_PORT")
                .unwrap()
                .parse()
                .unwrap(),
            default_harness: HarnessId::Mock,
            org_id: None,
            workos_client_id: Some("client_native_access_test".into()),
        })
        .run()
        .await
        .unwrap();
        return;
    }
    let root = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).unwrap();
    let ipc_reservation = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let ipc_port = ipc_reservation.local_addr().unwrap().port();
    let direct_reservation = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let direct_address = direct_reservation.local_addr().unwrap();
    std::fs::write(
        root.path().join("remote-access.json"),
        serde_json::json!({
            "enabled": true, "bindAddress": direct_address.to_string()
        })
        .to_string(),
    )
    .unwrap();
    drop(ipc_reservation);
    drop(direct_reservation);
    let log_path = root.path().join("daemon.log");
    let log = std::fs::File::create(&log_path).unwrap();
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "legacy_direct_settings_do_not_open_a_listener_and_ipc_still_works",
            "--nocapture",
        ])
        .env("ZERON_TEST_DAEMON_ROOT", root.path())
        .env("ZERON_TEST_IPC_PORT", ipc_port.to_string())
        .env("ZERON_NETWORK", "true")
        .env_remove("ZERON_EDGE_TOKEN")
        .stdin(Stdio::null())
        .stdout(log.try_clone().unwrap())
        .stderr(log);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: setsid is async-signal-safe. Only detach this owned child
        // from the runner's controlling terminal before exec; no user process
        // or existing session is modified.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    let mut daemon = Daemon(command.spawn().unwrap());
    let rpc = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            assert!(
                daemon.0.try_wait().unwrap().is_none(),
                "daemon exited before IPC readiness: {}",
                std::fs::read_to_string(&log_path).unwrap()
            );
            if let Ok(rpc) = connect_ws(&format!("ws://127.0.0.1:{ipc_port}")).await {
                break rpc;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap_or_else(|err| {
        panic!(
            "daemon IPC startup timed out: {err}; {}",
            std::fs::read_to_string(&log_path).unwrap()
        )
    });
    let info = rpc
        .call(methods::ENGINE_INFO, serde_json::json!({}))
        .await
        .unwrap();
    assert_eq!(info["workspaceScope"], "local");
    assert!(!info["deviceId"].as_str().unwrap().is_empty());
    let capabilities = info["capabilities"].as_array().unwrap();
    assert!(capabilities.iter().all(|value| value != "web-client"));
    assert!(capabilities.iter().any(|value| value == "message-queue-v1"));
    // Allow startup after the first IPC reply; prove the old enabled bind is
    // available while the daemon remains live, not merely after its teardown.
    tokio::time::sleep(Duration::from_millis(200)).await;
    let unused_direct_port = std::net::TcpListener::bind(direct_address)
        .expect("legacy settings/environment must not open a direct engine listener");
    assert!(daemon.0.try_wait().unwrap().is_none());
    assert_eq!(
        rpc.call(methods::STOP_ENGINE, serde_json::json!({}))
            .await
            .unwrap(),
        serde_json::json!({"ok": true})
    );
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some(status) = daemon.0.try_wait().unwrap() {
                assert!(status.success(), "daemon shutdown failed: {status}");
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("IPC shutdown did not stop the daemon");
    drop(unused_direct_port);
    assert!(
        std::fs::read_to_string(root.path().join("remote-access.json"))
            .unwrap()
            .contains("bindAddress"),
        "retirement must not delete a user's saved settings"
    );
}
