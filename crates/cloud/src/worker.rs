//! The Worker template and a client for its signed box routes
//! (docs/cloud.md §2). Any of the user's engines can call these: the routes
//! need only the box record (`workerUrl` + `wakeKey`), not the API token.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use zeron_proto::{CloudBox, CloudBoxState};

use crate::error::CloudError;
use crate::sign;

/// The Worker's script name in the user's account.
pub const SCRIPT_NAME: &str = "zeron-cloud";
/// The main module's name in the upload (`main_module`).
pub const MODULE_NAME: &str = "worker.js";
/// `cloud/worker/worker.js`, uploaded as-is: the entrypoints.
pub const WORKER_JS: &str = include_str!("../../../cloud/worker/worker.js");
/// `cloud/worker/core.js`, uploaded next to it: the pure helpers.
pub const CORE_JS: &str = include_str!("../../../cloud/worker/core.js");
/// The Worker's modules, main first: `(name, source)`.
pub const MODULES: [(&str, &str); 2] = [(MODULE_NAME, WORKER_JS), ("core.js", CORE_JS)];
/// Compatibility date the Worker is uploaded with (`ctx.exports` is on by
/// default from 2025-11-17).
pub const COMPATIBILITY_DATE: &str = "2026-09-01";
/// The Durable Object class that owns a box's container.
pub const BOX_CLASS: &str = "ZeronBox";
/// The secret holding every box's keys: `{deviceId: {wakeKey, credential, idleMinutes}}`.
pub const BOXES_SECRET: &str = "ZERON_BOXES";

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// A box route's reply. `wake`/`stop` fill only `state`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BoxStatus {
    pub state: CloudBoxState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_activity_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub busy: Option<bool>,
}

/// `purge` deletes a box's checkpoints a page at a time.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PurgeReply {
    #[serde(default)]
    pub deleted: u64,
    pub done: bool,
}

/// One entry of the `ZERON_BOXES` secret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BoxSecret {
    pub wake_key: String,
    pub credential: String,
    pub idle_minutes: u32,
}

#[derive(Clone)]
pub struct WorkerClient {
    http: reqwest::Client,
}

impl Default for WorkerClient {
    fn default() -> Self {
        Self::new()
    }
}

impl WorkerClient {
    pub fn new() -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(REQUEST_TIMEOUT)
                .build()
                .unwrap_or_default(),
        }
    }

    pub fn with_client(http: reqwest::Client) -> Self {
        Self { http }
    }

    /// Start the box's container if it's stopped.
    pub async fn wake(&self, record: &CloudBox) -> Result<BoxStatus, CloudError> {
        self.call(record, "wake").await
    }

    /// Final checkpoint, then stop the container.
    pub async fn stop(&self, record: &CloudBox) -> Result<BoxStatus, CloudError> {
        self.call(record, "stop").await
    }

    pub async fn status(&self, record: &CloudBox) -> Result<BoxStatus, CloudError> {
        self.call(record, "status").await
    }

    /// Delete one page of the box's checkpoints (`boxes/{deviceId}/`).
    pub async fn purge(&self, record: &CloudBox) -> Result<PurgeReply, CloudError> {
        self.call(record, "purge").await
    }

    async fn call<T: serde::de::DeserializeOwned>(
        &self,
        record: &CloudBox,
        action: &str,
    ) -> Result<T, CloudError> {
        let path = format!("/v1/boxes/{}/{action}", record.id);
        let url = format!("{}{path}", record.worker_url.trim_end_matches('/'));
        let body = b"{}".to_vec();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or_default();
        let headers = sign::signed_headers(&record.wake_key, "POST", &path, &body, now)
            .map_err(|e| CloudError::Other(format!("Can't sign the request: {e}")))?;
        let mut request = self
            .http
            .post(&url)
            .header("content-type", "application/json")
            .body(body);
        for (name, value) in headers {
            request = request.header(name, value);
        }
        let response = request
            .send()
            .await
            .map_err(|e| CloudError::Network(record.worker_url.clone(), e.to_string()))?;
        let status = response.status().as_u16();
        let text = response.text().await.unwrap_or_default();
        match status {
            200..=299 => serde_json::from_str(&text).map_err(|e| {
                CloudError::Other(format!(
                    "The cloud box sent an unexpected reply to {action}: {e}"
                ))
            }),
            401 | 403 => Err(CloudError::BoxUnauthorized),
            404 => Err(CloudError::BoxUnknown),
            _ => Err(CloudError::Api {
                path,
                status,
                code: None,
                message: worker_error(&text),
            }),
        }
    }
}

fn worker_error(text: &str) -> String {
    serde_json::from_str::<serde_json::Value>(text)
        .ok()
        .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(str::to_owned))
        .unwrap_or_else(|| text.chars().take(300).collect())
}
