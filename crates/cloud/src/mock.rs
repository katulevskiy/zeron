//! A local HTTP server that records every request, and [`FakeCloudflare`]:
//! enough of the Cloudflare API and of the Zeron Worker's routes to provision
//! and wake boxes in tests (feature `mock`).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use serde_json::{Value, json};

use crate::multipart;
use crate::sign;

#[derive(Debug, Clone)]
pub struct Recorded {
    pub method: String,
    /// Path and query.
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Recorded {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }

    /// `"METHOD path"`, for asserting sequences.
    pub fn line(&self) -> String {
        format!("{} {}", self.method, self.path)
    }

    pub fn path_only(&self) -> &str {
        self.path.split('?').next().unwrap_or_default()
    }
}

#[derive(Debug, Clone)]
pub struct Reply {
    pub status: u16,
    pub body: Vec<u8>,
}

impl Reply {
    pub fn json(status: u16, value: Value) -> Self {
        Self {
            status,
            body: value.to_string().into_bytes(),
        }
    }

    /// A Cloudflare success envelope.
    pub fn ok(result: Value) -> Self {
        Self::json(
            200,
            json!({ "success": true, "errors": [], "messages": [], "result": result }),
        )
    }

    /// A Cloudflare error envelope.
    pub fn error(status: u16, code: i64, message: &str) -> Self {
        Self::json(
            status,
            json!({ "success": false, "errors": [{ "code": code, "message": message }], "messages": [], "result": null }),
        )
    }
}

type Handler = Arc<dyn Fn(&Recorded) -> Reply + Send + Sync>;

pub struct MockServer {
    pub url: String,
    requests: Arc<Mutex<Vec<Recorded>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for MockServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl MockServer {
    pub async fn start(handler: impl Fn(&Recorded) -> Reply + Send + Sync + 'static) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock server");
        let url = format!("http://{}", listener.local_addr().expect("mock addr"));
        let requests: Arc<Mutex<Vec<Recorded>>> = Arc::default();
        let handler: Handler = Arc::new(handler);
        let log = requests.clone();
        let task = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                let handler = handler.clone();
                let log = log.clone();
                tokio::spawn(async move {
                    let service = service_fn(move |request: hyper::Request<Incoming>| {
                        let handler = handler.clone();
                        let log = log.clone();
                        async move {
                            let method = request.method().to_string();
                            let path = request
                                .uri()
                                .path_and_query()
                                .map(|p| p.to_string())
                                .unwrap_or_default();
                            let headers = request
                                .headers()
                                .iter()
                                .map(|(k, v)| {
                                    (k.to_string(), v.to_str().unwrap_or_default().to_string())
                                })
                                .collect();
                            let body = request
                                .into_body()
                                .collect()
                                .await
                                .map(|b| b.to_bytes().to_vec())
                                .unwrap_or_default();
                            let recorded = Recorded {
                                method,
                                path,
                                headers,
                                body,
                            };
                            let reply = handler(&recorded);
                            log.lock().unwrap_or_else(|e| e.into_inner()).push(recorded);
                            hyper::Response::builder()
                                .status(reply.status)
                                .header("content-type", "application/json")
                                .body(Full::new(bytes::Bytes::from(reply.body)))
                        }
                    });
                    let _ = hyper::server::conn::http1::Builder::new()
                        .serve_connection(TokioIo::new(stream), service)
                        .await;
                });
            }
        });
        Self {
            url,
            requests,
            task,
        }
    }

    pub fn requests(&self) -> Vec<Recorded> {
        self.requests
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn clear(&self) {
        self.requests
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }
}

// ── a fake Cloudflare account ───────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct FakeState {
    pub token: String,
    pub accounts: Vec<(String, String)>,
    /// `false`: the token is account-owned (the user endpoint refuses it).
    pub user_token: bool,
    /// Overrides `GET /accounts/{id}/containers/me`.
    pub containers_me: Option<Reply>,
    /// Paths (without query) answered 403, as a token lacking a permission.
    pub forbidden: Vec<String>,
    pub bucket: bool,
    pub bucket_objects: usize,
    /// The script's migration tag, once uploaded.
    pub script_tag: Option<String>,
    /// Every upload: `(metadata, modules)`.
    pub uploads: Vec<(Value, Vec<multipart::Part>)>,
    pub namespace_id: Option<String>,
    pub apps: Vec<Value>,
    pub rollouts: Vec<Value>,
    pub secrets: BTreeMap<String, String>,
    pub subdomain: Option<String>,
    pub script_subdomain: bool,
    /// Box routes the Worker served: `(deviceId, action)`.
    pub box_calls: Vec<(String, String)>,
    /// What `status` reports.
    pub box_state: String,
}

impl Default for FakeState {
    fn default() -> Self {
        Self {
            token: "good-token".into(),
            accounts: vec![("acc-1".into(), "Dana's account".into())],
            user_token: true,
            containers_me: None,
            forbidden: Vec::new(),
            bucket: false,
            bucket_objects: 0,
            script_tag: None,
            uploads: Vec::new(),
            namespace_id: None,
            apps: Vec::new(),
            rollouts: Vec::new(),
            secrets: BTreeMap::new(),
            subdomain: None,
            script_subdomain: false,
            box_calls: Vec::new(),
            box_state: "asleep".into(),
        }
    }
}

#[derive(Clone, Default)]
pub struct FakeCloudflare {
    pub state: Arc<Mutex<FakeState>>,
}

impl FakeCloudflare {
    pub fn new(state: FakeState) -> Self {
        Self {
            state: Arc::new(Mutex::new(state)),
        }
    }

    pub fn state(&self) -> std::sync::MutexGuard<'_, FakeState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Serve it. The API root is `{url}/client/v4`; the Worker's box routes
    /// are served at `{url}` itself.
    pub async fn serve(&self) -> MockServer {
        let fake = self.clone();
        MockServer::start(move |request| fake.handle(request)).await
    }

    pub fn handle(&self, request: &Recorded) -> Reply {
        let path = request.path_only().to_string();
        if let Some(rest) = path.strip_prefix("/v1/boxes/") {
            return self.box_route(request, rest);
        }
        let Some(api) = path.strip_prefix("/client/v4") else {
            return Reply::json(404, json!({ "error": "not found" }));
        };
        let mut state = self.state();
        let bearer = request
            .header("authorization")
            .and_then(|h| h.strip_prefix("Bearer "))
            .unwrap_or_default();
        if bearer != state.token {
            return Reply::error(401, 1000, "Invalid API Token");
        }
        if state.forbidden.iter().any(|p| p == api) {
            return Reply::error(403, 10000, "Authentication error");
        }
        let segments: Vec<&str> = api.trim_matches('/').split('/').collect();
        let method = request.method.as_str();
        match (method, segments.as_slice()) {
            ("GET", ["user", "tokens", "verify"]) => {
                if state.user_token {
                    Reply::ok(json!({ "id": "tok", "status": "active" }))
                } else {
                    Reply::error(401, 1000, "Invalid API Token")
                }
            }
            ("GET", ["accounts", _, "tokens", "verify"]) => {
                Reply::ok(json!({ "id": "tok", "status": "active" }))
            }
            ("GET", ["accounts"]) => Reply::ok(Value::Array(
                state
                    .accounts
                    .iter()
                    .map(|(id, name)| json!({ "id": id, "name": name }))
                    .collect(),
            )),
            ("GET", ["accounts", _, "containers", "me"]) => {
                state.containers_me.clone().unwrap_or_else(|| {
                    Reply::ok(json!({ "external_account_id": "acc-1", "limits": {} }))
                })
            }
            ("POST", ["accounts", _, "r2", "buckets"]) => {
                if state.bucket {
                    Reply::error(
                        409,
                        10004,
                        "The bucket you tried to create already exists, and you own it.",
                    )
                } else {
                    state.bucket = true;
                    Reply::ok(json!({ "name": request.json()["name"] }))
                }
            }
            ("DELETE", ["accounts", _, "r2", "buckets", _]) => {
                if !state.bucket {
                    Reply::error(404, 10006, "The specified bucket does not exist.")
                } else if state.bucket_objects > 0 {
                    Reply::error(409, 10008, "The bucket you tried to delete is not empty.")
                } else {
                    state.bucket = false;
                    Reply::ok(json!({}))
                }
            }
            ("GET", ["accounts", _, "workers", "services", _]) => match &state.script_tag {
                Some(tag) => Reply::ok(json!({
                    "id": "zeron-cloud",
                    "default_environment": { "environment": "production", "script": { "id": "zeron-cloud", "migration_tag": tag } }
                })),
                None => Reply::error(404, 10090, "This Worker does not exist on your account."),
            },
            ("PUT", ["accounts", _, "workers", "scripts", _]) => {
                let content_type = request.header("content-type").unwrap_or_default();
                let Some(parts) = multipart::parse(content_type, &request.body) else {
                    return Reply::error(400, 10021, "bad multipart");
                };
                let Some(metadata) = parts
                    .iter()
                    .find(|p| p.name == "metadata")
                    .and_then(|p| serde_json::from_slice::<Value>(&p.body).ok())
                else {
                    return Reply::error(400, 10021, "missing metadata");
                };
                if let Some(migrations) = metadata.get("migrations") {
                    let old = migrations.get("old_tag").and_then(Value::as_str);
                    if old != state.script_tag.as_deref() {
                        return Reply::error(400, 10079, "migration tag precondition failed");
                    }
                    state.script_tag = migrations
                        .get("new_tag")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                    state.namespace_id.get_or_insert_with(|| "ns-1".into());
                } else if state.script_tag.is_none() {
                    return Reply::error(
                        400,
                        10061,
                        "Cannot create binding for class ZeronBox that is not exported",
                    );
                }
                let modules = parts.into_iter().filter(|p| p.name != "metadata").collect();
                state.uploads.push((metadata, modules));
                Reply::ok(json!({ "id": "zeron-cloud" }))
            }
            ("DELETE", ["accounts", _, "workers", "scripts", _]) => {
                state.script_tag = None;
                state.namespace_id = None;
                state.secrets.clear();
                Reply::ok(json!({}))
            }
            ("GET", ["accounts", _, "workers", "durable_objects", "namespaces"]) => {
                let mut list = vec![
                    json!({ "id": "ns-other", "name": "other", "script": "other", "class": "ZeronBox" }),
                ];
                if let Some(id) = &state.namespace_id {
                    list.push(json!({ "id": id, "name": "zeron-cloud_ZeronBox", "script": "zeron-cloud", "class": "ZeronBox", "use_sqlite": true }));
                }
                Reply::ok(Value::Array(list))
            }
            ("GET", ["accounts", _, "containers", "applications"]) => {
                Reply::ok(Value::Array(state.apps.clone()))
            }
            ("POST", ["accounts", _, "containers", "applications"]) => {
                let mut app = request.json();
                app["id"] = json!(format!("app-{}", state.apps.len() + 1));
                state.apps.push(app.clone());
                Reply::ok(app)
            }
            ("PATCH", ["accounts", _, "containers", "applications", id]) => {
                let patch = request.json();
                let Some(app) = state.apps.iter_mut().find(|a| a["id"] == *id) else {
                    return Reply::error(404, 1, "application not found");
                };
                if let Value::Object(fields) = patch {
                    for (key, value) in fields {
                        app[key.as_str()] = value;
                    }
                }
                Reply::ok(app.clone())
            }
            ("DELETE", ["accounts", _, "containers", "applications", id]) => {
                state.apps.retain(|a| a["id"] != *id);
                Reply::ok(json!({}))
            }
            ("POST", ["accounts", _, "containers", "applications", _, "rollouts"]) => {
                state.rollouts.push(request.json());
                Reply::ok(json!({ "id": "rollout-1" }))
            }
            ("PUT", ["accounts", _, "workers", "scripts", _, "secrets"]) => {
                let body = request.json();
                state.secrets.insert(
                    body["name"].as_str().unwrap_or_default().to_string(),
                    body["text"].as_str().unwrap_or_default().to_string(),
                );
                Reply::ok(json!({ "name": body["name"], "type": "secret_text" }))
            }
            ("GET", ["accounts", _, "workers", "subdomain"]) => match &state.subdomain {
                Some(sub) => Reply::ok(json!({ "subdomain": sub })),
                None => Reply::error(404, 10007, "workers.dev subdomain not found"),
            },
            ("PUT", ["accounts", _, "workers", "subdomain"]) => {
                let sub = request.json()["subdomain"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                state.subdomain = Some(sub.clone());
                Reply::ok(json!({ "subdomain": sub }))
            }
            ("POST", ["accounts", _, "workers", "scripts", _, "subdomain"]) => {
                state.script_subdomain = request.json()["enabled"] == json!(true);
                Reply::ok(json!({ "enabled": state.script_subdomain }))
            }
            _ => Reply::error(404, 7003, "No route for that URI"),
        }
    }

    /// The Zeron Worker's signed routes, checked like `worker.js` does.
    fn box_route(&self, request: &Recorded, rest: &str) -> Reply {
        let mut state = self.state();
        let Some((device_id, action)) = rest.split_once('/') else {
            return Reply::json(404, json!({ "error": "not found" }));
        };
        let boxes: Value = state
            .secrets
            .get(crate::worker::BOXES_SECRET)
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or(Value::Null);
        let Some(wake_key) = boxes[device_id]["wakeKey"].as_str() else {
            return Reply::json(404, json!({ "error": "unknown box" }));
        };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or_default();
        if sign::verify(
            wake_key,
            &request.method,
            request.path_only(),
            request.header(sign::TIMESTAMP_HEADER).unwrap_or_default(),
            request.header(sign::SIGNATURE_HEADER).unwrap_or_default(),
            &request.body,
            now,
        )
        .is_err()
        {
            return Reply::json(401, json!({ "error": "bad signature" }));
        }
        state
            .box_calls
            .push((device_id.to_string(), action.to_string()));
        match action {
            "wake" => {
                if state.box_state == "asleep" {
                    state.box_state = "waking".into();
                }
                Reply::json(200, json!({ "state": state.box_state }))
            }
            "stop" => {
                state.box_state = "asleep".into();
                Reply::json(200, json!({ "state": "checkpointing" }))
            }
            "status" => Reply::json(200, json!({ "state": state.box_state, "busy": false })),
            "purge" => {
                let deleted = state.bucket_objects;
                state.bucket_objects = 0;
                Reply::json(200, json!({ "deleted": deleted, "done": true }))
            }
            _ => Reply::json(404, json!({ "error": "not found" })),
        }
    }
}
