use crate::{
    Error, Result,
    config::{Config, hash, secret},
    domain, flag,
    github::{GitHub, verify_webhook},
    now, s, seed,
    store::Store,
};
use axum::{
    Router,
    body::{Body, Bytes},
    extract::{DefaultBodyLimit, Request, State},
    http::{HeaderMap, Method, StatusCode, Uri, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::any,
};
use chrono::Utc;
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use subtle::ConstantTimeEq;
use tokio::sync::Mutex;

pub struct Asset {
    path: &'static str,
    mime: &'static str,
    etag: &'static str,
    raw: &'static [u8],
    gzip: &'static [u8],
}
include!(concat!(env!("OUT_DIR"), "/assets.rs"));
pub struct App {
    pub config: Arc<Config>,
    pub store: Store,
    pub github: GitHub,
    reconciling: Mutex<()>,
    publishing: Mutex<()>,
    preview_app: Mutex<Option<Arc<App>>>,
}
impl App {
    pub async fn new(config: Config, store: Store) -> Result<Arc<Self>> {
        config.validate()?;
        let config = Arc::new(config);
        let bootstrap = config.clone();
        store
            .write(move |db| {
                if bootstrap.demo {
                    seed::seed(db)?;
                }
                let fingerprint = hash(serde_json::to_vec(&bootstrap.policy)?);
                if db.value("policyFingerprint")? != json!(fingerprint) {
                    for item in db.list()? {
                        db.enqueue(s(&item, "id"))?;
                        db.event(
                            s(&item, "id"),
                            "system",
                            "policy-evaluated",
                            &json!({"fingerprint":fingerprint}),
                        )?;
                    }
                    db.set("policyFingerprint", &json!(fingerprint))?;
                }
                Ok(())
            })
            .await?;
        let github = GitHub::new(config.clone())?;
        Ok(Arc::new(Self {
            config,
            store,
            github,
            reconciling: Mutex::new(()),
            publishing: Mutex::new(()),
            preview_app: Mutex::new(None),
        }))
    }
    pub async fn preview_app(&self) -> Result<Arc<App>> {
        ensure(
            self.config.dev_preview,
            404,
            "Development preview is disabled.",
        )?;
        let mut cached = self.preview_app.lock().await;
        if let Some(app) = &*cached {
            return Ok(app.clone());
        }
        let mut config = self.config.as_ref().clone();
        config.dev_preview = false;
        config.preview = true;
        config.writeback = false;
        config.agent_tokens.clear();
        config.read_token.clear();
        config.app_id.clear();
        config.installation_id.clear();
        config.private_key_path.clear();
        config.policy.maintainers = vec!["preview-maintainer".into()];
        config.policy.triagers = vec!["preview-triager".into()];
        config.policy.reviewers = vec!["preview-reviewer".into()];
        config.policy.validators = vec!["preview-validator".into()];
        config.data_path = self.config.data_path.with_extension("preview.sqlite");
        let store = Store::open(&config.data_path)?;
        let items = self.store.read(|db| db.list()).await?;
        store
            .write(move |db| {
                for item in items {
                    if db.get(s(&item, "id"))?.is_none() {
                        db.put(&item)?;
                    }
                }
                Ok(())
            })
            .await?;
        let app = App::new(config, store).await?;
        app.workers();
        *cached = Some(app.clone());
        Ok(app)
    }
    pub async fn reconcile(&self) {
        if self.config.demo {
            return;
        }
        let Ok(_guard) = self.reconciling.try_lock() else {
            return;
        };
        let result = self.github.reconcile(&self.store).await;
        let _ = self
            .store
            .write(move |db| match result {
                Ok(()) => db.remove("syncError"),
                Err(error) => db.set("syncError", &json!({"at":now(),"message":error.message})),
            })
            .await;
    }
    pub async fn drain(&self) {
        if !self.config.writeback {
            return;
        }
        let Ok(_guard) = self.publishing.try_lock() else {
            return;
        };
        let Ok(pending) = self.store.read(|db| db.pending()).await else {
            return;
        };
        for id in pending.into_iter().take(1) {
            let get_id = id.clone();
            let Ok(Some(item)) = self.store.read(move |db| db.get(&get_id)).await else {
                continue;
            };
            if flag(&item, "sample") || flag(&item, "local") {
                let _ = self.store.write(move |db| db.sent(&id)).await;
                continue;
            }
            let result = async {
                let fresh = self
                    .github
                    .sync_item(
                        &self.store,
                        item["number"].as_u64().unwrap_or(0),
                        s(&item, "kind"),
                    )
                    .await?;
                if fresh["revision"] != item["revision"]
                    || fresh["baseRevision"] != item["baseRevision"]
                {
                    return Ok(());
                }
                let read_id = id.clone();
                let (latest, claims) = self
                    .store
                    .read(move |db| {
                        Ok((
                            db.get(&read_id)?
                                .ok_or_else(|| Error::new(404, "Item missing"))?,
                            db.claims(&read_id)?,
                        ))
                    })
                    .await?;
                let fingerprint = json!([latest, claims]);
                self.github.writeback(&self.store, &latest).await?;
                let read_id = id.clone();
                self.store
                    .write(move |db| {
                        if json!([db.get(&read_id)?, db.claims(&read_id)?]) == fingerprint {
                            db.sent(&read_id)?;
                        }
                        db.remove("writebackError")
                    })
                    .await
            }
            .await;
            if let Err(error) = result {
                let _ = self
                    .store
                    .write(move |db| {
                        db.failed(&id, &error.message)?;
                        db.set(
                            "writebackError",
                            &json!({"at":now(),"message":error.message}),
                        )
                    })
                    .await;
            }
        }
    }
    pub fn workers(self: &Arc<Self>) {
        let app = self.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(30));
            loop {
                tick.tick().await;
                let _ = app.store.write(|db| db.expire_claims()).await;
                app.drain().await;
            }
        });
        if !self.config.demo && !self.config.installation_id.is_empty() {
            let app = self.clone();
            tokio::spawn(async move {
                let mut tick = tokio::time::interval(Duration::from_secs(1800));
                loop {
                    tick.tick().await;
                    app.reconcile().await;
                }
            });
        }
    }
}
async fn security(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    for (name, value) in [
        ("x-content-type-options", "nosniff"),
        ("referrer-policy", "same-origin"),
        (
            "content-security-policy",
            "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self'; font-src 'self'; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'",
        ),
        ("cache-control", "no-store"),
    ] {
        response
            .headers_mut()
            .entry(header::HeaderName::from_static(name))
            .or_insert(header::HeaderValue::from_static(value));
    }
    response
}
pub fn router(app: Arc<App>) -> Router {
    Router::new()
        .route("/", any(dispatch))
        .route("/{*path}", any(dispatch))
        .layer(DefaultBodyLimit::max(1_000_000))
        .layer(middleware::from_fn(security))
        .with_state(app)
}
fn json_response(value: Value) -> Response {
    axum::Json(value).into_response()
}
fn header_value<'a>(headers: &'a HeaderMap, name: &str) -> &'a str {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
}
fn cookie_value<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    header_value(headers, "cookie")
        .split(';')
        .filter_map(|p| p.trim().split_once('='))
        .find(|(key, _)| *key == name)
        .map(|(_, v)| v)
}
fn equal(a: &str, b: &str) -> bool {
    !a.is_empty() && bool::from(a.as_bytes().ct_eq(b.as_bytes()))
}
fn cookie(config: &Config, name: &str, value: &str, age: u64) -> String {
    format!(
        "{name}={value}; Path=/; HttpOnly; SameSite=Lax; Max-Age={age}{}",
        if config.public_url.starts_with("https:") {
            "; Secure"
        } else {
            ""
        }
    )
}
#[derive(Default)]
struct Identity {
    actor: String,
    csrf: String,
    sid: String,
    agent: bool,
    preview: bool,
    github_id: String,
}
async fn identity(app: &App, headers: &HeaderMap) -> Result<Identity> {
    let authorization = header_value(headers, "authorization");
    if !authorization.is_empty() {
        let digest = authorization
            .strip_prefix("Bearer ")
            .map(hash)
            .unwrap_or_default();
        let actor = app
            .config
            .agent_tokens
            .iter()
            .find(|(token, _)| equal(token, &digest))
            .map(|(_, actor)| actor.clone())
            .ok_or_else(|| Error::new(401, "Invalid agent credential."))?;
        return Ok(Identity {
            actor,
            agent: true,
            ..Default::default()
        });
    }
    for name in ["review_preview", "review_session"] {
        if let Some(sid) = cookie_value(headers, name)
            .filter(|s| s.len() == 64 && s.bytes().all(|c| c.is_ascii_hexdigit()))
        {
            let key = format!("session:{}", hash(sid));
            let session = app.store.read(move |db| db.value(&key)).await?;
            if session["expires"].as_i64().unwrap_or(0) > Utc::now().timestamp_millis() {
                return Ok(Identity {
                    actor: s(&session, "actor").into(),
                    csrf: s(&session, "csrf").into(),
                    sid: sid.into(),
                    agent: false,
                    preview: flag(&session, "preview"),
                    github_id: s(&session, "githubId").into(),
                });
            }
        }
    }
    Ok(Identity::default())
}
fn ensure(condition: bool, status: u16, message: impl Into<String>) -> Result<()> {
    crate::ensure(condition, status, message)
}
fn parse(raw: &[u8]) -> Result<Value> {
    serde_json::from_slice(raw).map_err(|_| Error::new(400, "Invalid JSON."))
}
async fn session(app: &App, actor: String, github_id: String) -> Result<String> {
    let sid = secret();
    let key = format!("session:{}", hash(&sid));
    let value = json!({"actor":actor,"githubId":github_id,"csrf":secret(),"expires":Utc::now().timestamp_millis()+86400000});
    app.store.write(move |db| db.set(&key, &value)).await?;
    Ok(cookie(&app.config, "review_session", &sid, 86400))
}
fn with_cookie(mut response: Response, value: String) -> Result<Response> {
    response
        .headers_mut()
        .insert(header::SET_COOKIE, value.parse().map_err(Error::internal)?);
    Ok(response)
}
fn redirect(location: &str) -> Result<Response> {
    Response::builder()
        .status(StatusCode::FOUND)
        .header(header::LOCATION, location)
        .body(Body::empty())
        .map_err(Error::internal)
}
fn static_response(asset: &Asset, headers: &HeaderMap) -> Response {
    if header_value(headers, "if-none-match")
        .split(',')
        .any(|v| v.trim() == asset.etag || v.trim() == "*")
    {
        return Response::builder()
            .status(StatusCode::NOT_MODIFIED)
            .header(header::ETAG, asset.etag)
            .header(header::VARY, "Accept-Encoding")
            .header(header::CACHE_CONTROL, "public, max-age=0, must-revalidate")
            .body(Body::empty())
            .unwrap();
    }
    let gzip = header_value(headers, "accept-encoding")
        .split(',')
        .any(|part| {
            let mut parts = part.trim().split(';');
            parts.next() == Some("gzip")
                && parts.all(|p| {
                    p.trim()
                        .strip_prefix("q=")
                        .is_none_or(|q| q.parse::<f32>().is_ok_and(|q| q > 0.0))
                })
        });
    let mut builder = Response::builder()
        .header(header::CONTENT_TYPE, asset.mime)
        .header(header::ETAG, asset.etag)
        .header(header::VARY, "Accept-Encoding")
        .header(header::CACHE_CONTROL, "public, max-age=0, must-revalidate");
    if gzip {
        builder = builder.header(header::CONTENT_ENCODING, "gzip");
    }
    builder
        .body(Body::from(if gzip { asset.gzip } else { asset.raw }))
        .unwrap()
}
async fn verified_policy(app: &App, actor: &str) -> Result<(crate::config::Policy, String)> {
    if app.config.demo || app.config.preview {
        return Ok((
            app.config.policy.clone(),
            if actor.is_empty() {
                "guest"
            } else {
                "contributor"
            }
            .into(),
        ));
    }
    let role = if actor.is_empty() {
        "guest".into()
    } else {
        app.github
            .permissions(actor)
            .await
            .unwrap_or_else(|_| "unverified".into())
    };
    let login = actor.to_string();
    let saved_role = role.clone();
    let existing = app.store.read(|db| db.value("githubPermissions")).await?;
    let permissions = if actor.is_empty() || existing[actor] == role {
        existing
    } else {
        app.store
            .write(move |db| {
                let mut records = db.value("githubPermissions")?;
                if !records.is_object() {
                    records = json!({});
                }
                records[&login] = json!(saved_role);
                db.set("githubPermissions", &records)?;
                Ok(records)
            })
            .await?
    };
    let policy = app.config.policy.github_roles(&permissions);
    Ok((policy, role))
}
async fn public_state(app: &App, policy: crate::config::Policy) -> Result<(Bytes, String)> {
    let config = app.config.clone();
    app.store.read(move |db| {
        if let Some(snapshot)=&db.snapshot { return Ok(snapshot.clone()); }
        let claims=db.all_claims()?; let mut grouped:BTreeMap<String,Vec<Value>>=BTreeMap::new();
        for claim in claims { grouped.entry(s(&claim,"item").to_owned()).or_default().push(claim); }
        let mut items=Vec::new();
        for item in db.list()? { items.push(domain::view(&item,grouped.remove(s(&item,"id")).unwrap_or_default(),&policy)?); }
        let value=json!({"mode":if config.demo {"demo"} else {"live"},"repository":config.policy.repository,"writeback":config.writeback,
            "demoActors":if config.demo {seed::ACTORS.to_vec()} else {vec![]},"loginEnabled":!config.client_id.is_empty() && !config.client_secret.is_empty() && !config.installation_id.is_empty(),
            "policy":{"requiredChecks":config.policy.required_checks,"claimHours":config.policy.claim_hours,"decisionDays":config.policy.decision_days},
            "lastReconciled":db.value("lastReconciled")?,"syncError":db.value("syncError")?,"writebackError":db.value("writebackError")?,"items":items});
        let snapshot=Bytes::from(serde_json::to_vec(&value)?); let etag=hash(&snapshot); db.snapshot=Some((snapshot.clone(),etag.clone())); Ok((snapshot,etag))
    }).await
}
async fn webhook(app: Arc<App>, headers: HeaderMap, raw: Bytes) -> Result<Response> {
    ensure(!app.config.demo, 404, "Webhooks are disabled in demo mode.")?;
    ensure(
        verify_webhook(
            &raw,
            header_value(&headers, "x-hub-signature-256"),
            &app.config.webhook_secret,
        ),
        401,
        "Invalid webhook signature.",
    )?;
    let payload = parse(&raw)?;
    let delivery = header_value(&headers, "x-github-delivery").to_owned();
    let event = header_value(&headers, "x-github-event");
    ensure(
        !delivery.is_empty() && delivery.len() <= 200,
        400,
        "Delivery identifier required.",
    )?;
    let delivery_id = delivery.clone();
    if app.store.read(move |db| db.delivered(&delivery_id)).await? {
        return Ok(json_response(json!({"duplicate":true})));
    }
    if event != "ping" {
        let installation = payload["installation"]["id"]
            .as_u64()
            .map(|i| i.to_string())
            .unwrap_or_default();
        ensure(
            s(&payload["repository"], "full_name") == app.config.policy.repository
                && installation == app.config.installation_id,
            403,
            "Unexpected repository or installation.",
        )?;
        match event {
            "pull_request" | "pull_request_review" => {
                app.github
                    .sync_item(
                        &app.store,
                        payload["pull_request"]["number"].as_u64().unwrap_or(0),
                        "pr",
                    )
                    .await?;
            }
            "issues" => {
                app.github
                    .sync_item(
                        &app.store,
                        payload["issue"]["number"].as_u64().unwrap_or(0),
                        "issue",
                    )
                    .await?;
            }
            "issue_comment"
                if s(&payload, "action") == "created"
                    && s(&payload["comment"]["user"], "type") == "User" =>
            {
                let parts: Vec<_> = s(&payload["comment"], "body").split_whitespace().collect();
                if parts.len() == 3
                    && parts[0] == "/review"
                    && ["claim", "renew", "release"].contains(&parts[1])
                    && (["code-review", "reproduce"].contains(&parts[2])
                        || [
                            "validate:linux",
                            "validate:macos",
                            "validate:windows",
                            "validate:ios",
                        ]
                        .contains(&parts[2]))
                {
                    let item = app
                        .github
                        .sync_item(
                            &app.store,
                            payload["issue"]["number"].as_u64().unwrap_or(0),
                            if payload["issue"]["pull_request"].is_object() {
                                "pr"
                            } else {
                                "issue"
                            },
                        )
                        .await?;
                    let actor = s(&payload["comment"]["user"], "login").to_owned();
                    let action = parts[1].to_owned();
                    let task = parts[2].to_owned();
                    let (policy, access) = verified_policy(&app, &actor).await?;
                    ensure(
                        access != "unverified",
                        403,
                        "GitHub access could not be verified.",
                    )?;
                    let delivery_id = delivery.clone();
                    app.store
                        .write(move |db| {
                            if db.delivered(&delivery_id)? {
                                return Ok(());
                            }
                            domain::mutate(
                                db,
                                s(&item, "id"),
                                &actor,
                                &action,
                                &json!({"revision":item["revision"],"task":task}),
                                &policy,
                            )?;
                            db.mark_delivered(&delivery_id)
                        })
                        .await?;
                }
            }
            "check_run" | "check_suite" => {
                let run = if payload["check_run"].is_object() {
                    &payload["check_run"]
                } else {
                    &payload["check_suite"]
                };
                if s(run, "name") != "zeron-review/readiness" {
                    for item in app.store.read(|db| db.list()).await? {
                        if s(&item, "kind") == "pr" && s(&item, "revision") == s(run, "head_sha") {
                            app.github
                                .sync_item(&app.store, item["number"].as_u64().unwrap_or(0), "pr")
                                .await?;
                        }
                    }
                }
            }
            _ => {}
        }
    }
    app.store
        .write(move |db| db.mark_delivered(&delivery))
        .await?;
    let worker = app.clone();
    tokio::spawn(async move {
        worker.drain().await;
    });
    Ok(json_response(json!({"received":true})))
}
async fn dispatch(
    State(app): State<Arc<App>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    raw: Bytes,
) -> Result<Response> {
    let path = uri.path();
    if method == Method::GET || method == Method::HEAD {
        if path == "/health" {
            return Ok(json_response(
                json!({"ok":true,"mode":if app.config.demo {"demo"} else {"live"},"backend":"rust"}),
            ));
        }
        if let Some(asset) = ASSETS.iter().find(|a| a.path == path) {
            return Ok(static_response(asset, &headers));
        }
    }
    if path == "/webhooks/github" && method == Method::POST {
        return webhook(app, headers, raw).await;
    }
    if path.starts_with("/setup/") && method == Method::GET {
        return crate::setup::handle(app, &uri, &headers).await;
    }
    let user = identity(&app, &headers).await?;
    if !user.preview && !user.github_id.is_empty() && path != "/api/logout" {
        let profile = app
            .github
            .request(&format!("/users/{}", user.actor), Method::GET, None)
            .await?;
        ensure(
            profile["id"].as_u64() == user.github_id.parse::<u64>().ok(),
            401,
            "GitHub identity changed. Sign in again.",
        )?;
    }
    if path == "/api/dev/preview" && method == Method::POST {
        ensure(
            app.config.dev_preview,
            404,
            "Development preview is disabled.",
        )?;
        ensure(
            header_value(&headers, "origin") == app.config.public_url,
            403,
            "Same-origin request required.",
        )?;
        ensure(
            header_value(&headers, "content-type") == "application/json",
            415,
            "Use application/json.",
        )?;
        let body = parse(&raw)?;
        let role = s(&body, "role");
        ensure(
            [
                "guest",
                "contributor",
                "triager",
                "reviewer",
                "validator",
                "maintainer",
                "live",
            ]
            .contains(&role),
            400,
            "Unknown preview role.",
        )?;
        if role == "live" {
            return with_cookie(
                json_response(json!({"ok":true})),
                cookie(&app.config, "review_preview", "", 0),
            );
        }
        let sid = secret();
        let key = format!("session:{}", hash(&sid));
        let actor = if role == "guest" {
            String::new()
        } else {
            format!("preview-{role}")
        };
        let value = json!({"actor":actor,"preview":role != "live","csrf":secret(),"expires":Utc::now().timestamp_millis()+86400000});
        app.store.write(move |db| db.set(&key, &value)).await?;
        return with_cookie(
            json_response(json!({"ok":true})),
            cookie(&app.config, "review_preview", &sid, 86400),
        );
    }
    let root = app.clone();
    let app = if user.preview {
        app.preview_app().await?
    } else {
        app
    };
    let (policy, access) = verified_policy(&app, &user.actor).await?;
    if method != Method::GET
        && method != Method::HEAD
        && access == "unverified"
        && path != "/api/logout"
    {
        return Err(Error::new(
            403,
            "GitHub access could not be verified. Retry once GitHub is available.",
        ));
    }

    if method != Method::GET && method != Method::HEAD {
        let origin = header_value(&headers, "origin");
        ensure(
            origin.is_empty() || origin == app.config.public_url,
            403,
            "Unexpected request origin.",
        )?;
        if !user.agent && path != "/api/demo/login" && path != "/api/reports" {
            ensure(
                (!user.actor.is_empty() || (path == "/api/logout" && !user.sid.is_empty()))
                    && equal(header_value(&headers, "x-csrf-token"), &user.csrf),
                403,
                "Session or CSRF token missing.",
            )?;
        }
        ensure(
            header_value(&headers, "content-type").split(';').next() == Some("application/json"),
            415,
            "Use application/json.",
        )?;
    }
    if path == "/auth/github" && method == Method::GET {
        ensure(
            !app.config.demo
                && !app.config.preview
                && !app.config.client_id.is_empty()
                && !app.config.client_secret.is_empty(),
            503,
            "GitHub sign-in is not configured.",
        )?;
        let state = secret();
        let key = format!("oauth:{}", hash(&state));
        app.store
            .write(move |db| {
                db.set(
                    &key,
                    &json!({"expires":Utc::now().timestamp_millis()+600000}),
                )
            })
            .await?;
        let mut target = reqwest::Url::parse("https://github.com/login/oauth/authorize").unwrap();
        target
            .query_pairs_mut()
            .append_pair("client_id", &app.config.client_id)
            .append_pair(
                "redirect_uri",
                &format!("{}/auth/callback", app.config.public_url),
            )
            .append_pair("state", &state);
        return with_cookie(
            redirect(target.as_str())?,
            cookie(&app.config, "review_oauth", &state, 600),
        );
    }
    if path == "/auth/callback" && method == Method::GET {
        let url = reqwest::Url::parse(&format!("{}{}", app.config.public_url, uri))
            .map_err(Error::internal)?;
        let params: BTreeMap<_, _> = url.query_pairs().into_owned().collect();
        let state = params.get("state").cloned().unwrap_or_default();
        let key = format!("oauth:{}", hash(&state));
        let read_key = key.clone();
        let record = app.store.read(move |db| db.value(&read_key)).await?;
        ensure(
            !app.config.demo
                && record["expires"].as_i64().unwrap_or(0) > Utc::now().timestamp_millis()
                && equal(&state, cookie_value(&headers, "review_oauth").unwrap_or("")),
            403,
            "Sign-in expired or invalid.",
        )?;
        app.store.write(move |db| db.remove(&key)).await?;
        let token:Value=app.github.client.post("https://github.com/login/oauth/access_token").header("Accept","application/json").json(&json!({"client_id":app.config.client_id,"client_secret":app.config.client_secret,"code":params.get("code"),"redirect_uri":format!("{}/auth/callback",app.config.public_url)})).send().await?.json().await?;
        ensure(
            !s(&token, "access_token").is_empty(),
            401,
            "GitHub sign-in failed.",
        )?;
        let profile = app
            .github
            .request_token("/user", Method::GET, None, s(&token, "access_token"))
            .await?;
        return with_cookie(
            redirect("/")?,
            session(&app, s(&profile, "login").into(), profile["id"].to_string()).await?,
        );
    }
    if path == "/api/demo/login" && method == Method::POST {
        ensure(app.config.demo, 404, "Demo identities are disabled.")?;
        let body = parse(&raw)?;
        let actor = s(&body, "actor");
        ensure(seed::ACTORS.contains(&actor), 400, "Unknown demo identity.")?;
        return with_cookie(
            json_response(json!({"actor":actor})),
            session(&app, actor.into(), String::new()).await?,
        );
    }
    if path == "/api/reports" && method == Method::POST {
        ensure(
            header_value(&headers, "origin") == app.config.public_url || user.agent,
            403,
            "Same-origin request required.",
        )?;
        if !user.actor.is_empty() && !user.agent {
            ensure(
                equal(header_value(&headers, "x-csrf-token"), &user.csrf),
                403,
                "CSRF token missing.",
            )?;
        }
        let body = parse(&raw)?;
        let actor = user.actor.clone();
        let client = hash(header_value(&headers, "x-forwarded-for"));
        let bucket = format!("report-limit:{}:{}", client, Utc::now().timestamp() / 60);
        let report = app
            .store
            .write(move |db| {
                let count = db.value(&bucket)?.as_u64().unwrap_or(0);
                ensure(
                    count < 10,
                    429,
                    "Too many reports. Please retry in a minute.",
                )?;
                let report = crate::reports::create(db, &actor, &body)?;
                db.set(&bucket, &json!(count + 1))?;
                Ok(report)
            })
            .await?;
        return Ok((StatusCode::CREATED, axum::Json(report)).into_response());
    }
    if path == "/api/stars" {
        ensure(
            !user.actor.is_empty(),
            401,
            "Sign in to save your starred PRs.",
        )?;
        let key = format!("stars:{}", user.actor);
        if method == Method::POST {
            let body = parse(&raw)?;
            let id = s(&body, "id").to_string();
            let starred = flag(&body, "starred");
            app.store
                .write(move |db| {
                    ensure(db.get(&id)?.is_some(), 404, "Item not found.")?;
                    let mut stars = crate::array(&db.value(&key)?, "items");
                    stars.retain(|v| v != &json!(id));
                    if starred {
                        stars.push(json!(id));
                    }
                    db.set(&key, &json!({"items":stars}))
                })
                .await?;
            return Ok(json_response(json!({"ok":true})));
        }
    }
    if path == "/api/logout" && method == Method::POST {
        for name in ["review_session", "review_preview"] {
            if let Some(sid) = cookie_value(&headers, name) {
                let key = format!("session:{}", hash(sid));
                root.store.write(move |db| db.remove(&key)).await?;
            }
        }
        let mut response = with_cookie(
            json_response(json!({"ok":true})),
            cookie(&app.config, "review_session", "", 0),
        )?;
        response.headers_mut().append(
            header::SET_COOKIE,
            cookie(&app.config, "review_preview", "", 0)
                .parse()
                .unwrap(),
        );
        return Ok(response);
    }
    if path == "/api/state" && method == Method::GET {
        let (public, version) = public_state(&app, policy.clone()).await?;
        let star_key = format!("stars:{}", user.actor);
        let stars = if user.actor.is_empty() {
            json!([])
        } else {
            app.store
                .read(move |db| Ok(db.value(&star_key)?["items"].clone()))
                .await?
        };
        let etag = format!(
            "\"{}\"",
            hash(format!(
                "{version}:{access}:{}:{}:{}",
                user.actor,
                user.csrf,
                policy.roles(&user.actor).join(",")
            ))
        );
        if header_value(&headers, "if-none-match") == etag {
            return Ok(Response::builder()
                .status(StatusCode::NOT_MODIFIED)
                .header(header::ETAG, etag)
                .body(Body::empty())
                .unwrap());
        }
        let identity = serde_json::to_vec(
            &json!({"actor":if user.actor.is_empty() {Value::Null} else {json!(user.actor)},"roles":policy.roles(&user.actor),"access":access,"preview":app.config.preview,"devPreview":root.config.dev_preview,"stars":stars,"csrf":if user.csrf.is_empty() {Value::Null} else {json!(user.csrf)}}),
        )?;
        let mut body = Vec::with_capacity(public.len() + identity.len());
        body.extend_from_slice(&public[..public.len() - 1]);
        body.push(b',');
        body.extend_from_slice(&identity[1..]);
        let mut response = ([(header::CONTENT_TYPE, "application/json")], body).into_response();
        response
            .headers_mut()
            .insert(header::ETAG, etag.parse().unwrap());
        return Ok(response);
    }
    if path == "/api/sync" && method == Method::POST {
        ensure(
            policy.roles(&user.actor).contains(&"maintainer"),
            403,
            "Maintainer capability required.",
        )?;
        ensure(
            !app.config.demo && !app.config.preview,
            409,
            "Sandbox mode does not contact GitHub.",
        )?;
        let worker = app.clone();
        tokio::spawn(async move {
            worker.reconcile().await;
        });
        return Ok((StatusCode::ACCEPTED, axum::Json(json!({"accepted":true}))).into_response());
    }
    if let Some(tail) = path.strip_prefix("/api/items/") {
        let parts: Vec<_> = tail.split('/').collect();
        let id = parts[0].to_owned();
        let valid = id.split_once(':').is_some_and(|(kind, number)| {
            ["pr", "issue", "report"].contains(&kind)
                && !number.is_empty()
                && number.bytes().all(|c| c.is_ascii_digit())
        });
        ensure(valid, 404, "Item not found.")?;
        let policy = policy.clone();
        if method == Method::GET && parts.len() == 1 {
            return app
                .store
                .read(move |db| {
                    let item = db
                        .get(&id)?
                        .ok_or_else(|| Error::new(404, "Item not found."))?;
                    Ok(json_response(domain::context(db, &item, &policy)?))
                })
                .await;
        }
        ensure(
            method == Method::POST && parts.len() == 2,
            405,
            "Unsupported method.",
        )?;
        let action = parts[1].to_owned();
        let body = parse(&raw)?;
        ensure(body.is_object(), 400, "JSON object required.")?;
        let key = header_value(&headers, "idempotency-key");
        ensure(key.len() <= 200, 400, "Invalid idempotency key.")?;
        let cache_key = if key.is_empty() {
            None
        } else {
            Some(format!(
                "request:{}",
                hash(format!("{}:{path}:{key}", user.actor))
            ))
        };
        let digest = hash(body.to_string());
        let updated = app
            .store
            .write(move |db| {
                if let Some(key) = &cache_key {
                    let previous = db.value(key)?;
                    if previous.is_object() {
                        ensure(
                            s(&previous, "digest") == digest,
                            409,
                            "Idempotency key reused with different data.",
                        )?;
                        return Ok(previous["result"].clone());
                    }
                }
                let result = domain::mutate(db, &id, &user.actor, &action, &body, &policy)?;
                if let Some(key) = cache_key {
                    db.set(&key, &json!({"digest":digest,"result":result}))?;
                }
                Ok(result)
            })
            .await?;
        let worker = app.clone();
        tokio::spawn(async move {
            worker.drain().await;
        });
        return Ok(json_response(updated));
    }
    Err(Error::new(404, "Route not found."))
}
