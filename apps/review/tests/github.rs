use axum::{
    Router,
    body::Bytes,
    extract::State,
    http::{Method, StatusCode, Uri},
    response::IntoResponse,
    routing::any,
};
use hmac::{Hmac, Mac};
use serde_json::{Value, json};
use sha2::Sha256;
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::sync::{Mutex, Notify};
use zeron_review::{
    config::Config,
    domain,
    github::{GitHub, verify_webhook},
    seed,
    server::{App, router},
    store::Store,
};

struct Stub {
    raw: Mutex<Value>,
    reviews: Mutex<Vec<Value>>,
    run: Mutex<Value>,
    labels: Mutex<Vec<Value>>,
    calls: Mutex<Vec<(String, Method, Value)>>,
    pause: AtomicBool,
    entered: Notify,
    resume: Notify,
}
async fn handler(
    State(stub): State<Arc<Stub>>,
    method: Method,
    uri: Uri,
    bytes: Bytes,
) -> axum::response::Response {
    let path = uri.path();
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    stub.calls
        .lock()
        .await
        .push((path.into(), method.clone(), body.clone()));
    if path.ends_with("/reviews") {
        if stub.pause.load(Ordering::Relaxed) {
            stub.entered.notify_one();
            stub.resume.notified().await;
        }
        return axum::Json(json!(stub.reviews.lock().await.clone())).into_response();
    }
    if path.ends_with("/pulls/1") {
        return axum::Json(stub.raw.lock().await.clone()).into_response();
    }
    if path.contains("/commits/") && path.ends_with("/check-runs") {
        return axum::Json(json!({"check_runs":[stub.run.lock().await.clone()]})).into_response();
    }
    if path.ends_with("/issues/1/labels") {
        if method == Method::POST {
            *stub.labels.lock().await =
                vec![json!({"name":"bug"}), json!({"name":body["labels"][0]})];
        }
        return axum::Json(json!(stub.labels.lock().await.clone())).into_response();
    }
    if method == Method::DELETE {
        return StatusCode::NO_CONTENT.into_response();
    }
    axum::Json(json!({"id":10})).into_response()
}
async fn fixture() -> (Arc<Stub>, GitHub, Store, tokio::task::JoinHandle<()>) {
    let stub = Arc::new(Stub {
        raw: Mutex::new(
            json!({"number":1,"title":"Fix","user":{"login":"author"},"html_url":"https://github.com/zeronsh/zeron/pull/1","created_at":"2026-10-01T00:00:00Z","updated_at":"2026-10-05T00:00:00Z","state":"open","labels":[],"head":{"sha":"a".repeat(40)},"base":{"sha":"b".repeat(40)},"draft":false,"merged_at":null}),
        ),
        reviews: Mutex::new(vec![]),
        run: Mutex::new(
            json!({"id":1,"conclusion":"success","completed_at":"2026-10-05T00:00:00Z","html_url":"https://github.com/check/1"}),
        ),
        labels: Mutex::new(vec![
            json!({"name":"bug"}),
            json!({"name":"review:old-stage"}),
        ]),
        calls: Mutex::new(vec![]),
        pause: AtomicBool::new(false),
        entered: Notify::new(),
        resume: Notify::new(),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let service = Router::new()
        .route("/{*path}", any(handler))
        .with_state(stub.clone());
    let task = tokio::spawn(async move {
        axum::serve(listener, service).await.unwrap();
    });
    let mut config = Config::load(true).unwrap();
    config.api_url = format!("http://{address}");
    config.read_token.clear();
    config.app_id.clear();
    config.installation_id.clear();
    config.private_key_path.clear();
    (
        stub,
        GitHub::new(Arc::new(config)).unwrap(),
        Store::open(Path::new(":memory:")).unwrap(),
        task,
    )
}
#[test]
fn webhook_signatures_authenticate_exact_raw_bytes() {
    let bytes = b"{\"x\":1}";
    let mut mac = Hmac::<Sha256>::new_from_slice(b"secret").unwrap();
    mac.update(bytes);
    let sig = format!("sha256={}", hex::encode(mac.finalize().into_bytes()));
    assert!(verify_webhook(bytes, &sig, "secret"));
    assert!(!verify_webhook(b"{\"x\":2}", &sig, "secret"));
    assert!(!verify_webhook(bytes, "bad", "secret"));
    assert!(!verify_webhook(bytes, &sig, ""));
}
#[tokio::test]
async fn a_review_submitted_during_github_fetch_is_preserved() {
    let (stub, github, store, server) = fixture().await;
    github.sync_item(&store, 1, "pr").await.unwrap();
    stub.pause.store(true, Ordering::Relaxed);
    let syncing = {
        let github = github.clone();
        let store = store.clone();
        tokio::spawn(async move { github.sync_item(&store, 1, "pr").await.unwrap() })
    };
    stub.entered.notified().await;
    store
        .write(|db| {
            domain::mutate(
                db,
                "pr:1",
                "demo-reviewer",
                "review",
                &json!({"revision":"a".repeat(40),"verdict":"pass","summary":"Concurrent review"}),
                &seed::policy(),
            )
        })
        .await
        .unwrap();
    stub.resume.notify_one();
    let synced = syncing.await.unwrap();
    assert_eq!(synced["reviews"][0]["summary"], "Concurrent review");
    server.abort();
}
#[tokio::test]
async fn base_changes_invalidate_old_checks_until_a_new_run() {
    let (stub, github, store, server) = fixture().await;
    github.sync_item(&store, 1, "pr").await.unwrap();
    stub.raw.lock().await["base"]["sha"] = "c".repeat(40).into();
    let changed = github.sync_item(&store, 1, "pr").await.unwrap();
    assert_eq!(changed["checks"][0]["baseRevision"], "b".repeat(40));
    assert!(
        domain::assess(&changed, &seed::policy())
            .blockers
            .iter()
            .any(|b| b.starts_with("Required check"))
    );
    stub.run.lock().await["id"] = 2.into();
    let fresh = github.sync_item(&store, 1, "pr").await.unwrap();
    assert_eq!(fresh["checks"][0]["baseRevision"], "c".repeat(40));
    server.abort();
}
#[tokio::test]
async fn dismissed_native_reviews_stop_counting() {
    let (stub, github, store, server) = fixture().await;
    *stub.reviews.lock().await = vec![
        json!({"id":5,"user":{"login":"demo-reviewer"},"state":"APPROVED","commit_id":"a".repeat(40),"body":"Native approval","submitted_at":"2026-10-05T00:00:00Z"}),
    ];
    let approved = github.sync_item(&store, 1, "pr").await.unwrap();
    assert!(
        !domain::assess(&approved, &seed::policy())
            .blockers
            .contains(&"Independent code review needed".into())
    );
    stub.reviews.lock().await[0]["state"] = "DISMISSED".into();
    let dismissed = github.sync_item(&store, 1, "pr").await.unwrap();
    assert!(
        domain::assess(&dismissed, &seed::policy())
            .blockers
            .contains(&"Independent code review needed".into())
    );
    server.abort();
}
#[tokio::test]
async fn writeback_preserves_other_labels_and_never_merges_or_spams_checks() {
    let (stub, github, store, server) = fixture().await;
    let item = github.sync_item(&store, 1, "pr").await.unwrap();
    github.writeback(&store, &item).await.unwrap();
    let calls = stub.calls.lock().await.clone();
    assert!(
        calls
            .iter()
            .any(|(path, method, _)| *method == Method::DELETE
                && path.ends_with("review%3Aold-stage"))
    );
    assert!(
        !calls
            .iter()
            .any(|(path, method, _)| *method == Method::DELETE && path.ends_with("/bug"))
    );
    let (_, _, check) = calls
        .iter()
        .find(|(path, method, _)| path.ends_with("/check-runs") && *method == Method::POST)
        .unwrap();
    assert_eq!(check["head_sha"], item["revision"]);
    assert_eq!(check["conclusion"], "action_required");
    assert!(!calls.iter().any(|(path, _, _)| path.ends_with("/merge")));
    let count = calls
        .iter()
        .filter(|(path, method, _)| path.ends_with("/check-runs") && *method == Method::POST)
        .count();
    github.writeback(&store, &item).await.unwrap();
    assert_eq!(
        stub.calls
            .lock()
            .await
            .iter()
            .filter(|(path, method, _)| path.ends_with("/check-runs") && *method == Method::POST)
            .count(),
        count
    );
    server.abort();
}
#[tokio::test]
async fn signed_claim_commands_are_scoped_and_deduplicated() {
    let (_, github, store, server) = fixture().await;
    let mut config = github.config.as_ref().clone();
    config.demo = false;
    config.installation_id = "7".into();
    config.webhook_secret = "secret".into();
    let app = App::new(config, store.clone()).await.unwrap();
    let router = router(app);
    let payload = json!({"action":"created","repository":{"full_name":"zeronsh/zeron"},"installation":{"id":7},"issue":{"number":1,"pull_request":{}},"comment":{"body":"/review claim code-review","user":{"login":"demo-reviewer","type":"User"}}});
    use tower::ServiceExt;
    for _ in 0..2 {
        let bytes = payload.to_string();
        let mut mac = Hmac::<Sha256>::new_from_slice(b"secret").unwrap();
        mac.update(bytes.as_bytes());
        let sig = format!("sha256={}", hex::encode(mac.finalize().into_bytes()));
        let response = router
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/webhooks/github")
                    .header("x-github-event", "issue_comment")
                    .header("x-github-delivery", "one")
                    .header("x-hub-signature-256", sig)
                    .body(axum::body::Body::from(bytes))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
    let claims = store.read(|db| db.claims("pr:1")).await.unwrap();
    assert_eq!(claims[0]["actor"], "demo-reviewer");
    assert_eq!(
        store
            .read(|db| db.events("pr:1"))
            .await
            .unwrap()
            .iter()
            .filter(|e| e["action"] == "claim")
            .count(),
        1
    );
    server.abort();
}
