//! Cloud-box enrollment — the port of `edge/src/cloud-devices.ts`,
//! `device-credentials.ts` and `device-tokens.ts` (docs/cloud.md §6), so a
//! box engine can enrol against a local edge exactly as against the Worker:
//!
//! - `POST /cloud/devices` (shared secret) mints `{deviceId, credential}`;
//!   only a salted SHA-256 of the credential is stored.
//! - `POST /auth/device-token` (no bearer) trades it for `{token, expiresAt}`,
//!   with the Worker's per-device failure limit.
//! - `DELETE /cloud/devices/{id}` (shared secret) revokes it and drops the
//!   box's live host socket.
//! - Device tokens then authorize like the Worker's ([`scope_allows`]).
//!
//! Single tenant: the owner is the shared-secret holder, so tokens carry
//! `sub`/`org_id` = `local`. Unlike the Worker, tokens are HS256 under a
//! random key kept in the edge database (no public key to publish:
//! `/.well-known/zeron-device-jwks.json` answers an empty set), and every
//! request re-checks revocation, which costs one SQLite read here.

use std::collections::HashMap;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hmac::{Hmac, Mac};
use hyper::Request;
use hyper::body::Incoming;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use crate::http::{self, Reply, now_ms};
use crate::{Edge, valid_id};

/// The single tenant's identity in device tokens (the engine's
/// `LOCAL_EDGE_IDENTITY`).
pub(crate) const LOCAL_IDENTITY: &str = "local";
pub(crate) const ISSUER: &str = "zeron-edge";
pub(crate) const KIND: &str = "cloud";
pub(crate) const TOKEN_TTL_S: i64 = 30 * 60;
pub(crate) const FAIL_LIMIT: i64 = 10;
pub(crate) const FAIL_WINDOW_MS: i64 = 10 * 60_000;
const MAX_CREDENTIAL_LEN: usize = 256;
const MAX_NAME_LEN: usize = 80;
const DEFAULT_NAME: &str = "Cloud";

pub(crate) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS device_credentials (
    device_id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    salt BLOB NOT NULL,
    hash BLOB NOT NULL,
    created_at INTEGER NOT NULL,
    revoked_at INTEGER,
    failures INTEGER NOT NULL DEFAULT 0,
    fail_window_start INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS edge_meta (
    key TEXT PRIMARY KEY,
    value BLOB NOT NULL
);
";

fn random<const N: usize>() -> [u8; N] {
    let mut bytes = [0u8; N];
    getrandom::fill(&mut bytes).expect("OS randomness");
    bytes
}

fn hash_credential(salt: &[u8], credential: &str) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update(salt);
    hasher.update(credential.as_bytes());
    hasher.finalize().to_vec()
}

/// The token-signing key, created on first use and kept across restarts so
/// an edge restart does not invalidate live box tokens.
fn signing_key(db: &Connection) -> rusqlite::Result<Vec<u8>> {
    let existing: Option<Vec<u8>> = db
        .query_row(
            "SELECT value FROM edge_meta WHERE key = 'device_jwt_key'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(key) = existing {
        return Ok(key);
    }
    let key = random::<32>().to_vec();
    db.execute(
        "INSERT INTO edge_meta (key, value) VALUES ('device_jwt_key', ?1)",
        params![key],
    )?;
    Ok(key)
}

fn sign(key: &[u8], input: &str) -> Vec<u8> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(input.as_bytes());
    mac.finalize().into_bytes().to_vec()
}

fn mint_token(key: &[u8], device_id: &str, now_s: i64) -> (String, i64) {
    let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"HS256","typ":"JWT","kid":"local"}"#);
    let exp = now_s + TOKEN_TTL_S;
    let claims = json!({
        "iss": ISSUER,
        "sub": LOCAL_IDENTITY,
        "org_id": LOCAL_IDENTITY,
        "did": device_id,
        "kind": KIND,
        "iat": now_s,
        "exp": exp,
    });
    let payload = URL_SAFE_NO_PAD.encode(claims.to_string());
    let input = format!("{header}.{payload}");
    let signature = URL_SAFE_NO_PAD.encode(sign(key, &input));
    (format!("{input}.{signature}"), exp * 1000)
}

/// The device id of a valid, unexpired, unrevoked device token.
pub(crate) fn verify_token(db: &Connection, token: &str) -> Option<String> {
    let mut parts = token.split('.');
    let (header, payload, signature) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() {
        return None;
    }
    let key = signing_key(db).ok()?;
    let expected = sign(&key, &format!("{header}.{payload}"));
    let presented = URL_SAFE_NO_PAD.decode(signature).ok()?;
    if !bool::from(expected.ct_eq(&presented)) {
        return None;
    }
    let claims: Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload).ok()?).ok()?;
    if claims["iss"] != ISSUER || claims["kind"] != KIND {
        return None;
    }
    if claims["exp"].as_i64()? * 1000 <= now_ms() {
        return None;
    }
    let did = claims["did"].as_str()?.to_owned();
    let live: Option<Option<i64>> = db
        .query_row(
            "SELECT revoked_at FROM device_credentials WHERE device_id = ?1",
            params![did],
            |row| row.get(0),
        )
        .optional()
        .ok()?;
    matches!(live, Some(None)).then_some(did)
}

/// Unverified `iss` check: a token naming our issuer is verified only as a
/// device token, never compared against the shared secret.
pub(crate) fn names_issuer(token: &str) -> bool {
    token
        .split('.')
        .nth(1)
        .and_then(|payload| URL_SAFE_NO_PAD.decode(payload).ok())
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .is_some_and(|claims| claims["iss"] == ISSUER)
}

enum Verdict {
    Ok,
    Invalid,
    Revoked,
    RateLimited { retry_after_ms: i64 },
}

fn verify_credential(
    db: &Connection,
    device_id: &str,
    credential: &str,
    now: i64,
) -> rusqlite::Result<Verdict> {
    type Row = (Vec<u8>, Vec<u8>, Option<i64>, i64, i64);
    let row: Option<Row> = db
        .query_row(
            "SELECT salt, hash, revoked_at, failures, fail_window_start
             FROM device_credentials WHERE device_id = ?1",
            params![device_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
        )
        .optional()?;
    let Some((salt, hash, revoked_at, failures, window_start)) = row else {
        return Ok(Verdict::Invalid);
    };
    let window_open = now - window_start < FAIL_WINDOW_MS;
    if window_open && failures >= FAIL_LIMIT {
        return Ok(Verdict::RateLimited {
            retry_after_ms: window_start + FAIL_WINDOW_MS - now,
        });
    }
    let valid = !credential.is_empty()
        && credential.len() <= MAX_CREDENTIAL_LEN
        && bool::from(hash_credential(&salt, credential).ct_eq(&hash));
    if !valid {
        if window_open {
            db.execute(
                "UPDATE device_credentials SET failures = failures + 1 WHERE device_id = ?1",
                params![device_id],
            )?;
        } else {
            db.execute(
                "UPDATE device_credentials SET failures = 1, fail_window_start = ?2
                 WHERE device_id = ?1",
                params![device_id, now],
            )?;
        }
        return Ok(Verdict::Invalid);
    }
    if failures != 0 {
        db.execute(
            "UPDATE device_credentials SET failures = 0 WHERE device_id = ?1",
            params![device_id],
        )?;
    }
    Ok(if revoked_at.is_some() {
        Verdict::Revoked
    } else {
        Verdict::Ok
    })
}

fn storage_error(err: rusqlite::Error) -> Reply {
    tracing::error!(error = %err, "local edge: credential storage failed");
    http::error(500, "storage")
}

/// `POST /auth/device-token`.
pub(crate) async fn device_token(edge: &Edge, request: Request<Incoming>) -> Reply {
    let body = match http::read_body(request.into_body(), 16 * 1024).await {
        Ok(body) => body,
        Err(reply) => return reply,
    };
    let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    let (Some(device_id), Some(credential)) = (body["deviceId"].as_str(), body["credential"].as_str())
    else {
        return http::error(400, "missing deviceId or credential");
    };
    if !valid_id(device_id) {
        return http::error(400, "missing deviceId or credential");
    }
    let outcome = edge.with_state(|state| {
        let verdict = verify_credential(&state.db, device_id, credential, now_ms())?;
        let key = signing_key(&state.db)?;
        Ok::<_, rusqlite::Error>((verdict, key))
    });
    match outcome {
        Err(err) => storage_error(err),
        Ok((Verdict::Ok, key)) => {
            let (token, expires_at) = mint_token(&key, device_id, now_ms() / 1000);
            http::json(&json!({ "token": token, "expiresAt": expires_at }), 200)
        }
        Ok((Verdict::Invalid, _)) => {
            tracing::warn!(device = %device_id, "local edge: device credential rejected");
            http::error(401, "invalid_credential")
        }
        Ok((Verdict::Revoked, _)) => http::error(401, "revoked"),
        Ok((Verdict::RateLimited { retry_after_ms }, _)) => {
            let retry_after = ((retry_after_ms + 999) / 1000).max(1);
            http::reply(
                429,
                &[
                    ("content-type", "application/json"),
                    ("retry-after", &retry_after.to_string()),
                ],
                json!({ "error": "rate_limited", "retryAfter": retry_after })
                    .to_string()
                    .into_bytes(),
            )
        }
    }
}

/// `POST /cloud/devices` and `DELETE /cloud/devices/{id}` (owner only).
pub(crate) async fn devices(edge: &Edge, rest: &[&str], request: Request<Incoming>) -> Reply {
    match (rest, request.method().as_str()) {
        ([], "POST") => {
            let body = match http::read_body(request.into_body(), 16 * 1024).await {
                Ok(body) => body,
                Err(reply) => return reply,
            };
            let body: Value = if body.is_empty() {
                json!({})
            } else {
                serde_json::from_slice(&body).unwrap_or(json!({}))
            };
            let name = match &body["name"] {
                Value::Null => DEFAULT_NAME.to_owned(),
                Value::String(name) if name.trim().is_empty() => DEFAULT_NAME.to_owned(),
                Value::String(name) if name.trim().chars().count() <= MAX_NAME_LEN => {
                    name.trim().to_owned()
                }
                Value::String(_) => {
                    return http::error(400, &format!("name must be 1-{MAX_NAME_LEN} characters"));
                }
                _ => return http::error(400, "name must be a string"),
            };
            let device_id = uuid::Uuid::new_v4().to_string();
            let credential = URL_SAFE_NO_PAD.encode(random::<32>());
            let salt = random::<16>();
            let hash = hash_credential(&salt, &credential);
            let stored = edge.with_state(|state| {
                state.db.execute(
                    "INSERT INTO device_credentials (device_id, name, salt, hash, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![device_id, name, salt.to_vec(), hash, now_ms()],
                )?;
                // Claim the device room for the owner, as the Worker does.
                state.db.execute(
                    "INSERT OR IGNORE INTO device_meta (device_id, key, value)
                     VALUES (?1, 'owner', 'local')",
                    params![device_id],
                )
            });
            match stored {
                Ok(_) => http::json(
                    &json!({ "deviceId": device_id, "credential": credential }),
                    200,
                ),
                Err(err) => storage_error(err),
            }
        }
        ([device_id], "DELETE") if valid_id(device_id) => {
            let now = now_ms();
            let revoked = edge.with_state(|state| {
                let row: Option<Option<i64>> = state
                    .db
                    .query_row(
                        "SELECT revoked_at FROM device_credentials WHERE device_id = ?1",
                        params![device_id],
                        |row| row.get(0),
                    )
                    .optional()?;
                let Some(revoked_at) = row else {
                    return Ok(None);
                };
                let revoked_at = match revoked_at {
                    Some(at) => at,
                    None => {
                        state.db.execute(
                            "UPDATE device_credentials SET revoked_at = ?2 WHERE device_id = ?1",
                            params![device_id, now],
                        )?;
                        now
                    }
                };
                crate::device::close_hosts(state, device_id, 4403, "device credential revoked");
                Ok::<_, rusqlite::Error>(Some(revoked_at))
            });
            match revoked {
                Ok(Some(at)) => http::json(&json!({ "revokedAt": at }), 200),
                Ok(None) => http::error(404, "not_found"),
                Err(err) => storage_error(err),
            }
        }
        _ => http::error(404, "not_found"),
    }
}

/// The Worker's `deviceTokenScope`: may a device token for `did` reach this
/// route? `parts` is the split path, `query` its parameters.
pub(crate) fn scope_allows(
    did: &str,
    method: &str,
    parts: &[&str],
    query: &HashMap<String, String>,
) -> bool {
    match parts {
        ["device", id, "ws"] => {
            if query.get("role").map(String::as_str) == Some("host") {
                *id == did
            } else {
                *id != did
            }
        }
        ["device", id, "sidecar", ..] => *id == did,
        ["device", _, "status" | "nudge"] => true,
        ["registry", _, "ws" | "rows" | "push" | "stats"] => true,
        ["chat2", _, "ws" | "checkpoint" | "rows" | "tail" | "diff" | "stats"] => true,
        ["blob", ..] => true,
        ["preview", _, "ws"] => true,
        ["diff", _] => method == "POST",
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> (tempfile::TempDir, Connection) {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::store::open(dir.path()).unwrap();
        (dir, db)
    }

    #[test]
    fn tokens_verify_until_expiry_or_revocation_and_resist_tampering() {
        let (_dir, db) = db();
        db.execute(
            "INSERT INTO device_credentials (device_id, name, salt, hash, created_at)
             VALUES ('box', 'Cloud', x'00', x'00', 0)",
            [],
        )
        .unwrap();
        let key = signing_key(&db).unwrap();
        assert_eq!(signing_key(&db).unwrap(), key, "key is stable");
        let now = now_ms() / 1000;
        let (token, expires_at) = mint_token(&key, "box", now);
        assert_eq!(expires_at, (now + TOKEN_TTL_S) * 1000);
        assert!(names_issuer(&token));
        assert_eq!(verify_token(&db, &token).as_deref(), Some("box"));

        let (expired, _) = mint_token(&key, "box", now - TOKEN_TTL_S - 1);
        assert_eq!(verify_token(&db, &expired), None);
        let (foreign, _) = mint_token(&random::<32>(), "box", now);
        assert_eq!(verify_token(&db, &foreign), None);
        let mut parts: Vec<String> = token.split('.').map(str::to_owned).collect();
        parts[1] = URL_SAFE_NO_PAD.encode(
            json!({"iss": ISSUER, "kind": KIND, "did": "other", "exp": now + 60}).to_string(),
        );
        assert_eq!(verify_token(&db, &parts.join(".")), None);

        db.execute("UPDATE device_credentials SET revoked_at = 1", [])
            .unwrap();
        assert_eq!(verify_token(&db, &token), None);
    }

    #[test]
    fn credential_checks_count_failures_per_device() {
        let (_dir, db) = db();
        let salt = random::<16>();
        db.execute(
            "INSERT INTO device_credentials (device_id, name, salt, hash, created_at)
             VALUES ('box', 'Cloud', ?1, ?2, 0)",
            params![salt.to_vec(), hash_credential(&salt, "secret")],
        )
        .unwrap();
        let now = now_ms();
        assert!(matches!(verify_credential(&db, "box", "secret", now).unwrap(), Verdict::Ok));
        assert!(matches!(verify_credential(&db, "nobody", "secret", now).unwrap(), Verdict::Invalid));
        for _ in 0..FAIL_LIMIT {
            assert!(matches!(verify_credential(&db, "box", "wrong", now).unwrap(), Verdict::Invalid));
        }
        assert!(matches!(
            verify_credential(&db, "box", "secret", now).unwrap(),
            Verdict::RateLimited { .. }
        ));
        // The window ends; the right credential works again.
        assert!(matches!(
            verify_credential(&db, "box", "secret", now + FAIL_WINDOW_MS).unwrap(),
            Verdict::Ok
        ));
    }

    #[test]
    fn scope_matches_the_worker() {
        let none = HashMap::new();
        let host = HashMap::from([("role".to_owned(), "host".to_owned())]);
        assert!(scope_allows("box", "GET", &["device", "box", "ws"], &host));
        assert!(!scope_allows("box", "GET", &["device", "other", "ws"], &host));
        assert!(!scope_allows("box", "GET", &["device", "box", "ws"], &none));
        assert!(scope_allows("box", "GET", &["device", "other", "ws"], &none));
        assert!(scope_allows("box", "POST", &["device", "box", "sidecar", "repos"], &none));
        assert!(!scope_allows("box", "POST", &["device", "other", "sidecar", "repos"], &none));
        assert!(scope_allows("box", "GET", &["registry", "local", "rows"], &none));
        assert!(!scope_allows("box", "POST", &["registry", "local", "reset"], &none));
        assert!(!scope_allows("box", "POST", &["chat2", "c", "reset"], &none));
        assert!(!scope_allows("box", "POST", &["cloud", "devices"], &none));
        assert!(!scope_allows("box", "GET", &["auth", "orgs"], &none));
    }
}
