//! Cloud boxes through the engine's RPC surface (docs/cloud.md §7): connect a
//! Cloudflare token, provision against a fake Cloudflare + fake edge, wake,
//! stop and destroy — with the token kept in a file store.

use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use zeron_cloud::mock::{FakeCloudflare, FakeState, MockServer, Recorded, Reply};
use zeron_engine::cloud_boxes::{CloudSecrets, CloudTokenStore};
use zeron_engine::{EdgeConfig, EngineCore, HarnessRegistry};
use zeron_proto::{CloudBox, CloudBoxState, HarnessId};
use zeron_rpc::{TokenError, TokenSource, methods};

struct Bearer;

#[async_trait::async_trait]
impl TokenSource for Bearer {
    async fn token(&self) -> Result<String, TokenError> {
        Ok("user-bearer".into())
    }
}

/// A fake edge answering the enrolment routes.
async fn edge() -> (MockServer, Arc<Mutex<u32>>) {
    let minted = Arc::new(Mutex::new(0u32));
    let count = minted.clone();
    let server = MockServer::start(move |r: &Recorded| {
        match (r.method.as_str(), r.path.as_str()) {
            ("POST", "/cloud/devices") => {
                let mut n = count.lock().unwrap();
                *n += 1;
                Reply::json(
                    200,
                    json!({ "deviceId": format!("box-{n}"), "credential": format!("cred-{n}") }),
                )
            }
            ("DELETE", path) if path.starts_with("/cloud/devices/") => Reply::json(200, json!({})),
            _ => Reply::json(404, json!({})),
        }
    })
    .await;
    (server, minted)
}

fn engine(dir: &std::path::Path) -> EngineCore {
    EngineCore::assemble(dir, Arc::new(HarnessRegistry::new()), HarnessId::Mock, None)
        .expect("engine assembles")
}

#[tokio::test]
async fn connect_provision_wake_stop_destroy_over_rpc() {
    let dir = tempfile::tempdir().unwrap();
    let core = engine(dir.path());
    let fake = FakeCloudflare::new(FakeState {
        bucket_objects: 2,
        ..FakeState::default()
    });
    let cloudflare = fake.serve().await;
    let store_file = dir.path().join("cloud-credentials.json");
    core.cloud
        .set_token_store(CloudTokenStore::file(&store_file));
    core.cloud
        .set_api_base(&format!("{}/client/v4", cloudflare.url));
    let client = zeron_rpc::memory_client(core.rpc_service());

    // Nothing yet.
    let listed = client.call(methods::CLOUD_BOXES, json!({})).await.unwrap();
    assert_eq!(listed, json!({ "boxes": [], "connected": false }));

    // A bad token is refused and not kept.
    let err = client
        .call(methods::CLOUD_CONNECT, json!({ "apiToken": "nope" }))
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("didn't accept this API token"),
        "{err}"
    );
    assert!(!store_file.exists());

    let connected = client
        .call(
            methods::CLOUD_CONNECT,
            json!({ "apiToken": " good-token " }),
        )
        .await
        .unwrap();
    assert_eq!(
        connected,
        json!({ "accountId": "acc-1", "accountName": "Dana's account" })
    );
    let listed = client.call(methods::CLOUD_BOXES, json!({})).await.unwrap();
    assert_eq!(listed["connected"], true);
    assert_eq!(listed["accountId"], "acc-1");

    // Without an edge there's no account for the box to join.
    let provision = json!({ "name": "Cloud", "instanceType": "standard-2", "idleMinutes": 15 });
    let err = client
        .call(methods::CLOUD_PROVISION, provision.clone())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("Sign in to Zeron"), "{err}");

    let (edge, minted) = edge().await;
    core.cloud.set_edge(Some(EdgeConfig {
        url: edge.url.clone(),
        token: Arc::new(Bearer),
        device_id: String::new(),
    }));

    // A first attempt fails late (no workers.dev permission yet); the retry
    // reuses the box already enrolled with the edge.
    fake.state()
        .forbidden
        .push("/accounts/acc-1/workers/subdomain".into());
    let err = client
        .call(methods::CLOUD_PROVISION, provision.clone())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("Workers Scripts: Edit"), "{err}");
    let secrets: CloudSecrets =
        serde_json::from_slice(&std::fs::read(&store_file).unwrap()).unwrap();
    assert_eq!(
        secrets.pending.as_ref().unwrap().enrollment.device_id,
        "box-1"
    );
    fake.state().forbidden.clear();

    let created: CloudBox = serde_json::from_value(
        client
            .call(methods::CLOUD_PROVISION, provision.clone())
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(*minted.lock().unwrap(), 1, "enrolled once across the retry");
    assert_eq!(created.id, "box-1");
    assert_eq!(created.state, CloudBoxState::Asleep);
    let enrol = edge
        .requests()
        .into_iter()
        .find(|r| r.path == "/cloud/devices")
        .unwrap();
    assert_eq!(enrol.header("authorization"), Some("Bearer user-bearer"));
    assert_eq!(enrol.json(), json!({ "name": "Cloud" }));

    // The secret file keeps the credential; the record is in the registry.
    let secrets: CloudSecrets =
        serde_json::from_slice(&std::fs::read(&store_file).unwrap()).unwrap();
    assert_eq!(
        secrets.credentials.get("box-1").map(String::as_str),
        Some("cred-1")
    );
    assert!(secrets.pending.is_none());
    let worker_secret: Value = serde_json::from_str(&fake.state().secrets["ZERON_BOXES"]).unwrap();
    assert_eq!(worker_secret["box-1"]["credential"], "cred-1");
    assert_eq!(worker_secret["box-1"]["wakeKey"], created.wake_key);
    let listed = client.call(methods::CLOUD_BOXES, json!({})).await.unwrap();
    assert_eq!(listed["boxes"][0]["id"], "box-1");

    // Serve the Worker locally instead of workers.dev.
    core.workspace
        .upsert_cloud_box(&CloudBox {
            worker_url: cloudflare.url.clone(),
            ..created.clone()
        })
        .unwrap();
    let woken = client
        .call(methods::CLOUD_WAKE, json!({ "deviceId": "box-1" }))
        .await
        .unwrap();
    assert_eq!(woken["state"], "waking");
    let stopped = client
        .call(methods::CLOUD_STOP, json!({ "deviceId": "box-1" }))
        .await
        .unwrap();
    assert_eq!(stopped["state"], "checkpointing");
    let err = client
        .call(methods::CLOUD_WAKE, json!({ "deviceId": "box-9" }))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("No such cloud box"));

    // Destroy: data purged, deployment removed, device revoked, record gone.
    client
        .call(
            methods::CLOUD_DESTROY,
            json!({ "deviceId": "box-1", "keepData": false }),
        )
        .await
        .unwrap();
    {
        let state = fake.state();
        assert!(state.apps.is_empty());
        assert!(state.script_tag.is_none());
        assert!(!state.bucket);
        assert_eq!(state.bucket_objects, 0);
    }
    assert!(
        edge.requests()
            .iter()
            .any(|r| r.method == "DELETE" && r.path == "/cloud/devices/box-1")
    );
    let listed = client.call(methods::CLOUD_BOXES, json!({})).await.unwrap();
    assert_eq!(listed["boxes"], json!([]));
    let secrets: CloudSecrets =
        serde_json::from_slice(&std::fs::read(&store_file).unwrap()).unwrap();
    assert!(secrets.credentials.is_empty());
}

#[tokio::test]
async fn move_candidates_offer_an_asleep_box() {
    let dir = tempfile::tempdir().unwrap();
    let core = engine(dir.path());
    core.workspace
        .create_chat(
            "chat-1",
            None,
            Some(&core.device_id),
            None,
            Some(dir.path().to_string_lossy().into_owned()),
        )
        .unwrap();
    core.workspace
        .upsert_cloud_box(&CloudBox {
            id: "box-1".into(),
            provider: "cloudflare".into(),
            name: "Cloud".into(),
            account_id: "acc-1".into(),
            worker_url: "http://127.0.0.1:9".into(),
            wake_key: zeron_cloud::sign::new_wake_key(),
            instance_type: "standard-2".into(),
            idle_minutes: 15,
            created_at: 1,
            state: CloudBoxState::Asleep,
            state_at: 1,
            error: None,
        })
        .unwrap();
    let client = zeron_rpc::memory_client(core.rpc_service());
    let candidates = client
        .call(methods::MOVE_CANDIDATES, json!({ "chatId": "chat-1" }))
        .await
        .unwrap();
    assert_eq!(
        candidates,
        json!([{
            "deviceId": "box-1", "deviceName": "Cloud", "online": false, "supported": true,
            "harnessInstalled": true, "asleep": true, "note": "Asleep — wakes when you move",
        }])
    );
}
