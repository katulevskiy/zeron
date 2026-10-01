//! The checkpoint store client: plain HTTP against `$ZERON_CHECKPOINT_URL`
//! (docs/cloud.md §5). In a box that is the Worker's R2 gateway, which
//! confines every key to `boxes/{deviceId}/`; tests serve the same routes
//! from an in-process fake.
//!
//! | Request | Meaning |
//! | --- | --- |
//! | `PUT/HEAD/GET /objects/{sha256}` | Objects (idempotent) |
//! | `PUT/GET /manifests/{seq}.json` | Manifests |
//! | `PUT/GET /HEAD` | `{"seq": n}`, written last |

use std::time::Duration;

use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};

/// Attempts per request; transport errors and 5xx answers are retried.
const ATTEMPTS: u32 = 4;

#[derive(Clone)]
pub struct CheckpointStore {
    base: String,
    client: reqwest::Client,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Head {
    pub seq: u64,
}

impl CheckpointStore {
    pub fn new(base: impl Into<String>) -> Self {
        let base = base.into().trim_end_matches('/').to_string();
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(15))
            // A 64 MiB object over a slow link; the gateway streams to R2.
            .timeout(Duration::from_secs(600))
            .build()
            .unwrap_or_default();
        Self { base, client }
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    pub async fn has_object(&self, sha: &str) -> anyhow::Result<bool> {
        let url = format!("{}/objects/{sha}", self.base);
        let status = self
            .send(|| self.client.head(&url))
            .await
            .with_context(|| format!("HEAD object {sha}"))?
            .status();
        match status.as_u16() {
            200 | 204 => Ok(true),
            404 => Ok(false),
            code => bail!("HEAD object {sha}: HTTP {code}"),
        }
    }

    pub async fn put_object(&self, sha: &str, body: Vec<u8>) -> anyhow::Result<()> {
        let url = format!("{}/objects/{sha}", self.base);
        self.put(&url, body, "application/zstd")
            .await
            .with_context(|| format!("PUT object {sha}"))
    }

    pub async fn get_object(&self, sha: &str) -> anyhow::Result<Vec<u8>> {
        let url = format!("{}/objects/{sha}", self.base);
        self.get(&url)
            .await
            .with_context(|| format!("GET object {sha}"))?
            .with_context(|| format!("object {sha} is missing from the store"))
    }

    pub async fn put_manifest(&self, seq: u64, body: Vec<u8>) -> anyhow::Result<()> {
        let url = format!("{}/manifests/{seq}.json", self.base);
        self.put(&url, body, "application/json")
            .await
            .with_context(|| format!("PUT manifest {seq}"))
    }

    pub async fn get_manifest(&self, seq: u64) -> anyhow::Result<Vec<u8>> {
        let url = format!("{}/manifests/{seq}.json", self.base);
        self.get(&url)
            .await
            .with_context(|| format!("GET manifest {seq}"))?
            .with_context(|| format!("manifest {seq} is missing from the store"))
    }

    pub async fn put_head(&self, head: Head) -> anyhow::Result<()> {
        let url = format!("{}/HEAD", self.base);
        let body = serde_json::to_vec(&head)?;
        self.put(&url, body, "application/json")
            .await
            .context("PUT HEAD")
    }

    /// `None` on a store that has never been checkpointed to.
    pub async fn get_head(&self) -> anyhow::Result<Option<Head>> {
        let url = format!("{}/HEAD", self.base);
        let Some(body) = self.get(&url).await.context("GET HEAD")? else {
            return Ok(None);
        };
        Ok(Some(
            serde_json::from_slice(&body).context("HEAD is not {\"seq\": n}")?,
        ))
    }

    async fn put(&self, url: &str, body: Vec<u8>, content_type: &str) -> anyhow::Result<()> {
        let body = bytes::Bytes::from(body);
        let response = self
            .send(|| {
                self.client
                    .put(url)
                    .header("content-type", content_type)
                    .body(body.clone())
            })
            .await?;
        if !response.status().is_success() {
            bail!("HTTP {}", response.status().as_u16());
        }
        Ok(())
    }

    async fn get(&self, url: &str) -> anyhow::Result<Option<Vec<u8>>> {
        let response = self.send(|| self.client.get(url)).await?;
        match response.status().as_u16() {
            404 => Ok(None),
            code if (200..300).contains(&code) => Ok(Some(response.bytes().await?.to_vec())),
            code => bail!("HTTP {code}"),
        }
    }

    /// Send with retries: transport errors and 5xx back off and try again;
    /// any other answer is returned to the caller.
    async fn send(
        &self,
        build: impl Fn() -> reqwest::RequestBuilder,
    ) -> anyhow::Result<reqwest::Response> {
        let mut delay = Duration::from_millis(250);
        let mut attempt = 1;
        loop {
            let outcome = build().send().await;
            let retry = match &outcome {
                Ok(response) => response.status().is_server_error(),
                Err(_) => true,
            };
            if !retry || attempt >= ATTEMPTS {
                return Ok(outcome?);
            }
            tracing::debug!(attempt, "checkpoint store: retrying");
            tokio::time::sleep(delay).await;
            delay *= 2;
            attempt += 1;
        }
    }
}
