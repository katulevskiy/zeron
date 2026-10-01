//! Cloud boxes: Zeron devices running in a container in the user's own cloud
//! account (docs/cloud.md). One synced registry row per box (kind
//! `cloudBoxes`), private to the user.

use serde::{Deserialize, Serialize};

/// Where a box stands, as last recorded by any of the user's devices.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CloudBoxState {
    #[default]
    Asleep,
    Waking,
    Running,
    Checkpointing,
    Error,
}

impl CloudBoxState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Asleep => "asleep",
            Self::Waking => "waking",
            Self::Running => "running",
            Self::Checkpointing => "checkpointing",
            Self::Error => "error",
        }
    }
}

/// The box record (docs/cloud.md §1). `id` is the box's device id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudBox {
    pub id: String,
    /// `cloudflare`.
    pub provider: String,
    pub name: String,
    pub account_id: String,
    /// `https://zeron-cloud.<sub>.workers.dev`.
    pub worker_url: String,
    /// Base64 of 32 random bytes: the HMAC key for the Worker's routes.
    pub wake_key: String,
    pub instance_type: String,
    pub idle_minutes: u32,
    /// Epoch ms.
    #[serde(default)]
    pub created_at: i64,
    #[serde(default)]
    pub state: CloudBoxState,
    /// Epoch ms of the last `state` change.
    #[serde(default)]
    pub state_at: i64,
    #[serde(default)]
    pub error: Option<String>,
}

/// `CloudConnect`: store a Cloudflare API token on this device after checking it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudConnectParams {
    pub api_token: String,
    /// Needed only when the token can see more than one account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudConnectReply {
    pub account_id: String,
    pub account_name: String,
}

/// `CloudProvision`: create a box (idempotent).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudProvisionParams {
    pub name: String,
    pub instance_type: String,
    pub idle_minutes: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
}

/// `CloudBoxes`: the records plus whether this device holds a token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudBoxesReply {
    pub boxes: Vec<CloudBox>,
    /// This device holds a Cloudflare API token (it can provision).
    pub connected: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
}

/// `CloudWake` / `CloudStop`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudBoxParams {
    pub device_id: String,
}

/// `CloudDestroy`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudDestroyParams {
    pub device_id: String,
    /// Keep the box's checkpoints in R2.
    #[serde(default)]
    pub keep_data: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_round_trips_in_the_documented_shape() {
        let json = serde_json::json!({
            "id": "dev-1", "provider": "cloudflare", "name": "Cloud",
            "accountId": "acc", "workerUrl": "https://zeron-cloud.sub.workers.dev",
            "wakeKey": "a2V5", "instanceType": "standard-2",
            "idleMinutes": 15, "createdAt": 1, "state": "checkpointing",
            "stateAt": 2, "error": null
        });
        let record: CloudBox = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(record.state, CloudBoxState::Checkpointing);
        assert_eq!(serde_json::to_value(&record).unwrap(), json);
    }
}
