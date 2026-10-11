//! End-to-end: a local HTTP capture server stands in for PostHog.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use super::*;

/// Minimal HTTP/1.1 server: records decompressed `/batch/` bodies and
/// answers with the next queued status (default 200).
async fn capture_server(statuses: Vec<u16>) -> (String, Arc<Mutex<Vec<Value>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let statuses = Arc::new(Mutex::new(statuses));
    let sink = bodies.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            let sink = sink.clone();
            let statuses = statuses.clone();
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 8192];
                let (head_end, len) = loop {
                    let n = sock.read(&mut chunk).await.unwrap();
                    if n == 0 {
                        return;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&buf[..pos]).to_ascii_lowercase();
                        let len = head
                            .lines()
                            .find_map(|l| l.strip_prefix("content-length:"))
                            .map(|v| v.trim().parse::<usize>().unwrap())
                            .unwrap_or(0);
                        break (pos + 4, len);
                    }
                };
                while buf.len() < head_end + len {
                    let n = sock.read(&mut chunk).await.unwrap();
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                }
                let status = {
                    let mut s = statuses.lock().unwrap();
                    if s.is_empty() { 200 } else { s.remove(0) }
                };
                if status == 200 {
                    let mut gz = flate2::read::GzDecoder::new(&buf[head_end..head_end + len]);
                    let mut raw = Vec::new();
                    std::io::Read::read_to_end(&mut gz, &mut raw).unwrap();
                    sink.lock()
                        .unwrap()
                        .push(serde_json::from_slice(&raw).unwrap());
                }
                let resp = format!(
                    "HTTP/1.1 {status} X\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{{}}"
                );
                let _ = sock.write_all(resp.as_bytes()).await;
            });
        }
    });
    (format!("http://{addr}"), bodies)
}

fn opts(dir: &std::path::Path) -> TelemetryOptions {
    TelemetryOptions {
        data_dir: dir.to_path_buf(),
        run_mode: RunMode::Headless,
        workspace_scope: "local".into(),
        device_id: "device-1".into(),
        app_version: "9.9.9".into(),
    }
}

fn tuning(endpoint: Option<String>) -> Tuning {
    Tuning {
        endpoint,
        api_key: "phc_test".into(),
        batch_size: 100,
        flush_every: Duration::from_secs(3600),
        rollup_every: Duration::from_secs(3600),
        debug_log: false,
    }
}

fn events_of(bodies: &[Value]) -> Vec<Value> {
    bodies
        .iter()
        .flat_map(|b| b["batch"].as_array().unwrap().clone())
        .collect()
}

#[tokio::test]
async fn batches_are_gzipped_stamped_and_flushed() {
    let dir = tempfile::tempdir().unwrap();
    let (url, bodies) = capture_server(vec![]).await;
    let t = Telemetry::start_with(opts(dir.path()), tuning(Some(url)), None);
    t.capture(Event::new(names::APP_STARTED).with("cold_start_ms", 12u64));
    t.record("rpc", "ListChats", Some(Duration::from_millis(3)), true);
    t.flush(Duration::from_secs(5)).await;

    let bodies = bodies.lock().unwrap().clone();
    assert_eq!(bodies[0]["api_key"], "phc_test");
    let events = events_of(&bodies);
    let started = events.iter().find(|e| e["event"] == "app.started").unwrap();
    let p = &started["properties"];
    assert_eq!(p["app_version"], "9.9.9");
    assert_eq!(p["run_mode"], "headless");
    assert_eq!(p["$process_person_profile"], false);
    assert_eq!(p["cold_start_ms"], 12);
    assert_ne!(p["device_hash"], "device-1", "device id is hashed");
    assert_eq!(started["distinct_id"].as_str().unwrap().len(), 32);
    assert!(uuid::Uuid::parse_str(started["uuid"].as_str().unwrap()).is_ok());
    let rollup = events.iter().find(|e| e["event"] == "rollup").unwrap();
    assert_eq!(rollup["properties"]["calls"]["ListChats"], 1);
    assert_eq!(t.status().sent, events.len() as u64);
}

#[tokio::test]
async fn failed_sends_spool_and_replay_with_same_uuids() {
    let dir = tempfile::tempdir().unwrap();
    let (url, bodies) = capture_server(vec![503]).await;
    let t = Telemetry::start_with(opts(dir.path()), tuning(Some(url.clone())), None);
    t.capture(Event::new(names::APP_STARTED));
    t.flush(Duration::from_secs(5)).await;
    assert!(bodies.lock().unwrap().is_empty());
    assert!(dir.path().join("telemetry/spool.jsonl").exists());
    assert!(t.status().last_error.unwrap().contains("503"));

    // Next launch drains the spool.
    let first_uuid = {
        let line = std::fs::read_to_string(dir.path().join("telemetry/spool.jsonl")).unwrap();
        let batch: Value = serde_json::from_str(line.lines().next().unwrap()).unwrap();
        batch[0]["uuid"].clone()
    };
    drop(t);
    let t2 = Telemetry::start_with(opts(dir.path()), tuning(Some(url)), None);
    t2.flush(Duration::from_secs(5)).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    let events = events_of(&bodies.lock().unwrap());
    assert!(
        events.iter().any(|e| e["uuid"] == first_uuid),
        "spooled event replayed"
    );
    assert!(!dir.path().join("telemetry/spool.jsonl").exists());
}

#[tokio::test]
async fn opting_out_stops_and_purges() {
    let dir = tempfile::tempdir().unwrap();
    let (url, bodies) = capture_server(vec![]).await;
    let t = Telemetry::start_with(opts(dir.path()), tuning(Some(url.clone())), None);
    t.capture(Event::new(names::APP_STARTED));
    t.update_settings(TelemetrySettingsPatch {
        enabled: Some(false),
        ..Default::default()
    });
    t.capture(Event::new(names::APP_STOPPED));
    t.count("rpc", "x");
    t.flush(Duration::from_secs(5)).await;
    assert!(
        events_of(&bodies.lock().unwrap()).is_empty(),
        "nothing after opt-out"
    );
    assert!(t.status().recent.is_empty());
    assert!(!read_settings(dir.path()).enabled, "persisted");

    // A restart honours the persisted opt-out.
    let t2 = Telemetry::start_with(opts(dir.path()), tuning(Some(url)), None);
    assert!(!t2.is_collecting());
}

#[tokio::test]
async fn env_override_wins_over_settings() {
    let dir = tempfile::tempdir().unwrap();
    let t = Telemetry::start_with(opts(dir.path()), tuning(None), Some("DO_NOT_TRACK"));
    assert!(!t.is_collecting());
    t.update_settings(TelemetrySettingsPatch {
        enabled: Some(true),
        ..Default::default()
    });
    assert!(!t.is_collecting());
    assert_eq!(
        t.status().override_reason.as_deref(),
        Some("disabled by DO_NOT_TRACK")
    );
}

#[tokio::test]
async fn without_endpoint_events_are_recorded_locally_only() {
    let dir = tempfile::tempdir().unwrap();
    let t = Telemetry::start_with(opts(dir.path()), tuning(None), None);
    t.capture(Event::new(names::APP_STARTED));
    t.flush(Duration::from_secs(5)).await;
    let status = t.status();
    assert!(status.collecting);
    assert!(!status.uploading);
    assert_eq!(status.recent.len(), 1);
    assert_eq!(status.sent, 0);
}

#[tokio::test]
async fn account_link_changes_attribution() {
    let dir = tempfile::tempdir().unwrap();
    let t = Telemetry::start_with(opts(dir.path()), tuning(None), None);
    t.capture(Event::new(names::APP_STARTED));
    t.set_account(Some("user_01"));
    t.capture(Event::new(names::AUTH_CHANGED));
    t.flush(Duration::from_secs(5)).await;
    let recent = t.status().recent;
    assert_ne!(recent[0]["distinct_id"], recent[1]["distinct_id"]);
    assert_eq!(recent[1]["properties"]["account_linked"], true);
    assert_eq!(
        recent[0]["properties"]["install_hash"],
        recent[1]["properties"]["install_hash"]
    );
}

#[test]
fn disabled_handle_is_inert() {
    let t = Telemetry::disabled();
    t.capture(Event::new(names::APP_STARTED));
    t.count("rpc", "x");
    assert!(!t.is_collecting());
    assert!(t.status().recent.is_empty());
}
