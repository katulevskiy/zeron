//! Request signatures for the Worker's box routes (docs/cloud.md §2).
//!
//! `x-zeron-signature` = hex(HMAC-SHA256(key, "{METHOD}\n{path}\n{timestamp}\n{hex(sha256(body))}"))
//! where `key` is the box's `wakeKey` decoded from base64 (32 bytes),
//! `path` is the URL path without the query, and `timestamp` is the
//! `x-zeron-timestamp` header (epoch ms). The Worker (`cloud/worker/worker.js`)
//! implements the same check; both test against the same vector.

use base64::Engine as _;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

pub const TIMESTAMP_HEADER: &str = "x-zeron-timestamp";
pub const SIGNATURE_HEADER: &str = "x-zeron-signature";
/// The Worker rejects a timestamp further than this from its own clock.
pub const MAX_SKEW_MS: i64 = 5 * 60 * 1000;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SignError {
    #[error("the wake key isn't base64 of 32 bytes")]
    BadKey,
    #[error("missing or malformed timestamp")]
    BadTimestamp,
    #[error("the request timestamp is more than 5 minutes off")]
    Skewed,
    #[error("bad signature")]
    BadSignature,
}

/// A fresh wake key: base64 of 32 random bytes.
pub fn new_wake_key() -> String {
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes).expect("the OS random source is unavailable");
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn key_bytes(wake_key: &str) -> Result<Vec<u8>, SignError> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(wake_key.trim())
        .map_err(|_| SignError::BadKey)?;
    if bytes.len() != 32 {
        return Err(SignError::BadKey);
    }
    Ok(bytes)
}

/// The signed string.
pub fn canonical(method: &str, path: &str, timestamp_ms: i64, body: &[u8]) -> String {
    format!(
        "{}\n{}\n{}\n{}",
        method.to_ascii_uppercase(),
        path,
        timestamp_ms,
        hex::encode(Sha256::digest(body))
    )
}

fn mac(
    wake_key: &str,
    method: &str,
    path: &str,
    timestamp_ms: i64,
    body: &[u8],
) -> Result<Hmac<Sha256>, SignError> {
    let mut mac =
        Hmac::<Sha256>::new_from_slice(&key_bytes(wake_key)?).map_err(|_| SignError::BadKey)?;
    mac.update(canonical(method, path, timestamp_ms, body).as_bytes());
    Ok(mac)
}

/// Hex signature for a request.
pub fn sign(
    wake_key: &str,
    method: &str,
    path: &str,
    timestamp_ms: i64,
    body: &[u8],
) -> Result<String, SignError> {
    Ok(hex::encode(
        mac(wake_key, method, path, timestamp_ms, body)?
            .finalize()
            .into_bytes(),
    ))
}

/// Check a request's headers against `wake_key` at `now_ms` (constant-time
/// comparison).
pub fn verify(
    wake_key: &str,
    method: &str,
    path: &str,
    timestamp_header: &str,
    signature_hex: &str,
    body: &[u8],
    now_ms: i64,
) -> Result<(), SignError> {
    let timestamp: i64 = timestamp_header
        .trim()
        .parse()
        .map_err(|_| SignError::BadTimestamp)?;
    if (now_ms - timestamp).abs() > MAX_SKEW_MS {
        return Err(SignError::Skewed);
    }
    let signature = hex::decode(signature_hex.trim()).map_err(|_| SignError::BadSignature)?;
    mac(wake_key, method, path, timestamp, body)?
        .verify_slice(&signature)
        .map_err(|_| SignError::BadSignature)
}

/// The two headers for a request signed now.
pub fn signed_headers(
    wake_key: &str,
    method: &str,
    path: &str,
    body: &[u8],
    now_ms: i64,
) -> Result<[(&'static str, String); 2], SignError> {
    Ok([
        (TIMESTAMP_HEADER, now_ms.to_string()),
        (
            SIGNATURE_HEADER,
            sign(wake_key, method, path, now_ms, body)?,
        ),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shared with `cloud/worker/worker.test.js`.
    const KEY: &str = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=";
    const SIG: &str = "110b15f3785ac18344494e97a37359f8bd9a8785b91bc421938a3f587a13efa7";
    const TS: i64 = 1_700_000_000_000;
    const PATH: &str = "/v1/boxes/dev-1/wake";

    #[test]
    fn matches_the_shared_vector() {
        assert_eq!(sign(KEY, "POST", PATH, TS, b"{}").unwrap(), SIG);
        assert_eq!(sign(KEY, "post", PATH, TS, b"{}").unwrap(), SIG);
    }

    #[test]
    fn round_trips_and_rejects_tampering() {
        let key = new_wake_key();
        let sig = sign(&key, "POST", PATH, TS, b"{}").unwrap();
        let ts = TS.to_string();
        verify(&key, "POST", PATH, &ts, &sig, b"{}", TS + 1000).unwrap();
        assert_eq!(
            verify(&key, "POST", PATH, &ts, &sig, b"{\"x\":1}", TS),
            Err(SignError::BadSignature)
        );
        assert_eq!(
            verify(&key, "POST", "/v1/boxes/dev-2/wake", &ts, &sig, b"{}", TS),
            Err(SignError::BadSignature)
        );
        assert_eq!(
            verify(&KEY.to_string(), "POST", PATH, &ts, &sig, b"{}", TS),
            Err(SignError::BadSignature)
        );
        assert_eq!(
            verify(&key, "POST", PATH, &ts, &sig, b"{}", TS + MAX_SKEW_MS + 1),
            Err(SignError::Skewed)
        );
        assert_eq!(
            verify(&key, "POST", PATH, "soon", &sig, b"{}", TS),
            Err(SignError::BadTimestamp)
        );
        assert_eq!(sign("short", "POST", PATH, TS, b""), Err(SignError::BadKey));
    }

    #[test]
    fn wake_keys_are_32_random_bytes() {
        let a = new_wake_key();
        assert_eq!(key_bytes(&a).unwrap().len(), 32);
        assert_ne!(a, new_wake_key());
    }
}
