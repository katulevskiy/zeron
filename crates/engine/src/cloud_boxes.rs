//! Cloud boxes from the desktop side (docs/cloud.md §1, §6, §7): the
//! Cloudflare API token, provisioning, and waking/stopping/removing boxes.
//!
//! - The token (and the device credentials of the boxes this device created)
//!   live in the macOS Keychain, or a 0600 file elsewhere — the way agent
//!   logins are kept.
//! - Records are synced registry rows (`cloudBoxes`), so every device can
//!   list, wake and stop a box; provisioning and removal run where the token
//!   is (the RPCs are forwardable).
//! - The box's device credential is minted by the edge (`POST /cloud/devices`)
//!   with the user's bearer.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use zeron_cloud::cloudflare::{self, Cloudflare};
use zeron_cloud::{
    CloudError, CloudProvider, Enroller, Enrollment, ExistingBox, ProvisionRequest, WorkerClient,
};
use zeron_proto::{
    CloudBox, CloudBoxState, CloudBoxesReply, CloudConnectParams, CloudConnectReply,
    CloudProvisionParams,
};

use crate::EngineError;
use crate::doc_host::EdgeConfig;
use crate::workspace_host::WorkspaceHost;

/// Overrides the Cloudflare API root (development and tests).
pub const API_BASE_ENV: &str = "ZERON_CLOUDFLARE_API";
/// Overrides the container image (development).
pub const IMAGE_ENV: &str = "ZERON_CLOUD_IMAGE";

// ── secrets ─────────────────────────────────────────────────────────────────

/// A box enrolled with the edge whose provisioning hasn't finished yet: a
/// retry reuses it instead of minting another device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingBox {
    pub name: String,
    pub enrollment: Enrollment,
}

/// What this device keeps secret about the user's Cloudflare account.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudSecrets {
    pub api_token: String,
    #[serde(default)]
    pub account_id: Option<String>,
    #[serde(default)]
    pub account_name: Option<String>,
    /// Device credentials of the boxes created here, by box id (they rebuild
    /// the Worker's `ZERON_BOXES` secret).
    #[serde(default)]
    pub credentials: BTreeMap<String, String>,
    #[serde(default)]
    pub pending: Option<PendingBox>,
}

/// Keychain on macOS (one generic password), a 0600 file elsewhere.
#[derive(Debug, Clone)]
pub struct CloudTokenStore {
    file: PathBuf,
    keychain_service: Option<String>,
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const KEYCHAIN_ACCOUNT: &str = "cloudflare";

impl CloudTokenStore {
    /// The production store for an engine data dir.
    pub fn detect(data_dir: &Path) -> Self {
        let service = cfg!(target_os = "macos").then(|| {
            use sha2::Digest as _;
            let digest = sha2::Sha256::digest(data_dir.to_string_lossy().as_bytes());
            format!("Zeron Cloud ({})", &hex_prefix(&digest))
        });
        Self {
            file: data_dir.join("cloud-credentials.json"),
            keychain_service: service,
        }
    }

    /// A 0600 file only (tests, and platforms without a Keychain).
    pub fn file(path: impl Into<PathBuf>) -> Self {
        Self {
            file: path.into(),
            keychain_service: None,
        }
    }

    pub async fn load(&self) -> Result<Option<CloudSecrets>, EngineError> {
        let text = self.read().await?;
        match text {
            Some(text) => serde_json::from_str(&text).map(Some).map_err(|e| {
                EngineError::Other(format!("The saved Cloudflare token is unreadable: {e}"))
            }),
            None => Ok(None),
        }
    }

    async fn read(&self) -> Result<Option<String>, EngineError> {
        #[cfg(target_os = "macos")]
        if let Some(service) = &self.keychain_service {
            return crate::agent_accounts::keychain::read_secret(service, KEYCHAIN_ACCOUNT)
                .await
                .map_err(EngineError::Other);
        }
        match std::fs::read_to_string(&self.file) {
            Ok(text) => Ok(Some(text)),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(err.into()),
        }
    }

    pub async fn save(&self, secrets: &CloudSecrets) -> Result<(), EngineError> {
        let text = serde_json::to_string(secrets).map_err(|e| EngineError::Other(e.to_string()))?;
        #[cfg(target_os = "macos")]
        if let Some(service) = &self.keychain_service {
            return crate::agent_accounts::keychain::write_secret(service, KEYCHAIN_ACCOUNT, &text)
                .await
                .map_err(EngineError::Other);
        }
        write_private(&self.file, text.as_bytes()).map_err(Into::into)
    }

    pub async fn clear(&self) {
        #[cfg(target_os = "macos")]
        if let Some(service) = &self.keychain_service {
            crate::agent_accounts::keychain::delete_secret(service, KEYCHAIN_ACCOUNT).await;
            return;
        }
        let _ = std::fs::remove_file(&self.file);
    }
}

fn hex_prefix(bytes: &[u8]) -> String {
    bytes.iter().take(4).map(|b| format!("{b:02x}")).collect()
}

/// Write a file only its owner can read (0600), atomically.
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(dir)?;
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tmp.as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    tmp.write_all(bytes)?;
    tmp.as_file().sync_all()?;
    tmp.persist(path).map_err(|e| e.error)?;
    Ok(())
}

// ── the service ─────────────────────────────────────────────────────────────

pub struct CloudBoxesConfig {
    pub data_dir: PathBuf,
    pub workspace: WorkspaceHost,
    /// The edge the boxes' engines join (and that enrols them). `None`: a
    /// local-only engine, which can't create boxes.
    pub edge: Option<EdgeConfig>,
}

struct Inner {
    workspace: WorkspaceHost,
    store: Mutex<CloudTokenStore>,
    edge: Mutex<Option<EdgeConfig>>,
    api_base: Mutex<String>,
    image: String,
    http: reqwest::Client,
    worker: WorkerClient,
    /// One provisioning or removal at a time on this device.
    busy: tokio::sync::Mutex<()>,
}

#[derive(Clone)]
pub struct CloudBoxes(Arc<Inner>);

impl CloudBoxes {
    pub fn new(config: CloudBoxesConfig) -> Self {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .unwrap_or_default();
        Self(Arc::new(Inner {
            workspace: config.workspace,
            store: Mutex::new(CloudTokenStore::detect(&config.data_dir)),
            edge: Mutex::new(config.edge),
            api_base: Mutex::new(
                std::env::var(API_BASE_ENV)
                    .ok()
                    .filter(|v| !v.trim().is_empty())
                    .unwrap_or_else(|| cloudflare::DEFAULT_API_BASE.into()),
            ),
            image: std::env::var(IMAGE_ENV)
                .ok()
                .filter(|v| !v.trim().is_empty())
                .unwrap_or_else(zeron_cloud::default_image),
            worker: WorkerClient::with_client(http.clone()),
            http,
            busy: tokio::sync::Mutex::new(()),
        }))
    }

    /// Test seam: a file store instead of the Keychain.
    #[doc(hidden)]
    pub fn set_token_store(&self, store: CloudTokenStore) {
        *lock(&self.0.store) = store;
    }

    /// Test seam: another Cloudflare API root.
    #[doc(hidden)]
    pub fn set_api_base(&self, base: &str) {
        *lock(&self.0.api_base) = base.trim_end_matches('/').to_string();
    }

    /// Test seam: another edge for enrolment.
    #[doc(hidden)]
    pub fn set_edge(&self, edge: Option<EdgeConfig>) {
        *lock(&self.0.edge) = edge;
    }

    fn store(&self) -> CloudTokenStore {
        lock(&self.0.store).clone()
    }

    fn provider(&self, secrets: &CloudSecrets) -> Cloudflare {
        Cloudflare::new(secrets.api_token.clone())
            .with_base_url(lock(&self.0.api_base).clone())
            .with_account(secrets.account_id.clone())
    }

    async fn secrets(&self) -> Result<CloudSecrets, EngineError> {
        self.store()
            .load()
            .await?
            .ok_or_else(|| failed(CloudError::NotConnected))
    }

    /// `CloudConnect`: check the token, then keep it on this device.
    pub async fn connect(
        &self,
        params: CloudConnectParams,
    ) -> Result<CloudConnectReply, EngineError> {
        let token = params.api_token.trim().to_string();
        if token.is_empty() {
            return Err(EngineError::Other("Paste a Cloudflare API token".into()));
        }
        let candidate = CloudSecrets {
            api_token: token,
            account_id: params.account_id.clone(),
            ..CloudSecrets::default()
        };
        let account = self.provider(&candidate).validate().await.map_err(failed)?;
        let _guard = self.0.busy.lock().await;
        // A new token keeps what this device knows about the boxes it created.
        let previous = self.store().load().await.ok().flatten().unwrap_or_default();
        let same_account = previous.account_id.as_deref() == Some(account.id.as_str());
        let secrets = CloudSecrets {
            api_token: candidate.api_token,
            account_id: Some(account.id.clone()),
            account_name: Some(account.name.clone()),
            credentials: previous.credentials,
            pending: previous.pending.filter(|_| same_account),
        };
        self.store().save(&secrets).await?;
        Ok(CloudConnectReply {
            account_id: account.id,
            account_name: account.name,
        })
    }

    /// `CloudBoxes`.
    pub async fn list(&self) -> CloudBoxesReply {
        let secrets = self.store().load().await.ok().flatten();
        CloudBoxesReply {
            boxes: self.0.workspace.read_cloud_boxes(),
            connected: secrets.is_some(),
            account_id: secrets.and_then(|s| s.account_id),
        }
    }

    /// `CloudProvision`: create a box and write its record.
    pub async fn provision(&self, params: CloudProvisionParams) -> Result<CloudBox, EngineError> {
        let name = params.name.trim().to_string();
        if name.is_empty() {
            return Err(EngineError::Other("Name the cloud box".into()));
        }
        if !(1..=24 * 60).contains(&params.idle_minutes) {
            return Err(EngineError::Other(
                "The idle time must be between 1 minute and 24 hours".into(),
            ));
        }
        let edge = lock(&self.0.edge).clone().ok_or_else(|| {
            EngineError::Other(
                "Sign in to Zeron first: a cloud box joins your account like any device".into(),
            )
        })?;
        let _guard = self.0.busy.lock().await;
        let secrets = self.secrets().await?;
        let existing: Vec<ExistingBox> = self
            .0
            .workspace
            .read_cloud_boxes()
            .into_iter()
            .filter(|b| b.provider == cloudflare::PROVIDER)
            .map(|record| ExistingBox {
                credential: secrets.credentials.get(&record.id).cloned(),
                record,
            })
            .collect();
        let request = ProvisionRequest {
            name: name.clone(),
            instance_type: params.instance_type.clone(),
            idle_minutes: params.idle_minutes,
            region: params.region.clone().filter(|r| !r.trim().is_empty()),
            edge_url: edge.url.clone(),
            image: self.0.image.clone(),
            existing,
        };
        let enroller = EdgeEnroller {
            boxes: self,
            http: &self.0.http,
            edge: &edge,
        };
        let provisioned = self
            .provider(&secrets)
            .provision(&request, &enroller)
            .await
            .map_err(failed)?;
        let record = provisioned.record;
        let mut secrets = self.secrets().await?;
        secrets
            .credentials
            .insert(record.id.clone(), provisioned.credential);
        secrets.pending = None;
        self.store().save(&secrets).await?;
        // 9. The record: every device can now list and wake the box.
        self.0.workspace.upsert_cloud_box(&record)?;
        Ok(record)
    }

    fn record(&self, box_id: &str) -> Result<CloudBox, EngineError> {
        self.0
            .workspace
            .cloud_box(box_id)
            .ok_or_else(|| EngineError::Other("No such cloud box".into()))
    }

    /// `CloudWake` (any device: the record carries the key).
    pub async fn wake(&self, box_id: &str) -> Result<CloudBox, EngineError> {
        let record = self.record(box_id)?;
        wake_record(&self.0.workspace, &self.0.worker, &record).await?;
        self.record(box_id)
    }

    /// `CloudStop` (any device).
    pub async fn stop(&self, box_id: &str) -> Result<CloudBox, EngineError> {
        let record = self.record(box_id)?;
        let status = self.0.worker.stop(&record).await.map_err(failed)?;
        self.0
            .workspace
            .set_cloud_box_state(box_id, status.state, None)?;
        self.record(box_id)
    }

    /// `CloudDestroy`: remove the box from Cloudflare, revoke its device, and
    /// drop its record.
    pub async fn destroy(&self, box_id: &str, keep_data: bool) -> Result<(), EngineError> {
        let record = self.record(box_id)?;
        let _guard = self.0.busy.lock().await;
        let mut secrets = self.secrets().await?;
        let existing: Vec<ExistingBox> = self
            .0
            .workspace
            .read_cloud_boxes()
            .into_iter()
            .map(|record| ExistingBox {
                credential: secrets.credentials.get(&record.id).cloned(),
                record,
            })
            .collect();
        self.provider(&secrets)
            .destroy(&record, &existing, keep_data)
            .await
            .map_err(failed)?;
        // Copied out first: the guard must not be held across the request.
        let edge = lock(&self.0.edge).clone();
        if let Some(edge) = edge
            && let Err(error) = revoke(&self.0.http, &edge, box_id).await
        {
            tracing::warn!(box_id, %error, "could not revoke the cloud box's device credential");
        }
        secrets.credentials.remove(box_id);
        self.store().save(&secrets).await?;
        self.0.workspace.delete_cloud_box(box_id);
        Ok(())
    }
}

/// Wake a box from its record and record what the Worker said (moves use
/// this too).
pub(crate) async fn wake_record(
    workspace: &WorkspaceHost,
    worker: &WorkerClient,
    record: &CloudBox,
) -> Result<zeron_cloud::BoxStatus, EngineError> {
    match worker.wake(record).await {
        Ok(status) => {
            let state = match status.state {
                CloudBoxState::Running => CloudBoxState::Running,
                _ => CloudBoxState::Waking,
            };
            workspace.set_cloud_box_state(&record.id, state, None)?;
            Ok(status)
        }
        Err(error) => {
            let message = error.to_string();
            let _ = workspace.set_cloud_box_state(&record.id, CloudBoxState::Error, Some(&message));
            Err(EngineError::Other(message))
        }
    }
}

fn failed(error: CloudError) -> EngineError {
    EngineError::Other(error.to_string())
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

// ── enrolment (docs/cloud.md §6) ────────────────────────────────────────────

struct EdgeEnroller<'a> {
    boxes: &'a CloudBoxes,
    http: &'a reqwest::Client,
    edge: &'a EdgeConfig,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Enrolled {
    device_id: String,
    credential: String,
}

#[async_trait]
impl Enroller for EdgeEnroller<'_> {
    async fn enroll(&self, name: &str) -> Result<Enrollment, CloudError> {
        let store = self.boxes.store();
        let mut secrets = store
            .load()
            .await
            .map_err(|e| CloudError::Other(e.to_string()))?
            .ok_or(CloudError::NotConnected)?;
        if let Some(pending) = &secrets.pending
            && pending.name == name
        {
            return Ok(pending.enrollment.clone());
        }
        let enrolled = enroll(self.http, self.edge, name).await?;
        let enrollment = Enrollment {
            device_id: enrolled.device_id,
            credential: enrolled.credential,
            wake_key: zeron_cloud::sign::new_wake_key(),
        };
        // Kept before anything else happens: the credential is shown once.
        secrets.pending = Some(PendingBox {
            name: name.to_string(),
            enrollment: enrollment.clone(),
        });
        store
            .save(&secrets)
            .await
            .map_err(|e| CloudError::Other(e.to_string()))?;
        Ok(enrollment)
    }
}

async fn bearer(edge: &EdgeConfig) -> Result<String, CloudError> {
    edge.token
        .token()
        .await
        .map_err(|e| CloudError::Other(format!("Sign in to Zeron again: {e}")))
}

/// `POST {edge}/cloud/devices {name}` → `{deviceId, credential}`.
async fn enroll(
    http: &reqwest::Client,
    edge: &EdgeConfig,
    name: &str,
) -> Result<Enrolled, CloudError> {
    let url = format!("{}/cloud/devices", edge.url.trim_end_matches('/'));
    let response = http
        .post(&url)
        .bearer_auth(bearer(edge).await?)
        .json(&serde_json::json!({ "name": name }))
        .send()
        .await
        .map_err(|e| CloudError::Network("the Zeron edge".into(), e.to_string()))?;
    let status = response.status();
    if !status.is_success() {
        let text = response.text().await.unwrap_or_default();
        return Err(CloudError::Other(format!(
            "The Zeron edge couldn't register the cloud box ({}): {}",
            status.as_u16(),
            text.chars().take(200).collect::<String>()
        )));
    }
    response
        .json()
        .await
        .map_err(|e| CloudError::Other(format!("The Zeron edge sent an unexpected reply: {e}")))
}

/// `DELETE {edge}/cloud/devices/{deviceId}`.
async fn revoke(
    http: &reqwest::Client,
    edge: &EdgeConfig,
    device_id: &str,
) -> Result<(), CloudError> {
    let url = format!(
        "{}/cloud/devices/{device_id}",
        edge.url.trim_end_matches('/')
    );
    let response = http
        .delete(&url)
        .bearer_auth(bearer(edge).await?)
        .send()
        .await
        .map_err(|e| CloudError::Network("the Zeron edge".into(), e.to_string()))?;
    if response.status().is_success() || response.status().as_u16() == 404 {
        Ok(())
    } else {
        Err(CloudError::Other(format!(
            "revoking the box's device failed ({})",
            response.status().as_u16()
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_file_store_round_trips_owner_only() {
        let dir = tempfile::tempdir().unwrap();
        let store = CloudTokenStore::file(dir.path().join("cloud-credentials.json"));
        assert_eq!(store.load().await.unwrap(), None);
        let secrets = CloudSecrets {
            api_token: "tok".into(),
            account_id: Some("acc".into()),
            account_name: Some("A".into()),
            credentials: BTreeMap::from([("box-1".to_string(), "cred".to_string())]),
            pending: None,
        };
        store.save(&secrets).await.unwrap();
        assert_eq!(store.load().await.unwrap(), Some(secrets.clone()));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.path().join("cloud-credentials.json"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        store.clear().await;
        assert_eq!(store.load().await.unwrap(), None);
    }

    #[test]
    fn the_keychain_service_is_per_data_dir() {
        let a = CloudTokenStore::detect(Path::new("/a"));
        let b = CloudTokenStore::detect(Path::new("/b"));
        assert_eq!(a.keychain_service.is_some(), cfg!(target_os = "macos"));
        if cfg!(target_os = "macos") {
            assert_ne!(a.keychain_service, b.keychain_service);
        }
        assert_eq!(a.file, Path::new("/a/cloud-credentials.json"));
    }
}
