use crate::{Error, Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, env, path::PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Policy {
    pub repository: String,
    pub maintainers: Vec<String>,
    pub triagers: Vec<String>,
    pub reviewers: Vec<String>,
    pub validators: Vec<String>,
    pub required_checks: Vec<String>,
    pub claim_hours: f64,
    pub decision_days: i64,
}
impl Policy {
    pub fn roles(&self, actor: &str) -> Vec<&'static str> {
        if actor.is_empty() {
            return vec![];
        }
        if self.maintainers.iter().any(|a| a == actor) {
            return vec!["maintainer", "triager", "reviewer", "validator"];
        }
        [
            ("triager", &self.triagers),
            ("reviewer", &self.reviewers),
            ("validator", &self.validators),
        ]
        .into_iter()
        .filter_map(|(role, actors)| actors.iter().any(|a| a == actor).then_some(role))
        .collect()
    }
}
#[derive(Clone)]
pub struct Config {
    pub demo: bool,
    pub policy: Policy,
    pub public_url: String,
    pub host: String,
    pub port: u16,
    pub data_path: PathBuf,
    pub writeback: bool,
    pub app_id: String,
    pub installation_id: String,
    pub private_key_path: String,
    pub webhook_secret: String,
    pub client_id: String,
    pub client_secret: String,
    pub read_token: String,
    pub agent_tokens: BTreeMap<String, String>,
    pub api_url: String,
}
pub fn hash(value: impl AsRef<[u8]>) -> String {
    hex::encode(Sha256::digest(value.as_ref()))
}
pub fn secret() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}
impl Config {
    pub fn load(demo: bool) -> Result<Self> {
        let get = |key: &str| env::var(key).unwrap_or_default();
        let default = |key: &str, fallback: &str| {
            let value = get(key);
            if value.is_empty() {
                fallback.to_owned()
            } else {
                value
            }
        };
        let policy = if demo {
            crate::seed::policy()
        } else {
            serde_json::from_slice(&std::fs::read(default("POLICY_PATH", "policy.json"))?)?
        };
        let public_url = default("PUBLIC_URL", "http://localhost:3080")
            .trim_end_matches('/')
            .to_owned();
        let tokens: BTreeMap<String, String> =
            serde_json::from_str(&default("AGENT_TOKENS", "{}"))?;
        ensure(
            tokens
                .iter()
                .all(|(token, actor)| token.len() >= 32 && !actor.is_empty()),
            500,
            "Agent tokens must have at least 32 characters and map to an accountable login.",
        )?;
        let config = Self {
            demo,
            policy,
            public_url,
            host: default("HOST", "127.0.0.1"),
            port: default("PORT", "3080")
                .parse()
                .map_err(|_| Error::new(500, "Invalid PORT."))?,
            data_path: default(
                "DATA_PATH",
                if demo {
                    "data/demo.sqlite"
                } else {
                    "data/review.sqlite"
                },
            )
            .into(),
            writeback: !demo && get("GITHUB_WRITEBACK") == "true",
            app_id: get("GITHUB_APP_ID"),
            installation_id: get("GITHUB_INSTALLATION_ID"),
            private_key_path: get("GITHUB_PRIVATE_KEY_PATH"),
            webhook_secret: get("GITHUB_WEBHOOK_SECRET"),
            client_id: get("GITHUB_CLIENT_ID"),
            client_secret: get("GITHUB_CLIENT_SECRET"),
            read_token: get("GITHUB_READ_TOKEN"),
            agent_tokens: tokens
                .into_iter()
                .map(|(token, actor)| (hash(token), actor))
                .collect(),
            api_url: "https://api.github.com".into(),
        };
        config.validate()?;
        Ok(config)
    }
    pub fn validate(&self) -> Result<()> {
        let url = reqwest::Url::parse(&self.public_url).map_err(Error::internal)?;
        ensure(
            ["http", "https"].contains(&url.scheme())
                && url.origin().ascii_serialization() == self.public_url,
            500,
            "PUBLIC_URL must be an HTTP(S) origin.",
        )?;
        ensure(
            self.demo
                || url.scheme() == "https"
                || matches!(url.host_str(), Some("localhost" | "127.0.0.1")),
            500,
            "Live public deployments require HTTPS.",
        )?;
        let segments: Vec<_> = self.policy.repository.split('/').collect();
        ensure(
            segments.len() == 2
                && segments.iter().all(|p| {
                    !p.is_empty()
                        && p.bytes()
                            .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))
                }),
            500,
            "Invalid policy repository.",
        )?;
        ensure(
            self.policy.claim_hours.is_finite()
                && self.policy.claim_hours > 0.0
                && self.policy.claim_hours <= 168.0
                && self.policy.decision_days > 0,
            500,
            "Invalid policy durations.",
        )?;
        if !self.demo {
            ensure(
                !self.policy.maintainers.is_empty(),
                500,
                "Configure at least one maintainer before starting live mode.",
            )?;
            ensure(
                !self.policy.required_checks.is_empty()
                    && !self
                        .policy
                        .required_checks
                        .iter()
                        .any(|c| c == "zeron-review/readiness"),
                500,
                "Configure independent required CI checks.",
            )?;
        }
        if self.writeback {
            ensure(
                [
                    &self.app_id,
                    &self.installation_id,
                    &self.private_key_path,
                    &self.webhook_secret,
                ]
                .iter()
                .all(|s| !s.is_empty()),
                500,
                "Writeback requires a configured GitHub App and webhook secret.",
            )?;
        }
        Ok(())
    }
}
