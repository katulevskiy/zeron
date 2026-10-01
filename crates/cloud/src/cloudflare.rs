//! Cloudflare: a Worker + SQLite Durable Object + container application in
//! the user's own account, provisioned over the REST API (docs/cloud.md §7).
//!
//! Provisioning order (each step idempotent):
//!
//! 1. `GET /user/tokens/verify` (an account-owned token: `GET /accounts`
//!    then `/accounts/{id}/tokens/verify`), `GET /accounts`,
//!    `GET /accounts/{id}/containers/me`.
//! 2. Enrol the box with the Zeron edge ([`Enroller`]).
//! 3. `POST /accounts/{id}/r2/buckets` `zeron-cloud` (409 = exists).
//! 4. `GET /accounts/{id}/workers/services/zeron-cloud` for the migration
//!    tag, then `PUT /accounts/{id}/workers/scripts/zeron-cloud` (multipart).
//!    The `v1` migration is sent only while the script doesn't have it.
//! 5. `GET /accounts/{id}/workers/durable_objects/namespaces` → the
//!    `ZeronBox` namespace.
//! 6. `GET /accounts/{id}/containers/applications?name=zeron-cloud`, then
//!    `POST …/containers/applications`, or `PATCH …/applications/{app}`
//!    (plus `POST …/applications/{app}/rollouts` when the image or instance
//!    type changed).
//! 7. `PUT /accounts/{id}/workers/scripts/zeron-cloud/secrets` `ZERON_BOXES`.
//! 8. `GET /accounts/{id}/workers/subdomain` (`PUT` one if the account has
//!    none), `POST …/scripts/zeron-cloud/subdomain {"enabled":true}`.

use std::collections::BTreeMap;

use async_trait::async_trait;
use reqwest::Method;
use serde_json::{Value, json};

use crate::error::{CloudError, Permission};
use crate::multipart::Multipart;
use crate::worker::{self, BoxSecret, WorkerClient};
use crate::{
    Account, BoxStatus, CloudBox, CloudBoxState, CloudProvider, Enroller, Estimate, ExistingBox,
    ProvisionRequest, Provisioned, UpgradeRequest,
};

pub const DEFAULT_API_BASE: &str = "https://api.cloudflare.com/client/v4";
pub const PROVIDER: &str = "cloudflare";
/// R2 bucket for checkpoints (keys under `boxes/{deviceId}/`).
pub const BUCKET: &str = "zeron-cloud";
/// The container application's name.
pub const APP_NAME: &str = "zeron-cloud";
pub const MIGRATION_TAG: &str = "v1";
/// How long a rollout lets a running box finish (its SIGTERM grace).
pub const ROLLOUT_GRACE_SECONDS: u64 = 900;

/// Cloudflare's "worker not found" codes.
const WORKER_NOT_FOUND: [i64; 2] = [10007, 10090];
const BUCKET_EXISTS: i64 = 10004;
const R2_NOT_ENABLED: i64 = 10042;

#[derive(Clone)]
pub struct Cloudflare {
    http: reqwest::Client,
    base: String,
    token: String,
    account_id: Option<String>,
    worker: WorkerClient,
}

enum Body {
    None,
    Json(Value),
    Multipart(String, Vec<u8>),
}

impl Cloudflare {
    pub fn new(api_token: impl Into<String>) -> Self {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .unwrap_or_default();
        Self {
            worker: WorkerClient::with_client(http.clone()),
            http,
            base: DEFAULT_API_BASE.into(),
            token: api_token.into(),
            account_id: None,
        }
    }

    /// Point at another API root (tests use a local server).
    pub fn with_base_url(mut self, base: impl Into<String>) -> Self {
        self.base = base.into().trim_end_matches('/').to_string();
        self
    }

    /// Use this account (a token that can see several).
    pub fn with_account(mut self, account_id: Option<String>) -> Self {
        self.account_id = account_id.filter(|id| !id.trim().is_empty());
        self
    }

    // ── HTTP ────────────────────────────────────────────────────────────────

    /// One API call: the envelope's `result`, or an error that names the
    /// missing `permission` when Cloudflare refuses.
    async fn call(
        &self,
        method: Method,
        path: &str,
        body: Body,
        permission: Option<Permission>,
    ) -> Result<Value, CloudError> {
        self.call_raw(method, path, body)
            .await?
            .map_err(|failure| failure.into_error(path, permission))
    }

    /// One API call; `Ok(Err(..))` is Cloudflare's refusal, unmapped.
    async fn call_raw(
        &self,
        method: Method,
        path: &str,
        body: Body,
    ) -> Result<Result<Value, Failure>, CloudError> {
        let url = format!("{}{path}", self.base);
        let mut request = self.http.request(method, &url).bearer_auth(&self.token);
        request = match body {
            Body::None => request,
            Body::Json(value) => request.json(&value),
            Body::Multipart(content_type, bytes) => {
                request.header("content-type", content_type).body(bytes)
            }
        };
        let response = request
            .send()
            .await
            .map_err(|e| CloudError::Network("the Cloudflare API".into(), e.to_string()))?;
        let status = response.status().as_u16();
        let text = response.text().await.unwrap_or_default();
        let envelope: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        let success = envelope
            .get("success")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        if (200..300).contains(&status) && success {
            return Ok(Ok(envelope.get("result").cloned().unwrap_or(Value::Null)));
        }
        let first = envelope
            .get("errors")
            .and_then(Value::as_array)
            .and_then(|errors| errors.first());
        let code = first.and_then(|e| e.get("code")).and_then(Value::as_i64);
        let message = first
            .and_then(|e| e.get("message"))
            .and_then(Value::as_str)
            .or_else(|| envelope.get("error").and_then(Value::as_str))
            .map(str::to_owned)
            .unwrap_or_else(|| text.chars().take(300).collect());
        Ok(Err(Failure {
            status,
            code,
            message,
        }))
    }

    async fn get(&self, path: &str, permission: Permission) -> Result<Value, CloudError> {
        self.call(Method::GET, path, Body::None, Some(permission))
            .await
    }

    async fn send_json(
        &self,
        method: Method,
        path: &str,
        body: Value,
        permission: Permission,
    ) -> Result<Value, CloudError> {
        self.call(method, path, Body::Json(body), Some(permission))
            .await
    }

    // ── 1. validate ─────────────────────────────────────────────────────────

    async fn verify_token(&self) -> Result<(), CloudError> {
        let user = self
            .call(Method::GET, "/user/tokens/verify", Body::None, None)
            .await;
        match user {
            Ok(result) if token_active(&result) => Ok(()),
            Ok(_) => Err(CloudError::InvalidToken),
            // An account-owned token can't call the user endpoint: verify it
            // against its account instead.
            Err(_) => {
                let account = self.pick_account().await?;
                let result = self
                    .call(
                        Method::GET,
                        &format!("/accounts/{}/tokens/verify", account.id),
                        Body::None,
                        None,
                    )
                    .await?;
                if token_active(&result) {
                    Ok(())
                } else {
                    Err(CloudError::InvalidToken)
                }
            }
        }
    }

    async fn pick_account(&self) -> Result<Account, CloudError> {
        let listed = self
            .get("/accounts?per_page=50", Permission::AccountSettings)
            .await?;
        let accounts: Vec<Account> = listed
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|a| {
                Some(Account {
                    id: a.get("id")?.as_str()?.to_string(),
                    name: a
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                })
            })
            .collect();
        if let Some(wanted) = &self.account_id {
            return accounts
                .into_iter()
                .find(|a| &a.id == wanted)
                .ok_or(CloudError::NoAccount);
        }
        match accounts.len() {
            0 => Err(CloudError::NoAccount),
            1 => Ok(accounts.into_iter().next().expect("one account")),
            n => Err(CloudError::MultipleAccounts(n)),
        }
    }

    async fn check_containers(&self, account_id: &str) -> Result<(), CloudError> {
        let path = format!("/accounts/{account_id}/containers/me");
        let Err(failure) = self.call_raw(Method::GET, &path, Body::None).await? else {
            return Ok(());
        };
        let lower = failure.message.to_ascii_lowercase();
        let about_plan = ["paid", "plan", "subscri", "entitle"]
            .iter()
            .any(|w| lower.contains(w));
        Err(match failure.status {
            401 => CloudError::InvalidToken,
            _ if about_plan => CloudError::NotOnWorkersPaid,
            403 => CloudError::MissingPermission(Permission::Containers),
            500.. => failure.into_error(&path, None),
            _ => CloudError::ContainersNotEnabled(failure.message),
        })
    }

    // ── 3. bucket ───────────────────────────────────────────────────────────

    async fn ensure_bucket(&self, account_id: &str) -> Result<(), CloudError> {
        let result = self
            .send_json(
                Method::POST,
                &format!("/accounts/{account_id}/r2/buckets"),
                json!({ "name": BUCKET }),
                Permission::R2Storage,
            )
            .await;
        match result {
            Ok(_) => Ok(()),
            Err(e) if e.status() == Some(409) || e.code() == Some(BUCKET_EXISTS) => Ok(()),
            Err(e) => Err(e),
        }
    }

    // ── 4. Worker ───────────────────────────────────────────────────────────

    /// The script's current migration tag (`None`: no script yet).
    async fn migration_tag(&self, account_id: &str) -> Result<Option<String>, CloudError> {
        let path = format!(
            "/accounts/{account_id}/workers/services/{}",
            worker::SCRIPT_NAME
        );
        match self.get(&path, Permission::WorkersScripts).await {
            Ok(service) => Ok(service
                .pointer("/default_environment/script/migration_tag")
                .and_then(Value::as_str)
                .map(str::to_owned)),
            Err(e)
                if e.status() == Some(404)
                    || e.code().is_some_and(|c| WORKER_NOT_FOUND.contains(&c)) =>
            {
                Ok(None)
            }
            Err(e) => Err(e),
        }
    }

    /// The upload's `metadata` part.
    pub fn worker_metadata(edge_url: &str, migration_tag: Option<&str>) -> Value {
        let steps = json!([{ "new_sqlite_classes": [worker::BOX_CLASS] }]);
        let migrations = match migration_tag {
            Some(MIGRATION_TAG) => None,
            Some(old) => Some(json!({ "old_tag": old, "new_tag": MIGRATION_TAG, "steps": steps })),
            None => Some(json!({ "new_tag": MIGRATION_TAG, "steps": steps })),
        };
        let mut metadata = json!({
            "main_module": worker::MODULE_NAME,
            "compatibility_date": worker::COMPATIBILITY_DATE,
            "bindings": [
                { "type": "durable_object_namespace", "name": "BOX", "class_name": worker::BOX_CLASS },
                { "type": "r2_bucket", "name": "CKPT", "bucket_name": BUCKET },
                { "type": "plain_text", "name": "ZERON_EDGE_URL", "text": edge_url },
            ],
            "containers": [{ "class_name": worker::BOX_CLASS }],
            "keep_bindings": ["secret_text"],
        });
        if let Some(migrations) = migrations {
            metadata["migrations"] = migrations;
        }
        metadata
    }

    async fn upload_worker(&self, account_id: &str, edge_url: &str) -> Result<(), CloudError> {
        let tag = self.migration_tag(account_id).await?;
        let metadata = Self::worker_metadata(edge_url, tag.as_deref());
        let mut form = Multipart::new();
        form.part(
            "metadata",
            None,
            "application/json",
            metadata.to_string().as_bytes(),
        );
        for (name, source) in worker::MODULES {
            form.part(
                name,
                Some(name),
                "application/javascript+module",
                source.as_bytes(),
            );
        }
        let (content_type, bytes) = form.finish();
        self.call(
            Method::PUT,
            &format!(
                "/accounts/{account_id}/workers/scripts/{}",
                worker::SCRIPT_NAME
            ),
            Body::Multipart(content_type, bytes),
            Some(Permission::WorkersScripts),
        )
        .await?;
        Ok(())
    }

    // ── 5. Durable Object namespace ─────────────────────────────────────────

    async fn namespace_id(&self, account_id: &str) -> Result<String, CloudError> {
        let listed = self
            .get(
                &format!("/accounts/{account_id}/workers/durable_objects/namespaces?per_page=1000"),
                Permission::WorkersScripts,
            )
            .await?;
        listed
            .as_array()
            .into_iter()
            .flatten()
            .find(|ns| {
                ns.get("class").and_then(Value::as_str) == Some(worker::BOX_CLASS)
                    && ns.get("script").and_then(Value::as_str) == Some(worker::SCRIPT_NAME)
            })
            .and_then(|ns| ns.get("id").and_then(Value::as_str))
            .map(str::to_owned)
            .ok_or_else(|| {
                CloudError::Other(
                    "Cloudflare didn't create the ZeronBox Durable Object for the Worker. Try again in a minute."
                        .into(),
                )
            })
    }

    // ── 6. container application ────────────────────────────────────────────

    async fn find_app(&self, account_id: &str) -> Result<Option<Value>, CloudError> {
        let listed = self
            .get(
                &format!("/accounts/{account_id}/containers/applications?name={APP_NAME}"),
                Permission::Containers,
            )
            .await?;
        Ok(listed
            .as_array()
            .into_iter()
            .flatten()
            .find(|app| app.get("name").and_then(Value::as_str) == Some(APP_NAME))
            .cloned())
    }

    async fn ensure_app(
        &self,
        account_id: &str,
        namespace_id: &str,
        image: &str,
        instance_type: &str,
        max_instances: usize,
        region: Option<&str>,
    ) -> Result<(), CloudError> {
        let configuration = json!({ "image": image, "instance_type": instance_type });
        let apps = format!("/accounts/{account_id}/containers/applications");
        let Some(app) = self.find_app(account_id).await? else {
            let mut body = json!({
                "name": APP_NAME,
                "scheduling_policy": "default",
                "configuration": configuration,
                "instances": 0,
                "max_instances": max_instances,
                "durable_objects": { "namespace_id": namespace_id },
                "rollout_active_grace_period": ROLLOUT_GRACE_SECONDS,
            });
            if let Some(region) = region {
                body["constraints"] = json!({ "regions": [region] });
            }
            self.send_json(Method::POST, &apps, body, Permission::Containers)
                .await?;
            return Ok(());
        };
        let app_id = app.get("id").and_then(Value::as_str).ok_or_else(|| {
            CloudError::Other("Cloudflare listed a container app without an id".into())
        })?;
        if let Some(bound) = app
            .pointer("/durable_objects/namespace_id")
            .and_then(Value::as_str)
            && bound != namespace_id
        {
            return Err(CloudError::Other(format!(
                "A container application named “{APP_NAME}” already belongs to another Worker. \
                 Delete it under Workers & Pages → Containers in the Cloudflare dashboard, then try again."
            )));
        }
        let same_config = app.pointer("/configuration/image").and_then(Value::as_str)
            == Some(image)
            && app
                .pointer("/configuration/instance_type")
                .and_then(Value::as_str)
                == Some(instance_type);
        let same_size =
            app.get("max_instances").and_then(Value::as_u64) == Some(max_instances as u64);
        if same_config && same_size {
            return Ok(());
        }
        let mut patch = json!({
            "max_instances": max_instances,
            "rollout_active_grace_period": ROLLOUT_GRACE_SECONDS,
            "configuration": configuration,
        });
        if let Some(region) = region {
            patch["constraints"] = json!({ "regions": [region] });
        }
        self.send_json(
            Method::PATCH,
            &format!("{apps}/{app_id}"),
            patch,
            Permission::Containers,
        )
        .await?;
        if !same_config {
            self.send_json(
                Method::POST,
                &format!("{apps}/{app_id}/rollouts"),
                json!({
                    "description": "Zeron cloud box update",
                    "strategy": "rolling",
                    "kind": "full_auto",
                    "step_percentage": 100,
                    "target_configuration": configuration,
                }),
                Permission::Containers,
            )
            .await?;
        }
        Ok(())
    }

    // ── 7. secret ───────────────────────────────────────────────────────────

    async fn put_boxes_secret(
        &self,
        account_id: &str,
        boxes: &BTreeMap<String, BoxSecret>,
    ) -> Result<(), CloudError> {
        let text = serde_json::to_string(boxes).map_err(|e| CloudError::Other(e.to_string()))?;
        self.send_json(
            Method::PUT,
            &format!(
                "/accounts/{account_id}/workers/scripts/{}/secrets",
                worker::SCRIPT_NAME
            ),
            json!({ "name": worker::BOXES_SECRET, "text": text, "type": "secret_text" }),
            Permission::WorkersScripts,
        )
        .await?;
        Ok(())
    }

    // ── 8. workers.dev ──────────────────────────────────────────────────────

    async fn enable_workers_dev(&self, account_id: &str) -> Result<String, CloudError> {
        let path = format!("/accounts/{account_id}/workers/subdomain");
        let existing = match self.get(&path, Permission::WorkersScripts).await {
            Ok(result) => result
                .get("subdomain")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_owned),
            Err(e) if e.status() == Some(404) || e.code() == Some(10007) => None,
            Err(e) => return Err(e),
        };
        let subdomain = match existing {
            Some(subdomain) => subdomain,
            None => {
                let mut bytes = [0u8; 4];
                getrandom::getrandom(&mut bytes).map_err(|e| CloudError::Other(e.to_string()))?;
                let wanted = format!("zeron-{}", hex::encode(bytes));
                let result = self
                    .send_json(
                        Method::PUT,
                        &path,
                        json!({ "subdomain": wanted }),
                        Permission::WorkersScripts,
                    )
                    .await?;
                result
                    .get("subdomain")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .unwrap_or(wanted)
            }
        };
        self.send_json(
            Method::POST,
            &format!(
                "/accounts/{account_id}/workers/scripts/{}/subdomain",
                worker::SCRIPT_NAME
            ),
            json!({ "enabled": true, "previews_enabled": false }),
            Permission::WorkersScripts,
        )
        .await?;
        Ok(format!(
            "https://{}.{subdomain}.workers.dev",
            worker::SCRIPT_NAME
        ))
    }

    // ── helpers ─────────────────────────────────────────────────────────────

    /// `ZERON_BOXES` for `boxes` on `account_id`; every one needs its credential.
    fn secret_entries<'a>(
        account_id: &str,
        boxes: impl IntoIterator<Item = &'a ExistingBox>,
    ) -> Result<BTreeMap<String, BoxSecret>, CloudError> {
        let mut out = BTreeMap::new();
        for existing in boxes {
            let record = &existing.record;
            if record.provider != PROVIDER || record.account_id != account_id {
                continue;
            }
            let credential = existing.credential.clone().ok_or_else(|| {
                CloudError::Other(format!(
                    "This device doesn't hold the keys of your cloud box “{}”. \
                     Do this from the device that created it.",
                    record.name
                ))
            })?;
            out.insert(
                record.id.clone(),
                BoxSecret {
                    wake_key: record.wake_key.clone(),
                    credential,
                    idle_minutes: record.idle_minutes,
                },
            );
        }
        Ok(out)
    }
}

/// Cloudflare refused a call.
struct Failure {
    status: u16,
    code: Option<i64>,
    message: String,
}

impl Failure {
    fn into_error(self, path: &str, permission: Option<Permission>) -> CloudError {
        match (self.status, self.code) {
            (401, _) => CloudError::InvalidToken,
            (_, Some(R2_NOT_ENABLED)) => CloudError::R2NotEnabled,
            (403, _) | (_, Some(10000)) => match permission {
                Some(permission) => CloudError::MissingPermission(permission),
                None => CloudError::InvalidToken,
            },
            _ => CloudError::Api {
                path: path.to_string(),
                status: self.status,
                code: self.code,
                message: self.message,
            },
        }
    }
}

fn token_active(result: &Value) -> bool {
    result.get("status").and_then(Value::as_str) == Some("active")
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or_default()
}

#[async_trait]
impl CloudProvider for Cloudflare {
    fn id(&self) -> &'static str {
        PROVIDER
    }

    async fn validate(&self) -> Result<Account, CloudError> {
        self.verify_token().await?;
        let account = self.pick_account().await?;
        self.check_containers(&account.id).await?;
        Ok(account)
    }

    async fn provision(
        &self,
        request: &ProvisionRequest,
        enroller: &dyn Enroller,
    ) -> Result<Provisioned, CloudError> {
        if crate::estimate::instance_type(&request.instance_type).is_none() {
            return Err(CloudError::Other(format!(
                "“{}” isn't a Cloudflare Containers instance type",
                request.instance_type
            )));
        }
        // 1.
        let account = self.validate().await?;
        // Every other box on this account must stay reachable after the
        // secret is rewritten: check before enrolling anything.
        let mut secret = Self::secret_entries(&account.id, &request.existing)?;
        // 2.
        let enrolled = enroller.enroll(&request.name).await?;
        // 3–6.
        self.ensure_bucket(&account.id).await?;
        self.upload_worker(&account.id, &request.edge_url).await?;
        let namespace = self.namespace_id(&account.id).await?;
        secret.insert(
            enrolled.device_id.clone(),
            BoxSecret {
                wake_key: enrolled.wake_key.clone(),
                credential: enrolled.credential.clone(),
                idle_minutes: request.idle_minutes,
            },
        );
        self.ensure_app(
            &account.id,
            &namespace,
            &request.image,
            &request.instance_type,
            secret.len(),
            request.region.as_deref(),
        )
        .await?;
        // 7.
        self.put_boxes_secret(&account.id, &secret).await?;
        // 8.
        let worker_url = self.enable_workers_dev(&account.id).await?;
        let now = now_ms();
        Ok(Provisioned {
            record: CloudBox {
                id: enrolled.device_id,
                provider: PROVIDER.into(),
                name: request.name.clone(),
                account_id: account.id,
                worker_url,
                wake_key: enrolled.wake_key,
                instance_type: request.instance_type.clone(),
                idle_minutes: request.idle_minutes,
                created_at: now,
                state: CloudBoxState::Asleep,
                state_at: now,
                error: None,
            },
            credential: enrolled.credential,
        })
    }

    async fn upgrade(&self, request: &UpgradeRequest) -> Result<(), CloudError> {
        self.upload_worker(&request.account_id, &request.edge_url)
            .await?;
        let namespace = self.namespace_id(&request.account_id).await?;
        self.ensure_app(
            &request.account_id,
            &namespace,
            &request.image,
            &request.instance_type,
            request.boxes.max(1),
            request.region.as_deref(),
        )
        .await
    }

    async fn wake(&self, record: &CloudBox) -> Result<BoxStatus, CloudError> {
        self.worker.wake(record).await
    }

    async fn stop(&self, record: &CloudBox) -> Result<BoxStatus, CloudError> {
        self.worker.stop(record).await
    }

    async fn status(&self, record: &CloudBox) -> Result<BoxStatus, CloudError> {
        self.worker.status(record).await
    }

    async fn destroy(
        &self,
        record: &CloudBox,
        existing: &[ExistingBox],
        keep_data: bool,
    ) -> Result<(), CloudError> {
        let account_id = record.account_id.as_str();
        let remaining: Vec<&ExistingBox> = existing
            .iter()
            .filter(|b| {
                b.record.id != record.id
                    && b.record.provider == PROVIDER
                    && b.record.account_id == account_id
            })
            .collect();
        let secret = Self::secret_entries(account_id, remaining.iter().copied())?;
        if !keep_data {
            // The Worker holds the bucket binding: let it empty the prefix.
            for _ in 0..10_000 {
                match self.worker.purge(record).await {
                    Ok(reply) if reply.done => break,
                    Ok(_) => continue,
                    // Already gone from the Worker: nothing it can delete.
                    Err(CloudError::BoxUnknown) => break,
                    Err(e) => return Err(e),
                }
            }
        }
        if let Err(error) = self.worker.stop(record).await {
            tracing::debug!(box_id = %record.id, %error, "stopping a box being destroyed");
        }
        if !remaining.is_empty() {
            self.put_boxes_secret(account_id, &secret).await?;
            if let Some(app) = self.find_app(account_id).await?
                && let Some(app_id) = app.get("id").and_then(Value::as_str)
            {
                self.send_json(
                    Method::PATCH,
                    &format!("/accounts/{account_id}/containers/applications/{app_id}"),
                    json!({ "max_instances": remaining.len() }),
                    Permission::Containers,
                )
                .await?;
            }
            return Ok(());
        }
        // The last box: take the app and the Worker with it.
        if let Some(app) = self.find_app(account_id).await?
            && let Some(app_id) = app.get("id").and_then(Value::as_str)
        {
            self.call(
                Method::DELETE,
                &format!("/accounts/{account_id}/containers/applications/{app_id}"),
                Body::None,
                Some(Permission::Containers),
            )
            .await?;
        }
        match self
            .call(
                Method::DELETE,
                &format!(
                    "/accounts/{account_id}/workers/scripts/{}?force=true",
                    worker::SCRIPT_NAME
                ),
                Body::None,
                Some(Permission::WorkersScripts),
            )
            .await
        {
            Ok(_) => {}
            Err(e)
                if e.status() == Some(404)
                    || e.code().is_some_and(|c| WORKER_NOT_FOUND.contains(&c)) => {}
            Err(e) => return Err(e),
        }
        if !keep_data {
            match self
                .call(
                    Method::DELETE,
                    &format!("/accounts/{account_id}/r2/buckets/{BUCKET}"),
                    Body::None,
                    Some(Permission::R2Storage),
                )
                .await
            {
                Ok(_) => {}
                Err(e) if e.status() == Some(404) => {}
                // Something else lives in the bucket: leave it.
                Err(e) if e.status() == Some(409) => {
                    tracing::info!(error = %e, "kept the zeron-cloud bucket: it isn't empty");
                }
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    fn estimate(&self, instance_type: &str, active_hours_per_month: f64) -> Option<Estimate> {
        crate::estimate::estimate(instance_type, active_hours_per_month)
    }
}

#[cfg(test)]
mod tests;
