/**
 * Device credentials (docs/cloud.md §6): the long-lived secret a cloud box
 * trades for short-lived edge tokens. One record per device, kept in that
 * device's own DeviceRoom (`d2/{deviceId}`) — the exchange route knows only
 * the device id, and the room is already the per-device identity anchor, so
 * no new Durable Object class or index is needed.
 *
 * Only a salted SHA-256 of the credential is stored. Verification compares
 * digests in constant time and counts failures per device: after
 * `FAIL_LIMIT` wrong guesses inside `FAIL_WINDOW_MS` every attempt (right or
 * wrong) answers `rate_limited` until the window ends.
 */

export const FAIL_LIMIT = 10;
export const FAIL_WINDOW_MS = 10 * 60_000;
const MAX_CREDENTIAL_LEN = 256;

export interface CredentialRecord {
  userId: string;
  orgId: string;
  name: string;
  createdAt: number;
  revokedAt: number | null;
}

export type VerifyOutcome =
  | { ok: true; record: CredentialRecord }
  | { ok: false; error: "invalid_credential" | "revoked" }
  | { ok: false; error: "rate_limited"; retryAfterMs: number };

interface Row {
  user_id: string;
  org_id: string;
  name: string;
  salt: string;
  hash: string;
  created_at: number;
  revoked_at: number | null;
  failures: number;
  fail_window_start: number;
}

export const ensureCredentials = (sql: SqlStorage): void => {
  sql.exec(`CREATE TABLE IF NOT EXISTS device_credential (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    user_id TEXT NOT NULL,
    org_id TEXT NOT NULL,
    name TEXT NOT NULL,
    salt TEXT NOT NULL,
    hash TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    revoked_at INTEGER,
    failures INTEGER NOT NULL DEFAULT 0,
    fail_window_start INTEGER NOT NULL DEFAULT 0
  )`);
};

const readRow = (sql: SqlStorage): Row | undefined =>
  [...sql.exec("SELECT * FROM device_credential WHERE id = 1")][0] as unknown as Row | undefined;

const toRecord = (row: Row): CredentialRecord => ({
  userId: row.user_id,
  orgId: row.org_id,
  name: row.name,
  createdAt: row.created_at,
  revokedAt: row.revoked_at ?? null
});

export const credentialRecord = (sql: SqlStorage): CredentialRecord | undefined => {
  const row = readRow(sql);
  return row ? toRecord(row) : undefined;
};

/** 32 random bytes, base64url without padding (43 characters). */
export const newCredential = (): string => base64url(crypto.getRandomValues(new Uint8Array(32)));

export const base64url = (bytes: Uint8Array): string => {
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary).replaceAll("+", "-").replaceAll("/", "_").replace(/=+$/, "");
};

const hex = (bytes: Uint8Array): string =>
  [...bytes].map((b) => b.toString(16).padStart(2, "0")).join("");

const unhex = (value: string): Uint8Array => {
  const out = new Uint8Array(value.length / 2);
  for (let i = 0; i < out.length; i++) out[i] = parseInt(value.slice(i * 2, i * 2 + 2), 16);
  return out;
};

export const hashCredential = async (salt: Uint8Array, credential: string): Promise<Uint8Array> => {
  const secret = new TextEncoder().encode(credential);
  const input = new Uint8Array(salt.length + secret.length);
  input.set(salt);
  input.set(secret, salt.length);
  return new Uint8Array(await crypto.subtle.digest("SHA-256", input));
};

/** Equal-length byte comparison whose time does not depend on where the
 * inputs differ. */
export const constantTimeEqual = (a: Uint8Array, b: Uint8Array): boolean => {
  if (a.length !== b.length) return false;
  let diff = 0;
  for (let i = 0; i < a.length; i++) diff |= a[i]! ^ b[i]!;
  return diff === 0;
};

/** Store the record for a freshly minted credential. `false` = this device
 * already has one (ids are fresh uuids, so this is never expected). */
export const createCredential = async (
  sql: SqlStorage,
  input: { userId: string; orgId: string; name: string; credential: string; now: number }
): Promise<boolean> => {
  if (readRow(sql)) return false;
  const salt = crypto.getRandomValues(new Uint8Array(16));
  const hash = await hashCredential(salt, input.credential);
  sql.exec(
    `INSERT INTO device_credential (id, user_id, org_id, name, salt, hash, created_at)
     VALUES (1, ?, ?, ?, ?, ?, ?)`,
    input.userId,
    input.orgId,
    input.name,
    hex(salt),
    hex(hash),
    input.now
  );
  return true;
};

export const verifyCredential = async (
  sql: SqlStorage,
  credential: unknown,
  now: number
): Promise<VerifyOutcome> => {
  const row = readRow(sql);
  // Nothing to guess against: don't write (unknown ids must not grow state).
  if (!row) return { ok: false, error: "invalid_credential" };
  const windowOpen = now - row.fail_window_start < FAIL_WINDOW_MS;
  if (windowOpen && row.failures >= FAIL_LIMIT) {
    return {
      ok: false,
      error: "rate_limited",
      retryAfterMs: row.fail_window_start + FAIL_WINDOW_MS - now
    };
  }
  const valid =
    typeof credential === "string" &&
    credential.length > 0 &&
    credential.length <= MAX_CREDENTIAL_LEN &&
    constantTimeEqual(await hashCredential(unhex(row.salt), credential), unhex(row.hash));
  if (!valid) {
    if (windowOpen) {
      sql.exec("UPDATE device_credential SET failures = failures + 1 WHERE id = 1");
    } else {
      sql.exec(
        "UPDATE device_credential SET failures = 1, fail_window_start = ? WHERE id = 1",
        now
      );
    }
    return { ok: false, error: "invalid_credential" };
  }
  if (row.failures !== 0) sql.exec("UPDATE device_credential SET failures = 0 WHERE id = 1");
  // Revocation is only disclosed to a caller holding the credential.
  if (row.revoked_at !== null) return { ok: false, error: "revoked" };
  return { ok: true, record: toRecord(row) };
};

export type RevokeOutcome =
  | { ok: true; record: CredentialRecord }
  | { ok: false; error: "not_found" | "forbidden" };

/** Revoke (idempotent). Only the owning user may revoke. */
export const revokeCredential = (sql: SqlStorage, userId: string, now: number): RevokeOutcome => {
  const row = readRow(sql);
  if (!row) return { ok: false, error: "not_found" };
  if (row.user_id !== userId) return { ok: false, error: "forbidden" };
  if (row.revoked_at === null) {
    sql.exec("UPDATE device_credential SET revoked_at = ? WHERE id = 1", now);
    row.revoked_at = now;
  }
  return { ok: true, record: toRecord(row) };
};

/** Whether a device token for this room may still host it: the room must
 * hold a live credential belonging to the token's user. */
export const deviceTokenAdmitted = (sql: SqlStorage, userId: string): boolean => {
  const row = readRow(sql);
  return !!row && row.revoked_at === null && row.user_id === userId;
};
