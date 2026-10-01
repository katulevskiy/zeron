//! Cloud-box enrollment on a local edge (docs/cloud.md §6): the routes and
//! token checks mirror the Worker's, and a real engine started with only a
//! device credential signs in, pins its device id, and hosts its device room
//! next to a desktop engine.

use std::time::{Duration, Instant};

use futures::StreamExt;
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;
use zeron_engine::{DeviceCredential, Engine, EngineConfig, HarnessId, WorkspaceScope};
use zeron_localedge::{LocalEdge, LocalEdgeConfig};

const TOKEN: &str = "enroll-0123456789abcdef0123456789abcdef";

async fn start(dir: &std::path::Path) -> LocalEdge {
    LocalEdge::start(LocalEdgeConfig::loopback(dir, 0, TOKEN))
        .await
        .expect("local edge starts")
}

async fn post(edge: &LocalEdge, path: &str, bearer: Option<&str>, body: Value) -> (u16, Value) {
    let mut request = reqwest::Client::new()
        .post(format!("{}{path}", edge.url()))
        .json(&body);
    if let Some(bearer) = bearer {
        request = request.bearer_auth(bearer);
    }
    let response = request.send().await.expect("edge answers");
    let status = response.status().as_u16();
    (status, response.json().await.unwrap_or(Value::Null))
}

async fn send(edge: &LocalEdge, method: reqwest::Method, path: &str, bearer: &str) -> u16 {
    reqwest::Client::new()
        .request(method, format!("{}{path}", edge.url()))
        .bearer_auth(bearer)
        .send()
        .await
        .expect("edge answers")
        .status()
        .as_u16()
}

async fn enroll(edge: &LocalEdge) -> (String, String) {
    let (status, body) = post(edge, "/cloud/devices", Some(TOKEN), json!({ "name": "Box" })).await;
    assert_eq!(status, 200, "{body}");
    (
        body["deviceId"].as_str().unwrap().to_owned(),
        body["credential"].as_str().unwrap().to_owned(),
    )
}

async fn exchange(edge: &LocalEdge, device: &str, credential: &str) -> (u16, Value) {
    post(
        edge,
        "/auth/device-token",
        None,
        json!({ "deviceId": device, "credential": credential }),
    )
    .await
}

fn claims(token: &str) -> Value {
    use base64::Engine as _;
    let payload = token.split('.').nth(1).unwrap();
    serde_json::from_slice(
        &base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(payload)
            .unwrap(),
    )
    .unwrap()
}

#[tokio::test]
async fn enrollment_routes_and_device_tokens_mirror_the_worker() {
    let dir = tempfile::tempdir().unwrap();
    let edge = start(dir.path()).await;

    assert_eq!(post(&edge, "/cloud/devices", None, json!({})).await.0, 401);
    let (device, credential) = enroll(&edge).await;
    assert_eq!(credential.len(), 43);

    let (status, issued) = exchange(&edge, &device, &credential).await;
    assert_eq!(status, 200, "{issued}");
    let token = issued["token"].as_str().unwrap().to_owned();
    let claims = claims(&token);
    assert_eq!(claims["iss"], "zeron-edge");
    assert_eq!(claims["sub"], "local");
    assert_eq!(claims["org_id"], "local");
    assert_eq!(claims["did"], device.as_str());
    assert_eq!(claims["kind"], "cloud");
    assert_eq!(
        claims["exp"].as_i64().unwrap() - claims["iat"].as_i64().unwrap(),
        30 * 60
    );
    assert_eq!(issued["expiresAt"], claims["exp"].as_i64().unwrap() * 1000);

    let jwks: Value = reqwest::get(format!("{}/.well-known/zeron-device-jwks.json", edge.url()))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(jwks, json!({ "keys": [] }));

    // Wrong and unknown credentials.
    let (status, body) = exchange(&edge, &device, "wrong").await;
    assert_eq!((status, body["error"].as_str()), (401, Some("invalid_credential")));
    assert_eq!(exchange(&edge, "nobody", &credential).await.0, 401);
    assert_eq!(exchange(&edge, "../x", &credential).await.0, 400);

    // Scope: the box's sync surface, nothing else.
    use reqwest::Method;
    assert_eq!(send(&edge, Method::GET, "/registry/local/rows", &token).await, 200);
    assert_eq!(send(&edge, Method::GET, "/registry/local/stats", &token).await, 200);
    assert_eq!(send(&edge, Method::POST, "/registry/local/reset", &token).await, 403);
    assert_eq!(send(&edge, Method::POST, "/chat2/chat-1/reset", &token).await, 403);
    assert_eq!(send(&edge, Method::POST, "/cloud/devices", &token).await, 403);
    assert_eq!(
        send(&edge, Method::DELETE, &format!("/cloud/devices/{device}"), &token).await,
        403
    );
    assert_eq!(send(&edge, Method::GET, "/auth/orgs", &token).await, 403);
    assert_eq!(
        send(&edge, Method::POST, "/device/other/sidecar/repos", &token).await,
        403
    );
    // A forged token naming our issuer is never mistaken for the secret.
    let forged = format!("{}.{}x", token.rsplit_once('.').unwrap().0, "");
    assert_eq!(send(&edge, Method::GET, "/registry/local/rows", &forged).await, 401);

    // Host its own room only.
    let ws = |path: &str, bearer: &str| {
        format!(
            "ws://{}{path}{}token={bearer}",
            edge.addr(),
            if path.contains('?') { '&' } else { '?' }
        )
    };
    assert!(
        tokio_tungstenite::connect_async(ws("/device/other/ws?role=host", &token))
            .await
            .is_err()
    );
    let (mut host, _) =
        tokio_tungstenite::connect_async(ws(&format!("/device/{device}/ws?role=host"), &token))
            .await
            .expect("the box hosts its own room");

    // Revocation: owner only, closes the live host, refuses new tokens, and
    // (unlike the Worker, where tokens expire within 30 minutes) at once
    // stops the token already issued.
    assert_eq!(
        send(&edge, Method::DELETE, "/cloud/devices/nobody", TOKEN).await,
        404
    );
    assert_eq!(
        send(&edge, Method::DELETE, &format!("/cloud/devices/{device}"), TOKEN).await,
        200
    );
    let closed = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match host.next().await {
                Some(Ok(Message::Close(frame))) => return frame.map(|f| u16::from(f.code)),
                Some(Ok(_)) => continue,
                _ => return None,
            }
        }
    })
    .await
    .expect("host closed");
    assert_eq!(closed, Some(4403));
    let (status, body) = exchange(&edge, &device, &credential).await;
    assert_eq!((status, body["error"].as_str()), (401, Some("revoked")));
    assert_eq!(send(&edge, Method::GET, "/registry/local/rows", &token).await, 401);

    // Per-device failure limit.
    let (other, other_credential) = enroll(&edge).await;
    for i in 0..10 {
        assert_eq!(exchange(&edge, &other, &format!("wrong-{i}")).await.0, 401);
    }
    let (status, body) = exchange(&edge, &other, &other_credential).await;
    assert_eq!((status, body["error"].as_str()), (429, Some("rate_limited")));
}

async fn wait_for(what: &str, mut probe: impl AsyncFnMut() -> bool) {
    let start = Instant::now();
    while !probe().await {
        assert!(
            start.elapsed() < Duration::from_secs(30),
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn config(dir: std::path::PathBuf) -> EngineConfig {
    EngineConfig {
        data_dir: dir,
        edge_url: String::new(),
        edge_token: None,
        ipc_port: 0,
        default_harness: HarnessId::Mock,
        org_id: None,
        // The production default: device mode must ignore it.
        workos_client_id: Some("client_unused".into()),
        dev_user_id: None,
        device_credential: None,
    }
}

#[tokio::test]
async fn a_box_engine_enrolls_and_joins_next_to_a_desktop() {
    let dir = tempfile::tempdir().unwrap();
    let edge = start(&dir.path().join("edge")).await;

    // The desktop: the shared-secret owner.
    let desktop_config = config(dir.path().join("desktop")).with_local_edge(edge.url(), TOKEN);
    let desktop_auth = Engine::build_auth(&desktop_config).await;
    let scope = Engine::initial_workspace_scope(&desktop_auth);
    let desktop_profile = Engine::resolve_profile(&desktop_config, &desktop_auth, scope)
        .unwrap()
        .unwrap();
    let desktop = Engine::assemble_runtime(&desktop_config, desktop_auth, desktop_profile)
        .await
        .unwrap();

    // The box: nothing but the edge URL and its credential. A stale
    // installation id in its data dir gives way to the enrolled one.
    let (device, credential) = enroll(&edge).await;
    let box_dir = dir.path().join("box");
    std::fs::create_dir_all(&box_dir).unwrap();
    std::fs::write(box_dir.join("device-id"), "stale-installation-id").unwrap();
    let mut box_config = config(box_dir.clone())
        .with_device_credential(DeviceCredential::new(&device, &credential).unwrap());
    box_config.edge_url = edge.url();
    let box_auth = Engine::build_auth(&box_config).await;
    assert!(box_auth.device_mode());
    tokio::time::timeout(Duration::from_secs(15), box_auth.wait_for_device_identity())
        .await
        .expect("box signs in");
    let scope = Engine::initial_workspace_scope(&box_auth);
    assert_eq!(scope, WorkspaceScope::Synced);
    let box_profile = Engine::resolve_profile(&box_config, &box_auth, scope)
        .unwrap()
        .expect("the owner's synced profile");
    assert_eq!((box_profile.org_id(), box_profile.user_id()), ("local", "local"));
    let _refresh = box_auth.spawn_refresh_loop();
    let cloud = Engine::assemble_runtime(&box_config, box_auth, box_profile)
        .await
        .unwrap();
    assert_eq!(cloud.core().device_id, device);
    assert_eq!(
        std::fs::read_to_string(box_dir.join("device-id")).unwrap(),
        device
    );

    // The box hosts its device room with its device token...
    wait_for("the box to host its device room", async || {
        let status: Value = reqwest::Client::new()
            .get(format!("{}/device/{device}/status", edge.url()))
            .bearer_auth(TOKEN)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap_or(Value::Null);
        status["hostConnected"] == true
    })
    .await;
    // ...and the desktop sees it in the shared registry.
    wait_for("the desktop to see the box", async || {
        desktop
            .core()
            .workspace
            .read_devices()
            .unwrap_or_default()
            .iter()
            .any(|d| d.id == device)
    })
    .await;

    cloud.shutdown().await;
    desktop.shutdown().await;
}
