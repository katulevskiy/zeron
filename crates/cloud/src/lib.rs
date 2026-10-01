//! zeron-cloud — cloud boxes in the user's own cloud account (docs/cloud.md).
//!
//! - [`CloudProvider`]: validate a token, provision a box (idempotent),
//!   upgrade, wake/stop/status through the Worker's signed routes, destroy,
//!   estimate. [`cloudflare::Cloudflare`] implements it over the plain
//!   Cloudflare REST API (no wrangler, Docker or Node needed).
//! - [`sign`]: the HMAC request signer/verifier for the Worker's routes.
//! - [`worker`]: the Worker template (`cloud/worker/`), embedded, and a
//!   client for its routes that needs only the box record.
//! - Box records are [`zeron_proto::CloudBox`] (registry kind `cloudBoxes`).

pub mod cloudflare;
pub mod error;
pub mod estimate;
#[cfg(any(test, feature = "mock"))]
pub mod mock;
pub mod multipart;
pub mod sign;
pub mod worker;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

pub use error::{CloudError, Permission};
pub use estimate::Estimate;
pub use worker::{BoxStatus, WorkerClient};
pub use zeron_proto::{CloudBox, CloudBoxState};

/// The account a token reaches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub id: String,
    pub name: String,
}

/// A device minted for the box by the Zeron edge (docs/cloud.md §6), plus the
/// box's fresh wake key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Enrollment {
    pub device_id: String,
    pub credential: String,
    pub wake_key: String,
}

/// Enrolment is the engine's business (it holds the user's edge session);
/// the provider asks for it at step 2 of provisioning.
#[async_trait]
pub trait Enroller: Send + Sync {
    async fn enroll(&self, name: &str) -> Result<Enrollment, CloudError>;
}

/// A box the account already has. Its credential rebuilds the Worker's
/// `ZERON_BOXES` secret; `None` when this device doesn't know it.
#[derive(Debug, Clone)]
pub struct ExistingBox {
    pub record: CloudBox,
    pub credential: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ProvisionRequest {
    pub name: String,
    pub instance_type: String,
    pub idle_minutes: u32,
    pub region: Option<String>,
    /// The Zeron edge the box's engine dials.
    pub edge_url: String,
    /// Container image, e.g. `docker.io/zeronsh/zeron-cloud:0.2.99`.
    pub image: String,
    /// The user's other boxes (any account; the provider keeps those on the
    /// same account).
    pub existing: Vec<ExistingBox>,
}

#[derive(Debug, Clone)]
pub struct Provisioned {
    /// The record to write (`cloudBoxes`), state `asleep`.
    pub record: CloudBox,
    pub credential: String,
}

#[derive(Debug, Clone)]
pub struct UpgradeRequest {
    pub account_id: String,
    pub edge_url: String,
    pub image: String,
    pub instance_type: String,
    pub region: Option<String>,
    /// How many boxes the account has (the app's `max_instances`).
    pub boxes: usize,
}

#[async_trait]
pub trait CloudProvider: Send + Sync {
    /// `cloudflare`.
    fn id(&self) -> &'static str;

    /// Check the token and that the account can run boxes.
    async fn validate(&self) -> Result<Account, CloudError>;

    /// Create a box. Every step tolerates having run before, so a failed
    /// attempt can simply be retried.
    async fn provision(
        &self,
        request: &ProvisionRequest,
        enroller: &dyn Enroller,
    ) -> Result<Provisioned, CloudError>;

    /// Upload the current Worker and roll the container app to `image`.
    async fn upgrade(&self, request: &UpgradeRequest) -> Result<(), CloudError>;

    async fn wake(&self, record: &CloudBox) -> Result<BoxStatus, CloudError>;
    async fn stop(&self, record: &CloudBox) -> Result<BoxStatus, CloudError>;
    async fn status(&self, record: &CloudBox) -> Result<BoxStatus, CloudError>;

    /// Remove a box. The last box of an account takes the Worker and
    /// container app with it; `keep_data` keeps its checkpoints (and the
    /// bucket).
    async fn destroy(
        &self,
        record: &CloudBox,
        existing: &[ExistingBox],
        keep_data: bool,
    ) -> Result<(), CloudError>;

    /// A month's cost for `active_hours_per_month` of running.
    fn estimate(&self, instance_type: &str, active_hours_per_month: f64) -> Option<Estimate>;
}

/// The release image for this build.
pub fn default_image() -> String {
    format!(
        "docker.io/zeronsh/zeron-cloud:{}",
        env!("CARGO_PKG_VERSION")
    )
}
