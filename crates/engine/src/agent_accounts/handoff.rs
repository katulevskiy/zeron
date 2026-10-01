//! Login handoff — a live login carried to another of the account's devices
//! (session move, `docs/plans/2026-09-30-agent-mobility-and-policy.md` Part 1,
//! "Credentials (decision 3)").
//!
//! When a chat moves from engine A to engine B, B uses its own login for the
//! chat's agent if it has one. If it has none, A exports its live login
//! ([`AgentAccounts::export_login`]), the bytes travel over the move's
//! encrypted device-to-device channel (not this module's business), and B
//! installs them ([`AgentAccounts::import_login`]) as a normal slot — listed
//! in Settings → agent accounts like any other — that becomes B's live login.
//! An import NEVER replaces a login B already has; A keeps its own.
//!
//! The handoff carries the slot payload, never a platform blob: Claude's
//! credentials are the portable `.credentials.json` shape whether A read them
//! from the macOS Keychain or the file, and B writes them wherever its own
//! Claude Code reads them (Keychain on macOS, the file elsewhere). Only the
//! account's own tokens travel — Claude's machine-shared siblings (MCP OAuth,
//! plugin secrets, the trusted-device token) stay on A, and B keeps its own
//! `userID`.
//!
//! | agent       | handoff | payload                                          |
//! |-------------|---------|--------------------------------------------------|
//! | Claude Code | yes     | `{claudeAiOauth}` + `oauthAccount`               |
//! | Codex       | yes     | `auth.json` verbatim (ChatGPT tokens or API key) |
//! | Cursor      | yes     | the SDK's `auth.json` (an expired key stays home) |
//! | Grok        | yes     | the live issuer's token set + its map key        |
//! | Devin       | yes     | `credentials.toml`, as JSON                      |
//! | OpenCode/Pi | yes¹    | each identified live OAuth entry + its store key |
//! | Hermes      | no      | Hermes owns its pool (see [`super::stores`])     |
//! | Antigravity | no      | its token is never read                          |
//!
//! ¹ These keep one login per model provider. The handoff carries every
//! managed provider login A has; B installs those it has no entry for and
//! leaves the rest of its store byte-for-byte.
//!
//! **Refresh tokens rotate.** An OAuth refresh token is commonly single-use:
//! whichever device refreshes first gets the new pair, and the other's copy
//! of the grant may then be rejected. Two devices sharing one handed-over
//! login can therefore leave one of them needing to sign in again later. The
//! move UI says so; nothing here can prevent it.
//!
//! Secrets: nothing here logs credential contents (only the agent and a
//! count), error messages never echo a payload or a parser's view of it,
//! handoff payload strings are zeroed on drop, serialized bytes come back
//! [`Zeroizing`], and every file is written 0600 via an atomic rename.

use zeroize::{Zeroize, Zeroizing};

use super::stores::{
    Upstream, cli_name, devin_toml, keyed_accounts, oauth_entry, openai_detected,
    parse_devin_credentials, parse_grok_auth, upstream_of,
};
use super::*;

/// The handoff format this build writes and reads.
pub const LOGIN_HANDOFF_VERSION: u32 = 1;

/// A live login packaged to travel to another of the account's devices.
/// Serialized as camelCase JSON; see the module docs for the payloads.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginHandoff {
    /// [`LOGIN_HANDOFF_VERSION`] when written.
    pub v: u32,
    pub harness: HarnessId,
    /// Epoch millis on the exporting device.
    pub exported_at: i64,
    /// Exactly one, except OpenCode / Pi: one per model provider.
    pub logins: Vec<HandedLogin>,
}

/// One login inside a [`LoginHandoff`]: who it is (for display — "B will use
/// a@example.com") plus the slot payload.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HandedLogin {
    /// The provider-side identity its slot is keyed by.
    pub account_key: String,
    pub email: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub organization: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    pub auth_kind: AgentAuthKind,
    /// Grok: the issuer entry; OpenCode / Pi: the model provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub store_key: Option<String>,
    payload: HandoffPayload,
}

/// What a slot stores — all JSON (Devin's TOML travels as its JSON form), so
/// nothing needs base64.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HandoffPayload {
    credentials: serde_json::Value,
    /// Claude only: `{oauthAccount}`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    claude_config: Option<serde_json::Value>,
}

impl Drop for HandoffPayload {
    fn drop(&mut self) {
        scrub(&mut self.credentials);
    }
}

/// Zero every string in `value` in place (keys are names, not secrets).
fn scrub(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::String(s) => s.zeroize(),
        serde_json::Value::Array(items) => items.iter_mut().for_each(scrub),
        serde_json::Value::Object(map) => map.values_mut().for_each(scrub),
        _ => {}
    }
}

// Hand-written so a stray `{:?}` can never print a token.
impl std::fmt::Debug for LoginHandoff {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoginHandoff")
            .field("v", &self.v)
            .field("harness", &self.harness)
            .field("exported_at", &self.exported_at)
            .field("logins", &self.logins)
            .finish()
    }
}

impl std::fmt::Debug for HandedLogin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HandedLogin")
            .field("account_key", &self.account_key)
            .field("email", &self.email)
            .field("store_key", &self.store_key)
            .field("payload", &"<redacted>")
            .finish()
    }
}

/// What [`AgentAccounts::import_login`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginImport {
    /// The handed login is this device's live login now (and a saved slot).
    Installed,
    /// This device already has a login for the agent — it keeps its own and
    /// nothing was written.
    AlreadySignedIn,
    /// This device can't take this handoff (agent or format); says why.
    Unsupported(String),
}

/// Just enough of a handoff to refuse it before parsing the payload.
#[derive(Deserialize)]
struct Header {
    v: u32,
    harness: HarnessId,
}

impl LoginHandoff {
    /// The login B would use, for the UI ("a@example.com").
    pub fn label(&self) -> Option<&str> {
        self.logins.first().map(|login| login.email.as_str())
    }

    /// The handoff as JSON bytes, zeroed when dropped.
    pub fn to_json(&self) -> Result<Zeroizing<Vec<u8>>, EngineError> {
        serde_json::to_vec(self)
            .map(Zeroizing::new)
            .map_err(|_| EngineError::Other("Couldn't package the login.".into()))
    }

    /// Parse handoff bytes, refusing a newer format or a login for any agent
    /// but `expected` before the payload is even read. Parser errors are not
    /// echoed — serde quotes the offending value, which may be a token.
    pub fn from_json(bytes: &[u8], expected: HarnessId) -> Result<Self, EngineError> {
        let unreadable = || EngineError::Other("The handed-over login is unreadable.".into());
        let header: Header = serde_json::from_slice(bytes).map_err(|_| unreadable())?;
        if header.v != LOGIN_HANDOFF_VERSION {
            return Err(EngineError::Other(format!(
                "The handed-over login uses format {} — this Zeron reads format \
                 {LOGIN_HANDOFF_VERSION}. Update both devices.",
                header.v
            )));
        }
        if header.harness != expected {
            return Err(EngineError::Other(format!(
                "The handed-over login is for {}, not {}.",
                cli_name(header.harness),
                cli_name(expected)
            )));
        }
        serde_json::from_slice(bytes).map_err(|_| unreadable())
    }

    /// Write the handoff to `path`: 0600 from birth, atomic rename. The
    /// directory is the caller's (a private staging dir).
    pub fn save(&self, path: &Path) -> Result<(), EngineError> {
        write_file_atomic(path, &self.to_json()?, true)
    }

    /// [`Self::from_json`] of the file at `path`.
    pub fn load(path: &Path, expected: HarnessId) -> Result<Self, EngineError> {
        let bytes = Zeroizing::new(std::fs::read(path)?);
        Self::from_json(&bytes, expected)
    }
}

/// Why `harness` can't hand its login over, or `None` when it can.
fn unsupported_reason(harness: HarnessId) -> Option<String> {
    match harness {
        HarnessId::ClaudeCode
        | HarnessId::Codex
        | HarnessId::Cursor
        | HarnessId::Grok
        | HarnessId::Devin
        | HarnessId::Opencode
        | HarnessId::Pi => None,
        HarnessId::Hermes => Some(
            "Hermes keeps its logins in its own credential pool — sign in with `hermes auth` \
             on this device."
                .into(),
        ),
        HarnessId::Antigravity => Some(
            "Antigravity's login can't be handed to another device — sign in on this device."
                .into(),
        ),
        HarnessId::Mock => Some("The mock agent has no login.".into()),
    }
}

/// A Claude credential blob a run can use: account tokens with a refresh
/// token, or an access token not yet past its expiry.
fn claude_oauth_usable(credentials: &serde_json::Value) -> bool {
    let Some(oauth) = credentials.get("claudeAiOauth") else {
        return false;
    };
    str_field(oauth, "refreshToken").is_some()
        || (str_field(oauth, "accessToken").is_some()
            && oauth
                .get("expiresAt")
                .and_then(|v| v.as_i64())
                .is_none_or(|ms| ms > now_ms()))
}

/// A Codex `auth.json` that signs in: ChatGPT tokens or an API key.
fn codex_auth_usable(auth: &serde_json::Value) -> bool {
    let tokens = auth.get("tokens");
    tokens.is_some_and(|t| {
        str_field(t, "refresh_token").is_some() || str_field(t, "access_token").is_some()
    }) || str_field(auth, "OPENAI_API_KEY").is_some()
}

/// A JSON store file: `Some(None)` when missing, `None` when it exists but
/// doesn't parse (can't tell — and never overwritten).
fn json_store(file: &Path) -> Option<Option<serde_json::Value>> {
    if !file.exists() {
        return Some(None);
    }
    read_json(file).map(Some)
}

impl HandedLogin {
    fn from_detected(detected: Detected, credentials: serde_json::Value) -> Self {
        Self {
            account_key: detected.account_key,
            email: detected.profile.email,
            display_name: detected.profile.display_name,
            organization: detected.profile.organization,
            plan: detected.profile.plan,
            auth_kind: detected.profile.auth_kind,
            store_key: detected.store_key,
            payload: HandoffPayload {
                credentials,
                claude_config: detected.claude_config,
            },
        }
    }

    /// The slot this login becomes on the importing device — every field
    /// re-derived from the payload and checked against the claimed identity,
    /// so a payload for another agent (or garbage) fails here, before
    /// anything is written.
    fn checked_slot(&self, harness: HarnessId) -> Result<Slot, EngineError> {
        let bad = |why: &str| {
            EngineError::Other(format!(
                "The handed-over {} login can't be used: {why}.",
                cli_name(harness)
            ))
        };
        let creds = &self.payload.credentials;
        if !creds.is_object() {
            return Err(bad("it carries no credentials"));
        }
        let (account_key, credentials, claude_config, store_key) = match harness {
            HarnessId::ClaudeCode => {
                // Only the account's tokens — never A's machine-shared fields.
                let oauth = creds
                    .get("claudeAiOauth")
                    .filter(|v| v.is_object())
                    .ok_or_else(|| bad("it holds no Claude account tokens"))?;
                let credentials = serde_json::json!({ "claudeAiOauth": oauth });
                if !claude_oauth_usable(&credentials) {
                    return Err(bad("its tokens are missing or expired"));
                }
                let account = self
                    .payload
                    .claude_config
                    .as_ref()
                    .and_then(|c| c.get("oauthAccount"))
                    .filter(|v| v.is_object())
                    .ok_or_else(|| bad("it names no account"))?;
                let email =
                    str_field(account, "emailAddress").ok_or_else(|| bad("it names no account"))?;
                let key = str_field(account, "accountUuid").unwrap_or(email);
                let config = serde_json::json!({ "oauthAccount": account });
                (key, credentials, Some(config), None)
            }
            HarnessId::Codex => {
                let detected = parse_codex_auth(creds.clone())
                    .filter(|_| codex_auth_usable(creds))
                    .ok_or_else(|| bad("it isn't a Codex sign-in"))?;
                (detected.account_key, creds.clone(), None, None)
            }
            HarnessId::Cursor => {
                let detected =
                    parse_cursor_auth(creds.clone()).ok_or_else(|| bad("it holds no API key"))?;
                if !cursor_key_usable(creds) {
                    return Err(bad("its API key has expired"));
                }
                (detected.account_key, creds.clone(), None, None)
            }
            HarnessId::Grok => {
                let map_key = self
                    .store_key
                    .clone()
                    .ok_or_else(|| bad("it names no issuer"))?;
                let mut store = serde_json::Map::new();
                store.insert(map_key.clone(), creds.clone());
                let detected = parse_grok_auth(serde_json::Value::Object(store))
                    .ok_or_else(|| bad("it holds no token"))?;
                (detected.account_key, creds.clone(), None, Some(map_key))
            }
            HarnessId::Devin => {
                let detected =
                    parse_devin_credentials(creds.clone()).ok_or_else(|| bad("it holds no key"))?;
                // The live file is TOML: prove the payload serializes first.
                devin_toml(creds).map_err(|_| bad("its credentials don't form a valid file"))?;
                (detected.account_key, creds.clone(), None, None)
            }
            HarnessId::Opencode | HarnessId::Pi => {
                let store_key = self
                    .store_key
                    .clone()
                    .ok_or_else(|| bad("it names no model provider"))?;
                let upstream = upstream_of(harness, &store_key)
                    .ok_or_else(|| bad("it names a provider zeron doesn't manage"))?;
                if !oauth_entry(creds) {
                    return Err(bad("it isn't an OAuth login"));
                }
                let key = match upstream {
                    Upstream::OpenAi => {
                        openai_detected(&store_key, creds, None)
                            .ok_or_else(|| bad("it names no account"))?
                            .account_key
                    }
                    // Opaque tokens name no one offline: keep the exporter's
                    // identity, but only inside this provider's key space.
                    Upstream::Anthropic | Upstream::Copilot => {
                        if !self.account_key.starts_with(&format!("{store_key}:")) {
                            return Err(bad("its identity doesn't match its provider"));
                        }
                        self.account_key.clone()
                    }
                };
                (key, creds.clone(), None, Some(store_key))
            }
            other => {
                return Err(EngineError::Other(
                    unsupported_reason(other).unwrap_or_default(),
                ));
            }
        };
        if account_key != self.account_key {
            return Err(bad("its credentials belong to a different account"));
        }
        if self.email.trim().is_empty() {
            return Err(bad("it names no account"));
        }
        Ok(Slot {
            id: slot_id_for(harness, &account_key),
            harness,
            account_key,
            profile: SlotProfile {
                email: self.email.clone(),
                display_name: self.display_name.clone(),
                organization: self.organization.clone(),
                plan: self.plan.clone(),
                auth_kind: self.auth_kind,
            },
            credentials,
            claude_config,
            saved_at: now_ms(),
            created_at: None,
            store_key,
        })
    }
}

impl AgentAccounts {
    /// Whether this device's CLI for `harness` is signed in: a live login
    /// exists and its credentials are readable and usable. `None` = can't
    /// tell (the Keychain denied the read, the store doesn't parse, or the
    /// agent has no login). OpenCode / Pi: any login zeron manages. Logins
    /// supplied through the environment (`ANTHROPIC_API_KEY`, …) aren't seen.
    pub async fn signed_in(&self, harness: HarnessId) -> Result<Option<bool>, EngineError> {
        let _ops = self.inner.ops.lock().await;
        Ok(self.signed_in_locked(harness).await)
    }

    /// Caller holds [`Inner::ops`].
    async fn signed_in_locked(&self, harness: HarnessId) -> Option<bool> {
        let config = &self.inner.config;
        match harness {
            HarnessId::ClaudeCode => match self.read_claude_credentials().await {
                (Some(credentials), _) => Some(claude_oauth_usable(&credentials)),
                (None, Some(_denied)) => None,
                (None, None) => Some(false),
            },
            HarnessId::Codex => json_store(&config.codex_auth_file())
                .map(|auth| auth.is_some_and(|auth| codex_auth_usable(&auth))),
            HarnessId::Cursor => json_store(&config.cursor_sdk_auth_file)
                .map(|auth| auth.is_some_and(|auth| cursor_key_usable(&auth))),
            HarnessId::Grok => json_store(&config.grok_home.join("auth.json"))
                .map(|auth| auth.and_then(parse_grok_auth).is_some()),
            HarnessId::Devin => {
                let file = &config.devin_credentials_file;
                if !file.exists() {
                    return Some(false);
                }
                let text = Zeroizing::new(std::fs::read_to_string(file).ok()?);
                let credentials = stores::parse_devin_toml(&text)?;
                Some(parse_devin_credentials(credentials).is_some())
            }
            HarnessId::Opencode | HarnessId::Pi => {
                json_store(&self.keyed_file(harness)).map(|store| {
                    store.is_some_and(|store| {
                        keyed_accounts(harness)
                            .iter()
                            .any(|(key, _)| store.get(*key).is_some_and(oauth_entry))
                    })
                })
            }
            HarnessId::Hermes => Some(!self.hermes_slots().0.is_empty()),
            HarnessId::Antigravity => Some(self.detect_antigravity().await.is_some()),
            HarnessId::Mock => None,
        }
    }

    /// The live login for `harness`, packaged to travel to another of the
    /// account's devices (see the module docs for what each agent sends).
    /// `None` = no usable live login, unreadable credentials (the Keychain
    /// denied the read), or an agent whose login can't be handed over.
    ///
    /// The receiving device shares the grant from then on: an OAuth refresh
    /// token may rotate on whichever device refreshes first, and the other
    /// may have to sign in again later.
    pub async fn export_login(
        &self,
        harness: HarnessId,
    ) -> Result<Option<LoginHandoff>, EngineError> {
        if unsupported_reason(harness).is_some() {
            return Ok(None);
        }
        let _ops = self.inner.ops.lock().await;
        // The tokens a few seconds old at most — never a cached read.
        *lock(&self.inner.claude_credentials) = None;
        let logins: Vec<HandedLogin> = match harness {
            HarnessId::ClaudeCode => {
                let (detected, _) = self.detect_claude().await;
                detected
                    .and_then(|mut detected| {
                        let oauth = detected.credentials.take()?.get("claudeAiOauth")?.clone();
                        let credentials = serde_json::json!({ "claudeAiOauth": oauth });
                        if !claude_oauth_usable(&credentials) {
                            return None;
                        }
                        // The identity only: `userID` is per install.
                        detected.claude_config = detected
                            .claude_config
                            .as_ref()
                            .and_then(|c| c.get("oauthAccount"))
                            .map(|account| serde_json::json!({ "oauthAccount": account }));
                        Some(HandedLogin::from_detected(detected, credentials))
                    })
                    .into_iter()
                    .collect()
            }
            HarnessId::Codex => self.export_detected(
                self.detect_codex()
                    .filter(|d| d.credentials.as_ref().is_some_and(codex_auth_usable)),
            ),
            HarnessId::Cursor => {
                self.export_detected(self.detect_cursor().filter(|_| self.cursor_live_usable()))
            }
            HarnessId::Grok => self.export_detected(self.detect_grok()),
            HarnessId::Devin => self.export_detected(self.detect_devin().map(|mut detected| {
                // The key file names no one; the slot may know (a usage
                // probe wrote it back).
                if let Some(slot) =
                    self.read_slot(harness, &slot_id_for(harness, &detected.account_key))
                {
                    detected.profile = slot.profile;
                }
                detected
            })),
            HarnessId::Opencode | HarnessId::Pi => {
                let (resolved, _unidentified) = self.detect_keyed(harness).await;
                resolved
                    .into_iter()
                    .flat_map(|detected| self.export_detected(Some(detected)))
                    .collect()
            }
            _ => Vec::new(),
        };
        if logins.is_empty() {
            return Ok(None);
        }
        tracing::info!(
            agent = harness_slug(harness),
            logins = logins.len(),
            "exported a login for another device"
        );
        Ok(Some(LoginHandoff {
            v: LOGIN_HANDOFF_VERSION,
            harness,
            exported_at: now_ms(),
            logins,
        }))
    }

    fn export_detected(&self, detected: Option<Detected>) -> Vec<HandedLogin> {
        detected
            .and_then(|mut detected| {
                let credentials = detected.credentials.take()?;
                Some(HandedLogin::from_detected(detected, credentials))
            })
            .into_iter()
            .collect()
    }

    /// Install a login exported on another device — ONLY where this device
    /// has none for that agent (OpenCode / Pi: per model provider); an
    /// existing login is never replaced. The login is saved as a slot (so it
    /// lists like any other) and made live.
    ///
    /// Everything is validated before anything is written: the format
    /// version, that the agent can take a handoff (else
    /// [`LoginImport::Unsupported`]), and that each payload really is a login
    /// for the agent the handoff claims, belonging to the account it names
    /// (else an error). "Can't tell whether this device is signed in" is an
    /// error too — nothing is written then either.
    ///
    /// See [`Self::export_login`] for the shared-refresh-token caveat.
    pub async fn import_login(&self, handoff: &LoginHandoff) -> Result<LoginImport, EngineError> {
        let harness = handoff.harness;
        if handoff.v != LOGIN_HANDOFF_VERSION {
            return Ok(LoginImport::Unsupported(format!(
                "The handed-over login uses format {} — this Zeron reads format \
                 {LOGIN_HANDOFF_VERSION}. Update both devices.",
                handoff.v
            )));
        }
        if let Some(reason) = unsupported_reason(harness) {
            return Ok(LoginImport::Unsupported(reason));
        }
        let keyed = matches!(harness, HarnessId::Opencode | HarnessId::Pi);
        let expected_count = if keyed {
            1..=keyed_accounts(harness).len()
        } else {
            1..=1
        };
        if !expected_count.contains(&handoff.logins.len()) {
            return Err(EngineError::Other(format!(
                "The handed-over {} login is malformed.",
                cli_name(harness)
            )));
        }
        let slots = handoff
            .logins
            .iter()
            .map(|login| login.checked_slot(harness))
            .collect::<Result<Vec<Slot>, _>>()?;
        let mut store_keys = std::collections::HashSet::new();
        if keyed && !slots.iter().all(|s| store_keys.insert(s.store_key.clone())) {
            return Err(EngineError::Other(format!(
                "The handed-over {} login is malformed.",
                cli_name(harness)
            )));
        }

        let _ops = self.inner.ops.lock().await;
        *lock(&self.inner.claude_credentials) = None;
        let installed = if keyed {
            self.import_keyed(harness, &slots)?
        } else {
            self.import_single(harness, &slots[0]).await?
        };
        *lock(&self.inner.claude_credentials) = None;
        if installed == 0 {
            return Ok(LoginImport::AlreadySignedIn);
        }
        tracing::info!(
            agent = harness_slug(harness),
            logins = installed,
            "installed a login handed over by another device"
        );
        Ok(LoginImport::Installed)
    }

    /// A single-login agent: install `slot` unless a login is live. Returns
    /// how many logins went in (0 or 1). Caller holds [`Inner::ops`].
    async fn import_single(&self, harness: HarnessId, slot: &Slot) -> Result<usize, EngineError> {
        match self.signed_in_locked(harness).await {
            Some(true) => return Ok(0),
            Some(false) => {}
            None => {
                return Err(EngineError::Other(format!(
                    "Couldn't tell whether {} is signed in on this device — leaving its login \
                     alone.",
                    cli_name(harness)
                )));
            }
        }
        let mut slot = slot.clone();
        if harness == HarnessId::ClaudeCode {
            // `activate_claude` merges into ~/.claude.json and refuses one it
            // can't parse — check before the credentials go in, not after.
            let file = &self.inner.config.claude_config_file;
            let cfg = read_json(file);
            if cfg.is_none() && file.exists() {
                return Err(EngineError::Other(
                    "~/.claude.json exists but could not be parsed — not installing the \
                     handed-over login. Fix or remove the file and try again."
                        .into(),
                ));
            }
            // This install's own `userID` stays (it identifies the install,
            // not the account).
            if let (Some(user_id), Some(config)) = (
                cfg.as_ref()
                    .and_then(|c| c.get("userID"))
                    .filter(|v| v.is_string()),
                slot.claude_config.as_mut().and_then(|c| c.as_object_mut()),
            ) {
                config.insert("userID".into(), user_id.clone());
            }
        }
        // Whatever is there but unusable (an expired Cursor key, a blob
        // without account tokens) is snapshotted first — never stranded.
        self.list_locked(false).await?;
        match harness {
            HarnessId::ClaudeCode => self.activate_claude(&slot).await?,
            HarnessId::Codex => self.activate_codex(&slot)?,
            HarnessId::Cursor => self.write_cursor_auth(&slot.credentials)?,
            HarnessId::Grok => {
                self.write_grok_entry(slot.store_key.as_deref(), &slot.credentials)?
            }
            HarnessId::Devin => self.write_devin_credentials(&slot.credentials)?,
            _ => return Ok(0),
        }
        self.write_slot(&slot)?;
        Ok(1)
    }

    /// OpenCode / Pi: install each provider login this device has no entry
    /// for; every other entry stays as it is. Caller holds [`Inner::ops`].
    fn import_keyed(&self, harness: HarnessId, slots: &[Slot]) -> Result<usize, EngineError> {
        let file = self.keyed_file(harness);
        let Some(store) = json_store(&file) else {
            return Err(EngineError::Other(format!(
                "{} exists but could not be parsed — leaving it alone.",
                file.display()
            )));
        };
        let mut installed = 0;
        for slot in slots {
            let Some(key) = slot.store_key.as_deref() else {
                continue;
            };
            // ANY entry under this provider (an API key included) is the
            // user's choice — never replaced.
            if store.as_ref().is_some_and(|s| s.get(key).is_some()) {
                continue;
            }
            self.write_keyed_entry(harness, key, Some(&slot.credentials))?;
            self.write_slot(slot)?;
            installed += 1;
        }
        Ok(installed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, contents: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    fn jwt(claims: serde_json::Value) -> String {
        format!(
            "e30.{}.sig",
            BASE64_URL.encode(serde_json::to_vec(&claims).unwrap())
        )
    }

    /// Two devices, each with every provider pointed into its own temp root.
    struct Devices {
        _tmp: tempfile::TempDir,
        a_root: PathBuf,
        b_root: PathBuf,
        a: AgentAccounts,
        b: AgentAccounts,
    }

    fn devices() -> Devices {
        let tmp = tempfile::tempdir().unwrap();
        let a_root = tmp.path().join("a");
        let b_root = tmp.path().join("b");
        Devices {
            a: AgentAccounts::new(AgentAccountsConfig::isolated(&a_root)),
            b: AgentAccounts::new(AgentAccountsConfig::isolated(&b_root)),
            a_root,
            b_root,
            _tmp: tmp,
        }
    }

    fn cfg(root: &Path) -> AgentAccountsConfig {
        AgentAccountsConfig::isolated(root)
    }

    /// A signed-in Claude Code (file-based, as on Linux/Windows) with the
    /// machine-shared siblings Claude keeps in the same blob.
    fn live_claude(root: &Path, uuid: &str, email: &str, token: &str) {
        let config = cfg(root);
        write(
            &config.claude_config_file,
            &serde_json::json!({
                "oauthAccount": { "accountUuid": uuid, "emailAddress": email },
                "userID": format!("user-of-{}", root.display()),
            })
            .to_string(),
        );
        write(
            &config.claude_creds_file(),
            &serde_json::json!({
                "claudeAiOauth": {
                    "accessToken": token,
                    "refreshToken": format!("{token}-refresh"),
                    "expiresAt": 1,
                    "scopes": ["user:inference"],
                },
                "mcpOAuth": { "server": "a-only" },
                "trustedDeviceToken": "a-device",
            })
            .to_string(),
        );
    }

    fn codex_auth(email: &str, user: &str, token: &str) -> serde_json::Value {
        serde_json::json!({
            "OPENAI_API_KEY": null,
            "tokens": {
                "id_token": jwt(serde_json::json!({
                    "email": email,
                    "https://api.openai.com/auth": {
                        "chatgpt_account_id": "ws-1",
                        "chatgpt_user_id": user,
                        "chatgpt_plan_type": "plus",
                    },
                })),
                "access_token": token,
                "refresh_token": format!("{token}-refresh"),
                "account_id": "ws-1",
            },
            "last_refresh": "2026-09-30T00:00:00Z",
        })
    }

    fn live_codex(root: &Path, auth: &serde_json::Value) {
        write(&cfg(root).codex_auth_file(), &auth.to_string());
    }

    #[cfg(unix)]
    fn mode(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    /// Every file under `dir` (recursively) — "nothing was written" checks.
    fn files(dir: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let Ok(entries) = std::fs::read_dir(dir) else {
            return out;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                out.extend(files(&path));
            } else {
                out.push(path);
            }
        }
        out.sort();
        out
    }

    /// Export on A, through the bytes that travel, import on B.
    async fn hand_over(d: &Devices, harness: HarnessId) -> LoginImport {
        let handoff =
            d.a.export_login(harness)
                .await
                .unwrap()
                .expect("A has a login");
        let bytes = handoff.to_json().unwrap();
        let received = LoginHandoff::from_json(&bytes, harness).unwrap();
        d.b.import_login(&received).await.unwrap()
    }

    #[tokio::test]
    async fn a_claude_login_moves_to_a_device_without_one() {
        let d = devices();
        live_claude(&d.a_root, "acct-a", "a@example.com", "tok-a");
        // B: Claude Code installed, never signed in — but it has its own MCP
        // OAuth, its own install id and its own project history.
        let b = cfg(&d.b_root);
        write(
            &b.claude_config_file,
            &serde_json::json!({ "userID": "b-install", "projects": { "/keep": {} } }).to_string(),
        );
        write(
            &b.claude_creds_file(),
            &serde_json::json!({ "mcpOAuth": { "server": "b-only" } }).to_string(),
        );
        assert_eq!(
            d.a.signed_in(HarnessId::ClaudeCode).await.unwrap(),
            Some(true)
        );
        assert_eq!(
            d.b.signed_in(HarnessId::ClaudeCode).await.unwrap(),
            Some(false)
        );

        assert_eq!(
            hand_over(&d, HarnessId::ClaudeCode).await,
            LoginImport::Installed
        );

        let creds = read_json(&b.claude_creds_file()).unwrap();
        assert_eq!(creds["claudeAiOauth"]["accessToken"], "tok-a");
        assert_eq!(creds["claudeAiOauth"]["refreshToken"], "tok-a-refresh");
        // B's machine-shared fields stay B's; A's never travel.
        assert_eq!(creds["mcpOAuth"]["server"], "b-only");
        assert!(creds.get("trustedDeviceToken").is_none());
        let config = read_json(&b.claude_config_file).unwrap();
        assert_eq!(config["oauthAccount"]["emailAddress"], "a@example.com");
        assert_eq!(config["userID"], "b-install");
        assert!(config["projects"].get("/keep").is_some());
        #[cfg(unix)]
        assert_eq!(mode(&b.claude_creds_file()), 0o600);

        assert_eq!(
            d.b.signed_in(HarnessId::ClaudeCode).await.unwrap(),
            Some(true)
        );
        let rows = d.b.list(false).await.unwrap().accounts;
        let row = rows
            .iter()
            .find(|r| r.harness == HarnessId::ClaudeCode)
            .unwrap();
        assert!(row.active && row.switchable);
        assert_eq!(row.email.as_deref(), Some("a@example.com"));
        // A keeps its own login untouched.
        let a_creds = read_json(&cfg(&d.a_root).claude_creds_file()).unwrap();
        assert_eq!(a_creds["trustedDeviceToken"], "a-device");
    }

    #[tokio::test]
    async fn the_claude_payload_is_the_portable_credentials_file_shape() {
        let d = devices();
        live_claude(&d.a_root, "acct-a", "a@example.com", "tok-a");
        let handoff =
            d.a.export_login(HarnessId::ClaudeCode)
                .await
                .unwrap()
                .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&handoff.to_json().unwrap()).unwrap();
        assert_eq!(json["v"], 1);
        assert_eq!(json["harness"], "claude-code");
        assert!(json["exportedAt"].as_i64().unwrap() > 0);
        let login = &json["logins"][0];
        assert_eq!(login["accountKey"], "acct-a");
        assert_eq!(login["email"], "a@example.com");
        // Exactly `.credentials.json`'s account part — plain JSON strings,
        // the same whether A read it from the Keychain or the file.
        let credentials = login["payload"]["credentials"].as_object().unwrap();
        assert_eq!(
            credentials.keys().collect::<Vec<_>>(),
            vec!["claudeAiOauth"]
        );
        assert_eq!(credentials["claudeAiOauth"]["accessToken"], "tok-a");
        assert_eq!(
            login["payload"]["claudeConfig"],
            serde_json::json!({
                "oauthAccount": { "accountUuid": "acct-a", "emailAddress": "a@example.com" }
            })
        );
    }

    #[tokio::test]
    async fn a_codex_login_moves_as_its_auth_json() {
        let d = devices();
        let auth = codex_auth("a@example.com", "user-a", "tok-a");
        live_codex(&d.a_root, &auth);
        assert_eq!(d.b.signed_in(HarnessId::Codex).await.unwrap(), Some(false));

        let handoff = d.a.export_login(HarnessId::Codex).await.unwrap().unwrap();
        let json: serde_json::Value = serde_json::from_slice(&handoff.to_json().unwrap()).unwrap();
        assert_eq!(json["harness"], "codex");
        assert_eq!(json["logins"][0]["payload"]["credentials"], auth);
        assert_eq!(handoff.label(), Some("a@example.com"));

        assert_eq!(
            hand_over(&d, HarnessId::Codex).await,
            LoginImport::Installed
        );
        let b = cfg(&d.b_root);
        assert_eq!(read_json(&b.codex_auth_file()).unwrap(), auth);
        #[cfg(unix)]
        assert_eq!(mode(&b.codex_auth_file()), 0o600);
        assert_eq!(d.b.signed_in(HarnessId::Codex).await.unwrap(), Some(true));
        let slots = d.b.read_slots(HarnessId::Codex);
        assert_eq!(slots.len(), 1);
        assert_eq!(slots[0].profile.email, "a@example.com");
        let rows = d.b.list(false).await.unwrap().accounts;
        assert!(
            rows.iter()
                .any(|r| r.harness == HarnessId::Codex && r.active)
        );
    }

    #[tokio::test]
    async fn a_signed_in_device_keeps_its_own_login() {
        let d = devices();
        live_codex(&d.a_root, &codex_auth("a@example.com", "user-a", "tok-a"));
        live_codex(&d.b_root, &codex_auth("b@example.com", "user-b", "tok-b"));
        live_claude(&d.a_root, "acct-a", "a@example.com", "tok-a");
        live_claude(&d.b_root, "acct-b", "b@example.com", "tok-b");
        let before: Vec<(PathBuf, Vec<u8>)> = files(&d.b_root)
            .into_iter()
            .map(|f| (f.clone(), std::fs::read(&f).unwrap()))
            .collect();

        assert_eq!(
            hand_over(&d, HarnessId::Codex).await,
            LoginImport::AlreadySignedIn
        );
        assert_eq!(
            hand_over(&d, HarnessId::ClaudeCode).await,
            LoginImport::AlreadySignedIn
        );

        let after: Vec<(PathBuf, Vec<u8>)> = files(&d.b_root)
            .into_iter()
            .map(|f| (f.clone(), std::fs::read(&f).unwrap()))
            .collect();
        assert_eq!(before, after, "B was touched");
    }

    #[tokio::test]
    async fn garbage_and_mislabelled_handoffs_write_nothing() {
        let d = devices();
        live_codex(&d.a_root, &codex_auth("a@example.com", "user-a", "tok-a"));
        let handoff = d.a.export_login(HarnessId::Codex).await.unwrap().unwrap();

        // Garbage bytes, a different agent than expected, a newer format.
        let err = LoginHandoff::from_json(
            b"{\"v\":1,\"harness\":\"codex\",\"logins\":\"tok-x\"}",
            HarnessId::Codex,
        )
        .unwrap_err()
        .to_string();
        assert!(!err.contains("tok-x"), "{err}");
        assert!(LoginHandoff::from_json(b"not json", HarnessId::Codex).is_err());
        let bytes = handoff.to_json().unwrap();
        assert!(LoginHandoff::from_json(&bytes, HarnessId::ClaudeCode).is_err());

        // A Codex payload relabelled as Claude (or Cursor) is refused.
        for claimed in [HarnessId::ClaudeCode, HarnessId::Cursor, HarnessId::Grok] {
            let mut relabelled = handoff.clone();
            relabelled.harness = claimed;
            assert!(d.b.import_login(&relabelled).await.is_err(), "{claimed:?}");
        }
        // Credentials that belong to another account than the one named.
        let mut forged = handoff.clone();
        forged.logins[0].account_key = "someone-else".into();
        assert!(d.b.import_login(&forged).await.is_err());
        // Garbage credentials under the right label.
        let mut garbage = handoff.clone();
        garbage.logins[0].payload.credentials = serde_json::json!({ "tokens": "nope" });
        assert!(d.b.import_login(&garbage).await.is_err());
        // Unknown format, agents that can't take a handoff.
        let mut newer = handoff.clone();
        newer.v = 2;
        assert!(matches!(
            d.b.import_login(&newer).await.unwrap(),
            LoginImport::Unsupported(_)
        ));
        let mut hermes = handoff.clone();
        hermes.harness = HarnessId::Hermes;
        assert!(matches!(
            d.b.import_login(&hermes).await.unwrap(),
            LoginImport::Unsupported(_)
        ));

        assert_eq!(files(&d.b_root), Vec::<PathBuf>::new(), "B was written to");
        assert_eq!(d.b.signed_in(HarnessId::Codex).await.unwrap(), Some(false));
    }

    #[tokio::test]
    async fn a_handoff_round_trips_through_serde_and_a_private_file() {
        let d = devices();
        live_claude(&d.a_root, "acct-a", "a@example.com", "tok-a");
        let handoff =
            d.a.export_login(HarnessId::ClaudeCode)
                .await
                .unwrap()
                .unwrap();

        let parsed =
            LoginHandoff::from_json(&handoff.to_json().unwrap(), HarnessId::ClaudeCode).unwrap();
        assert_eq!(parsed, handoff);

        let staged = d.b_root.join("staging").join("login.json");
        std::fs::create_dir_all(staged.parent().unwrap()).unwrap();
        handoff.save(&staged).unwrap();
        #[cfg(unix)]
        assert_eq!(mode(&staged), 0o600);
        assert_eq!(
            LoginHandoff::load(&staged, HarnessId::ClaudeCode).unwrap(),
            handoff
        );
        // Debug output never carries a token.
        let debug = format!("{handoff:?}");
        assert!(
            debug.contains("a@example.com") && !debug.contains("tok-a"),
            "{debug}"
        );
    }

    #[tokio::test]
    async fn no_live_login_exports_nothing() {
        let d = devices();
        for harness in [
            HarnessId::ClaudeCode,
            HarnessId::Codex,
            HarnessId::Cursor,
            HarnessId::Grok,
            HarnessId::Devin,
            HarnessId::Opencode,
            HarnessId::Pi,
            HarnessId::Hermes,
            HarnessId::Antigravity,
        ] {
            assert!(
                d.a.export_login(harness).await.unwrap().is_none(),
                "{harness:?}"
            );
        }
        // A login with dead tokens isn't worth sending.
        write(
            &cfg(&d.a_root).codex_auth_file(),
            &serde_json::json!({ "tokens": {} }).to_string(),
        );
        assert!(d.a.export_login(HarnessId::Codex).await.unwrap().is_none());
        assert_eq!(d.a.signed_in(HarnessId::Codex).await.unwrap(), Some(false));
        // A store that doesn't parse is "can't tell" — and never replaced.
        write(&cfg(&d.b_root).codex_auth_file(), "{torn");
        assert_eq!(d.b.signed_in(HarnessId::Codex).await.unwrap(), None);
        live_codex(&d.a_root, &codex_auth("a@example.com", "user-a", "tok-a"));
        let handoff = d.a.export_login(HarnessId::Codex).await.unwrap().unwrap();
        assert!(d.b.import_login(&handoff).await.is_err());
        assert_eq!(
            std::fs::read_to_string(cfg(&d.b_root).codex_auth_file()).unwrap(),
            "{torn"
        );
    }

    #[tokio::test]
    async fn cursor_grok_and_devin_logins_move_too() {
        let d = devices();
        let a = cfg(&d.a_root);
        let b = cfg(&d.b_root);
        write(
            &a.cursor_sdk_auth_file,
            &serde_json::json!({
                "version": 1,
                "apiKey": "key_cursor_a",
                "email": "c@example.com",
                "apiKeyExpiresAtMs": now_ms() + 86_400_000,
            })
            .to_string(),
        );
        write(
            &a.grok_home.join("auth.json"),
            &serde_json::json!({
                "https://auth.x.ai::client": {
                    "key": "grok-access",
                    "refresh_token": "grok-refresh",
                    "user_id": "grok-user",
                    "email": "g@example.com",
                    "oidc_issuer": "https://auth.x.ai",
                },
            })
            .to_string(),
        );
        write(
            &a.devin_credentials_file,
            "windsurf_api_key = \"sk-ws-devin-a\"\napi_server_url = \"https://server.codeium.com\"\n",
        );
        // B's grok store already holds another issuer's (non-login) entry.
        write(
            &b.grok_home.join("auth.json"),
            &serde_json::json!({ "legacy": { "note": "keep" } }).to_string(),
        );

        for harness in [HarnessId::Cursor, HarnessId::Grok, HarnessId::Devin] {
            assert_eq!(
                d.b.signed_in(harness).await.unwrap(),
                Some(false),
                "{harness:?}"
            );
            assert_eq!(
                hand_over(&d, harness).await,
                LoginImport::Installed,
                "{harness:?}"
            );
            assert_eq!(
                d.b.signed_in(harness).await.unwrap(),
                Some(true),
                "{harness:?}"
            );
            assert_eq!(d.b.read_slots(harness).len(), 1, "{harness:?}");
            // A second handoff finds B signed in.
            assert_eq!(hand_over(&d, harness).await, LoginImport::AlreadySignedIn);
        }
        assert_eq!(
            read_json(&b.cursor_sdk_auth_file).unwrap()["apiKey"],
            "key_cursor_a"
        );
        let grok = read_json(&b.grok_home.join("auth.json")).unwrap();
        assert_eq!(grok["https://auth.x.ai::client"]["key"], "grok-access");
        assert_eq!(grok["legacy"]["note"], "keep");
        let devin = std::fs::read_to_string(&b.devin_credentials_file).unwrap();
        assert!(devin.contains("sk-ws-devin-a") && devin.contains("server.codeium.com"));
        #[cfg(unix)]
        for file in [&b.cursor_sdk_auth_file, &b.devin_credentials_file] {
            assert_eq!(mode(file), 0o600);
        }
    }

    #[tokio::test]
    async fn opencode_takes_only_the_provider_logins_it_lacks() {
        let d = devices();
        let access = jwt(serde_json::json!({
            "https://api.openai.com/auth": { "chatgpt_account_id": "ws-a", "chatgpt_plan_type": "pro" },
            "https://api.openai.com/profile": { "email": "o@example.com" },
        }));
        write(
            &cfg(&d.a_root).opencode_auth_file,
            &serde_json::json!({
                "openai": { "type": "oauth", "access": access, "refresh": "o-refresh", "expires": 1 },
                "anthropic": { "type": "api", "key": "sk-ant-a" },
            })
            .to_string(),
        );
        // B keeps an API key of its own for another provider.
        let b_file = cfg(&d.b_root).opencode_auth_file;
        write(
            &b_file,
            &serde_json::json!({ "anthropic": { "type": "api", "key": "sk-ant-b" } }).to_string(),
        );

        let handoff =
            d.a.export_login(HarnessId::Opencode)
                .await
                .unwrap()
                .unwrap();
        // Only managed OAuth logins travel — never A's API keys.
        assert_eq!(handoff.logins.len(), 1);
        assert_eq!(handoff.logins[0].store_key.as_deref(), Some("openai"));
        assert!(!String::from_utf8_lossy(&handoff.to_json().unwrap()).contains("sk-ant-a"));

        assert_eq!(
            d.b.import_login(&handoff).await.unwrap(),
            LoginImport::Installed
        );
        let store = read_json(&b_file).unwrap();
        assert_eq!(store["openai"]["refresh"], "o-refresh");
        assert_eq!(store["anthropic"]["key"], "sk-ant-b");
        assert_eq!(
            d.b.signed_in(HarnessId::Opencode).await.unwrap(),
            Some(true)
        );
        assert_eq!(
            d.b.import_login(&handoff).await.unwrap(),
            LoginImport::AlreadySignedIn
        );
        let rows = d.b.list(false).await.unwrap().accounts;
        assert!(rows.iter().any(|r| r.harness == HarnessId::Opencode
            && r.active
            && r.email.as_deref() == Some("o@example.com")));
    }
}
