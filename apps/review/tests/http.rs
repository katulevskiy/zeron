use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use serde_json::{Value, json};
use std::path::Path;
use tower::ServiceExt;
use zeron_review::{
    config::Config,
    server::{App, router},
    store::Store,
};
async fn fixture() -> (Router, std::sync::Arc<App>) {
    let mut config = Config::load(true).unwrap();
    config.public_url = "http://localhost:3080".into();
    let app = App::new(config, Store::open(Path::new(":memory:")).unwrap())
        .await
        .unwrap();
    (router(app.clone()), app)
}
async fn call(
    router: &Router,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Value,
) -> (StatusCode, axum::http::HeaderMap, Value) {
    let mut builder = Request::builder().method(method).uri(path);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let response = router
        .clone()
        .oneshot(
            builder
                .body(if method == "GET" {
                    Body::empty()
                } else {
                    Body::from(body.to_string())
                })
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = to_bytes(response.into_body(), 1_000_000).await.unwrap();
    (
        status,
        headers,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}
async fn login(router: &Router) -> (String, String, Value) {
    let (_, headers, _) = call(
        router,
        "POST",
        "/api/demo/login",
        &[("content-type", "application/json")],
        json!({"actor":"demo-maintainer"}),
    )
    .await;
    let cookie = headers["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let (_, _, state) = call(
        router,
        "GET",
        "/api/state",
        &[("cookie", &cookie)],
        Value::Null,
    )
    .await;
    (cookie, state["csrf"].as_str().unwrap().to_owned(), state)
}
#[tokio::test]
async fn authenticated_actions_enforce_origin_csrf_and_idempotency() {
    let (router, app) = fixture().await;
    let (cookie, csrf, state) = login(&router).await;
    let item = state["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["id"] == "pr:90001")
        .unwrap();
    let input = json!({"revision":item["revision"],"task":"code-review"});
    let headers = [
        ("cookie", cookie.as_str()),
        ("content-type", "application/json"),
        ("x-csrf-token", csrf.as_str()),
        ("idempotency-key", "one"),
    ];
    let mut hostile = headers.to_vec();
    hostile.push(("origin", "https://other.example"));
    assert_eq!(
        call(
            &router,
            "POST",
            "/api/items/pr:90001/claim",
            &hostile,
            input.clone()
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &router,
            "POST",
            "/api/items/pr:90001/claim",
            &[("cookie", &cookie), ("content-type", "application/json")],
            input.clone()
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &router,
            "POST",
            "/api/items/pr:90001/claim",
            &headers,
            input.clone()
        )
        .await
        .0,
        StatusCode::OK
    );
    let before = app
        .store
        .read(|db| db.events("pr:90001"))
        .await
        .unwrap()
        .len();
    assert_eq!(
        call(
            &router,
            "POST",
            "/api/items/pr:90001/claim",
            &headers,
            input
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        app.store
            .read(|db| db.events("pr:90001"))
            .await
            .unwrap()
            .len(),
        before
    );
    assert_eq!(
        call(
            &router,
            "POST",
            "/api/items/pr:90001/claim",
            &headers,
            json!({"revision":item["revision"],"task":"validate:macos"})
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
}
#[tokio::test]
async fn demo_disables_webhooks_and_live_writeback() {
    let (router, app) = fixture().await;
    assert!(!app.config.writeback);
    assert_eq!(
        call(&router, "POST", "/webhooks/github", &[], json!({}))
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(
            &router,
            "GET",
            "/api/state",
            &[("authorization", "Bearer invalid")],
            Value::Null
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    let mut config = app.config.as_ref().clone();
    config.demo = false;
    config.policy.maintainers.clear();
    assert!(config.validate().is_err());
}
#[tokio::test]
async fn static_assets_are_embedded_compressed_and_revalidated() {
    let (router, _) = fixture().await;
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/app.js")
                .header("accept-encoding", "gzip")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-encoding"], "gzip");
    let etag = response.headers()["etag"].clone();
    let compressed = to_bytes(response.into_body(), 1_000_000).await.unwrap();
    let raw = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/app.js")
                .header("accept-encoding", "gzip;q=0")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(!raw.headers().contains_key("content-encoding"));
    assert!(to_bytes(raw.into_body(), 1_000_000).await.unwrap().len() > compressed.len());
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/app.js")
                .header("if-none-match", etag)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
}
#[tokio::test]
async fn snapshots_invalidate_after_mutations_and_do_not_expose_another_identity() {
    let (router, _) = fixture().await;
    let (cookie, csrf, state) = login(&router).await;
    let item = state["items"][0].clone();
    call(
        &router,
        "POST",
        &format!("/api/items/{}/vote", item["id"].as_str().unwrap()),
        &[
            ("cookie", &cookie),
            ("x-csrf-token", &csrf),
            ("content-type", "application/json"),
        ],
        json!({"revision":item["revision"],"dimension":"demand","reason":"Need this"}),
    )
    .await;
    let (_, _, public) = call(&router, "GET", "/api/state", &[], Value::Null).await;
    assert!(public["actor"].is_null());
    assert!(public["csrf"].is_null());
    assert_eq!(
        public["items"][0]["votes"]["demo-maintainer:demand"]["reason"],
        "Need this"
    );
}
#[tokio::test]
async fn queue_etags_are_bound_to_identity_and_change_after_committed_work() {
    let (router, _) = fixture().await;
    let (_, public_headers, _) = call(&router, "GET", "/api/state", &[], Value::Null).await;
    let public_tag = public_headers["etag"].to_str().unwrap();
    assert_eq!(
        call(
            &router,
            "GET",
            "/api/state",
            &[("if-none-match", public_tag)],
            Value::Null
        )
        .await
        .0,
        StatusCode::NOT_MODIFIED
    );
    let (cookie, csrf, state) = login(&router).await;
    let (status, personal_headers, _) = call(
        &router,
        "GET",
        "/api/state",
        &[("cookie", &cookie), ("if-none-match", public_tag)],
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_ne!(public_headers["etag"], personal_headers["etag"]);
    let item = &state["items"][0];
    assert_eq!(
        call(
            &router,
            "POST",
            &format!("/api/items/{}/vote", item["id"].as_str().unwrap()),
            &[
                ("cookie", &cookie),
                ("x-csrf-token", &csrf),
                ("content-type", "application/json")
            ],
            json!({"revision":item["revision"],"dimension":"urgency","reason":"Users are blocked"})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        call(
            &router,
            "GET",
            "/api/state",
            &[("if-none-match", public_tag)],
            Value::Null
        )
        .await
        .0,
        StatusCode::OK
    );
}
#[tokio::test]
async fn agent_tokens_inherit_the_accountable_principals_permissions() {
    let mut config = Config::load(true).unwrap();
    let token = "test-agent-opaque-token-with-at-least-32-characters";
    config
        .agent_tokens
        .insert(zeron_review::config::hash(token), "demo-reviewer".into());
    let app = App::new(config, Store::open(Path::new(":memory:")).unwrap())
        .await
        .unwrap();
    let router = router(app);
    let authorization = format!("Bearer {token}");
    let headers = [
        ("authorization", authorization.as_str()),
        ("content-type", "application/json"),
    ];
    let (_, _, state) = call(&router, "GET", "/api/state", &headers, Value::Null).await;
    assert_eq!(state["actor"], "demo-reviewer");
    let item = state["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == "pr:90001")
        .unwrap();
    assert_eq!(
        call(
            &router,
            "POST",
            "/api/items/pr:90001/claim",
            &headers,
            json!({"revision":item["revision"],"task":"code-review"})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(call(&router, "POST", "/api/items/pr:90001/direction", &headers, json!({"revision":item["revision"],"verdict":"accepted","reason":"Agent cannot set direction"})).await.0, StatusCode::FORBIDDEN);
}
#[tokio::test]
async fn a_policy_change_invalidates_an_existing_sessions_capability_etag() {
    let (original, app) = fixture().await;
    let (cookie, _, _) = login(&original).await;
    let (_, headers, _) = call(
        &original,
        "GET",
        "/api/state",
        &[("cookie", &cookie)],
        Value::Null,
    )
    .await;
    let previous_tag = headers["etag"].to_str().unwrap();
    let mut config = app.config.as_ref().clone();
    config.policy.maintainers.clear();
    let updated = router(App::new(config, app.store.clone()).await.unwrap());
    let (status, _, state) = call(
        &updated,
        "GET",
        "/api/state",
        &[("cookie", &cookie), ("if-none-match", previous_tag)],
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(state["actor"], "demo-maintainer");
    assert_eq!(state["roles"], json!([]));
}
