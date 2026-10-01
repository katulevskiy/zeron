// Pure parts of the Zeron cloud box Worker (docs/cloud.md §2–§5): request
// signatures, routing, the alarm's decision and the checkpoint store. No
// runtime imports, so they test under node; worker.js wires them up.

export const CONTROL_PORT = 8787;
export const ALARM_MS = 60_000;
export const MAX_SKEW_MS = 5 * 60_000;
/** How long a starting engine may stay silent before the box is put to sleep. */
export const BOOT_GRACE_MS = 5 * 60_000;
export const MAX_OBJECT_BYTES = 64 * 1024 * 1024;
export const DEFAULT_IDLE_MINUTES = 15;
export const CHECKPOINT_HOST = "r2.zeron.internal";
export const FINAL_CHECKPOINT_MS = 10 * 60_000;
export const SIGTERM = 15;
const ACTIONS = new Set(["wake", "stop", "status", "purge"]);
const DEVICE_ID = /^[A-Za-z0-9_.:-]{1,128}$/;
const encoder = new TextEncoder();

export const json = (value, status = 200) =>
  new Response(JSON.stringify(value), {
    status,
    headers: { "content-type": "application/json" },
  });

// ── signatures (§2) ──────────────────────────────────────────────────────────

export const toHex = (buffer) =>
  [...new Uint8Array(buffer)].map((b) => b.toString(16).padStart(2, "0")).join("");

function fromHex(text) {
  if (text.length % 2 !== 0 || !/^[0-9a-f]*$/i.test(text)) return null;
  const out = new Uint8Array(text.length / 2);
  for (let i = 0; i < out.length; i++) out[i] = parseInt(text.slice(2 * i, 2 * i + 2), 16);
  return out;
}

function hmacKey(wakeKey, usage) {
  let raw;
  try {
    raw = Uint8Array.from(atob(wakeKey), (c) => c.charCodeAt(0));
  } catch {
    raw = new Uint8Array();
  }
  if (raw.length !== 32) throw new Error("bad wake key");
  return crypto.subtle.importKey("raw", raw, { name: "HMAC", hash: "SHA-256" }, false, [usage]);
}

/** `{METHOD}\n{path}\n{timestamp}\n{hex(sha256(body))}` */
export async function canonical(method, path, timestamp, body) {
  const digest = await crypto.subtle.digest("SHA-256", body);
  return `${method.toUpperCase()}\n${path}\n${timestamp}\n${toHex(digest)}`;
}

export async function sign(wakeKey, method, path, timestamp, body) {
  const key = await hmacKey(wakeKey, "sign");
  const text = await canonical(method, path, timestamp, body);
  return toHex(await crypto.subtle.sign("HMAC", key, encoder.encode(text)));
}

/** `null` when the request is genuine, else why not. */
export async function verifySignature({ wakeKey, method, path, timestamp, signature, body, now }) {
  if (!/^\d{1,16}$/.test(timestamp ?? "")) return "bad timestamp";
  if (Math.abs(now - Number(timestamp)) > MAX_SKEW_MS) return "stale timestamp";
  const mac = fromHex(signature ?? "");
  if (!mac || mac.length !== 32) return "bad signature";
  let key;
  try {
    key = await hmacKey(wakeKey, "verify");
  } catch {
    return "bad key";
  }
  const text = await canonical(method, path, timestamp, body);
  // subtle.verify compares in constant time.
  const ok = await crypto.subtle.verify("HMAC", key, mac, encoder.encode(text));
  return ok ? null : "bad signature";
}

// ── routing ──────────────────────────────────────────────────────────────────

/** `{deviceId, action}`, `{status}` for a known route with the wrong method, or `null`. */
export function route(method, pathname) {
  const match = /^\/v1\/boxes\/([^/]+)\/([a-z]+)$/.exec(pathname);
  if (!match || !DEVICE_ID.test(match[1]) || !ACTIONS.has(match[2])) return null;
  if (method !== "POST") return { status: 405 };
  return { deviceId: match[1], action: match[2] };
}

/** The `ZERON_BOXES` secret: `{deviceId: {wakeKey, credential, idleMinutes?}}`. */
export function parseBoxes(secret) {
  try {
    const boxes = JSON.parse(secret ?? "{}");
    return boxes && typeof boxes === "object" && !Array.isArray(boxes) ? boxes : {};
  } catch {
    return {};
  }
}

/** Delete a page of the box's checkpoints. Call until `done`. */
export async function purge(bucket, deviceId, pages = 5) {
  const prefix = `boxes/${deviceId}/`;
  let deleted = 0;
  for (let page = 0; page < pages; page++) {
    const listed = await bucket.list({ prefix, limit: 1000 });
    const keys = listed.objects.map((o) => o.key);
    if (keys.length) await bucket.delete(keys);
    deleted += keys.length;
    if (!listed.truncated) return { deleted, done: true };
  }
  return { deleted, done: false };
}

// ── the box (§3) ─────────────────────────────────────────────────────────────

/**
 * What the alarm does: `"renew"` keeps the box awake for another round,
 * `"sleep"` checkpoints and stops it, `"stopped"` records that it already
 * stopped. `status` is the engine's `/zeron/cloud/status` reply, or `null`
 * when it didn't answer.
 */
export function decideAlarm({ running, status, now, idleMs, startedAt, lastOkAt }) {
  if (!running) return "stopped";
  if (!status) {
    // A booting engine (restoring checkpoints) may not answer for a while;
    // one that stopped answering gets the idle window before it's put down.
    const since = Math.max(lastOkAt ?? 0, startedAt ?? 0);
    return now - since < Math.max(idleMs, BOOT_GRACE_MS) ? "renew" : "sleep";
  }
  if (status.busy) return "renew";
  const last = Number(status.lastActivityAt) || 0;
  return now - last < idleMs ? "renew" : "sleep";
}

export const idleMs = (box) => (Number(box.idleMinutes) || DEFAULT_IDLE_MINUTES) * 60_000;

export function randomToken() {
  const bytes = new Uint8Array(32);
  crypto.getRandomValues(bytes);
  return toHex(bytes);
}

// ── checkpoint store (§5) ────────────────────────────────────────────────────

/** The R2 key for a checkpoint request, or the HTTP status refusing it. */
export function checkpointKey(deviceId, method, pathname) {
  if (!DEVICE_ID.test(deviceId ?? "")) return { status: 403 };
  let kind = null;
  if (/^\/objects\/[0-9a-f]{64}$/.test(pathname)) kind = "object";
  else if (/^\/manifests\/\d{1,20}\.json$/.test(pathname)) kind = "manifest";
  else if (pathname === "/HEAD") kind = "head";
  if (!kind) return { status: 404 };
  const allowed = kind === "object" ? ["GET", "HEAD", "PUT"] : ["GET", "PUT"];
  if (!allowed.includes(method)) return { status: 405 };
  return { key: `boxes/${deviceId}${pathname}`, kind };
}

export async function handleCheckpoint(request, bucket, deviceId) {
  const url = new URL(request.url);
  const target = checkpointKey(deviceId, request.method, url.pathname);
  if (target.status) return new Response(null, { status: target.status });
  const { key, kind } = target;

  if (request.method === "HEAD") {
    const found = await bucket.head(key);
    return new Response(null, {
      status: found ? 200 : 404,
      headers: found ? { "content-length": String(found.size) } : {},
    });
  }
  if (request.method === "GET") {
    const found = await bucket.get(key);
    if (!found) return new Response(null, { status: 404 });
    return new Response(found.body, { headers: { "content-length": String(found.size) } });
  }

  // PUT
  const length = Number(request.headers.get("content-length") ?? NaN);
  if (length > MAX_OBJECT_BYTES) return new Response(null, { status: 413 });
  if (kind === "head") {
    const text = await request.text();
    let seq;
    try {
      seq = JSON.parse(text).seq;
    } catch {
      seq = undefined;
    }
    if (!Number.isSafeInteger(seq) || seq < 0) return json({ error: "HEAD must be {\"seq\": n}" }, 400);
    await bucket.put(key, JSON.stringify({ seq }));
    return json({ seq });
  }
  if (kind === "object" && (await bucket.head(key))) {
    // Content-addressed: the same name is the same bytes.
    await request.body?.cancel();
    return json({ existed: true });
  }
  const body = Number.isFinite(length) ? request.body : await request.arrayBuffer();
  if (!Number.isFinite(length) && body.byteLength > MAX_OBJECT_BYTES) {
    return new Response(null, { status: 413 });
  }
  // R2 checks the content against the name.
  const options = kind === "object" ? { sha256: url.pathname.slice("/objects/".length) } : {};
  try {
    await bucket.put(key, body ?? new ArrayBuffer(0), options);
  } catch (error) {
    return json({ error: String(error?.message ?? error) }, 400);
  }
  return json({ existed: false });
}

