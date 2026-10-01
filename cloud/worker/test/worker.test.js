import { test } from "node:test";
import assert from "node:assert/strict";
import { createHash } from "node:crypto";

import {
  sign,
  verifySignature,
  route,
  parseBoxes,
  checkpointKey,
  decideAlarm,
  purge,
  ALARM_MS,
  BOOT_GRACE_MS,
} from "../core.js";

// worker.js imports `cloudflare:workers`, stubbed by test/register.mjs.
const worker = await import("../worker.js");
const { ZeronBox, R2Gateway } = worker;

// Shared with crates/cloud/src/sign.rs.
const KEY = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=";
const SIG = "110b15f3785ac18344494e97a37359f8bd9a8785b91bc421938a3f587a13efa7";
const TS = 1_700_000_000_000;
const PATH = "/v1/boxes/dev-1/wake";
const body = (text) => new TextEncoder().encode(text);
const sha = (bytes) => createHash("sha256").update(bytes).digest("hex");

// ── fakes ────────────────────────────────────────────────────────────────────

class MemoryBucket {
  constructor() {
    this.objects = new Map();
  }
  async head(key) {
    const o = this.objects.get(key);
    return o ? { key, size: o.byteLength } : null;
  }
  async get(key) {
    const o = this.objects.get(key);
    return o ? { key, size: o.byteLength, body: new Blob([o]).stream() } : null;
  }
  async put(key, value, options = {}) {
    const bytes = new Uint8Array(await new Response(value).arrayBuffer());
    if (options.sha256 && sha(bytes) !== options.sha256) throw new Error("sha256 mismatch");
    this.objects.set(key, bytes);
  }
  async list({ prefix, limit }) {
    const keys = [...this.objects.keys()].filter((k) => k.startsWith(prefix)).sort();
    return { objects: keys.slice(0, limit).map((key) => ({ key })), truncated: keys.length > limit };
  }
  async delete(keys) {
    for (const key of [].concat(keys)) this.objects.delete(key);
  }
}

class FakeContainer {
  constructor() {
    this.running = false;
    this.calls = [];
    this.engine = { busy: false, lastActivityAt: 0 };
    this.answer = true;
  }
  start(options) {
    this.calls.push(["start", options]);
    this.running = true;
  }
  setInactivityTimeout(ms) {
    this.calls.push(["setInactivityTimeout", ms]);
  }
  interceptOutboundHttp(host, fetcher) {
    this.calls.push(["interceptOutboundHttp", host, fetcher]);
  }
  signal(n) {
    this.calls.push(["signal", n]);
  }
  monitor() {
    return new Promise(() => {});
  }
  getTcpPort(port) {
    return {
      fetch: async (url, init) => {
        this.calls.push(["control", port, url, init.method, init.headers["x-zeron-control"]]);
        if (!this.answer) throw new Error("connection refused");
        if (url.endsWith("/zeron/cloud/status")) return Response.json(this.engine);
        return Response.json({ seq: 1, objects: 0, bytes: 0 });
      },
    };
  }
  named(name) {
    return this.calls.filter((c) => c[0] === name);
  }
}

function fakeBox(container = new FakeContainer()) {
  const data = new Map();
  let alarm = null;
  const ctx = {
    container,
    storage: {
      get: async (k) => structuredClone(data.get(k)),
      put: async (k, v) => void data.set(k, structuredClone(v)),
      getAlarm: async () => alarm,
      setAlarm: async (at) => void (alarm = at),
    },
    exports: { R2Gateway: ({ props }) => ({ gateway: true, props }) },
    blockConcurrencyWhile: (fn) => fn(),
  };
  const box = new ZeronBox(ctx, { ZERON_EDGE_URL: "https://edge.example" });
  return { box, container, data, alarm: () => alarm };
}

async function signedRequest(path, { key = KEY, method = "POST", text = "{}", ts = Date.now() } = {}) {
  return new Request(`https://zeron-cloud.sub.workers.dev${path}`, {
    method,
    body: method === "GET" ? undefined : text,
    headers: {
      "x-zeron-timestamp": String(ts),
      "x-zeron-signature": await sign(key, method, path, ts, body(text)),
    },
  });
}

// ── signatures ───────────────────────────────────────────────────────────────

test("signatures match the shared vector", async () => {
  assert.equal(await sign(KEY, "POST", PATH, TS, body("{}")), SIG);
  assert.equal(await sign(KEY, "post", PATH, TS, body("{}")), SIG);
});

test("signature check accepts genuine requests and rejects the rest", async () => {
  const check = (over) =>
    verifySignature({
      wakeKey: KEY,
      method: "POST",
      path: PATH,
      timestamp: String(TS),
      signature: SIG,
      body: body("{}"),
      now: TS + 1_000,
      ...over,
    });
  assert.equal(await check({}), null);
  assert.equal(await check({ body: body('{"a":1}') }), "bad signature");
  assert.equal(await check({ path: "/v1/boxes/dev-2/wake" }), "bad signature");
  assert.equal(await check({ method: "PUT" }), "bad signature");
  assert.equal(await check({ signature: SIG.replace(/^./, "0") }), "bad signature");
  assert.equal(await check({ signature: "zz" }), "bad signature");
  assert.equal(await check({ now: TS + 5 * 60_000 + 1 }), "stale timestamp");
  assert.equal(await check({ now: TS - 5 * 60_000 - 1 }), "stale timestamp");
  assert.equal(await check({ timestamp: "" }), "bad timestamp");
  assert.equal(await check({ timestamp: null }), "bad timestamp");
  assert.equal(await check({ wakeKey: "c2hvcnQ=" }), "bad key");
});

// ── routing ──────────────────────────────────────────────────────────────────

test("routes", () => {
  assert.deepEqual(route("POST", "/v1/boxes/dev-1/wake"), { deviceId: "dev-1", action: "wake" });
  assert.deepEqual(route("POST", "/v1/boxes/dev-1/status"), { deviceId: "dev-1", action: "status" });
  assert.deepEqual(route("GET", "/v1/boxes/dev-1/stop"), { status: 405 });
  assert.equal(route("POST", "/v1/boxes/dev-1/reboot"), null);
  assert.equal(route("POST", "/v1/boxes/a%2Fb/wake"), null);
  assert.equal(route("POST", "/v1/boxes//wake"), null);
  assert.equal(route("POST", "/v2/boxes/dev-1/wake"), null);
  assert.deepEqual(parseBoxes("not json"), {});
  assert.deepEqual(parseBoxes("[1]"), {});
  assert.deepEqual(parseBoxes(undefined), {});
});

test("the fetch handler checks the box's key, then calls its Durable Object", async () => {
  const calls = [];
  const bucket = new MemoryBucket();
  await bucket.put("boxes/dev-1/HEAD", "{}");
  await bucket.put("boxes/dev-2/HEAD", "{}");
  const env = {
    ZERON_BOXES: JSON.stringify({ "dev-1": { wakeKey: KEY, credential: "cred-1", idleMinutes: 20 } }),
    CKPT: bucket,
    BOX: {
      getByName: (name) => ({
        wake: async (spec) => (calls.push(["wake", name, spec]), { state: "waking" }),
        stop: async () => (calls.push(["stop", name]), { state: "checkpointing" }),
        status: async () => (calls.push(["status", name]), { state: "running", busy: true }),
      }),
    },
  };
  const handler = worker.default;

  let res = await handler.fetch(await signedRequest("/v1/boxes/dev-1/wake"), env);
  assert.equal(res.status, 200);
  assert.deepEqual(await res.json(), { state: "waking" });
  assert.deepEqual(calls.pop(), ["wake", "dev-1", { deviceId: "dev-1", credential: "cred-1", idleMinutes: 20 }]);

  res = await handler.fetch(await signedRequest("/v1/boxes/dev-1/status"), env);
  assert.deepEqual(await res.json(), { state: "running", busy: true });
  assert.deepEqual(calls.pop(), ["status", "dev-1"]);

  // Another key, an old timestamp, an unknown box, a wrong method.
  const otherKey = btoa(String.fromCharCode(...new Uint8Array(32).fill(7)));
  res = await handler.fetch(await signedRequest("/v1/boxes/dev-1/stop", { key: otherKey }), env);
  assert.equal(res.status, 401);
  res = await handler.fetch(await signedRequest("/v1/boxes/dev-1/stop", { ts: Date.now() - 6 * 60_000 }), env);
  assert.equal(res.status, 401);
  res = await handler.fetch(await signedRequest("/v1/boxes/dev-9/wake"), env);
  assert.equal(res.status, 404);
  res = await handler.fetch(await signedRequest("/v1/boxes/dev-1/wake", { method: "GET" }), env);
  assert.equal(res.status, 405);
  assert.equal(calls.length, 0);

  res = await handler.fetch(await signedRequest("/v1/boxes/dev-1/purge"), env);
  assert.deepEqual(await res.json(), { deleted: 1, done: true });
  assert.deepEqual([...bucket.objects.keys()], ["boxes/dev-2/HEAD"]);
});

test("purge pages through a box's prefix only", async () => {
  const bucket = new MemoryBucket();
  for (let i = 0; i < 2500; i++) bucket.objects.set(`boxes/dev-1/objects/${i}`, new Uint8Array());
  bucket.objects.set("boxes/dev-10/HEAD", new Uint8Array());
  assert.deepEqual(await purge(bucket, "dev-1", 2), { deleted: 2000, done: false });
  assert.deepEqual(await purge(bucket, "dev-1", 2), { deleted: 500, done: true });
  assert.deepEqual([...bucket.objects.keys()], ["boxes/dev-10/HEAD"]);
});

// ── checkpoint store ─────────────────────────────────────────────────────────

test("checkpoint keys stay inside boxes/{deviceId}/", () => {
  const h = "a".repeat(64);
  assert.deepEqual(checkpointKey("dev-1", "PUT", `/objects/${h}`), { key: `boxes/dev-1/objects/${h}`, kind: "object" });
  assert.deepEqual(checkpointKey("dev-1", "HEAD", `/objects/${h}`), { key: `boxes/dev-1/objects/${h}`, kind: "object" });
  assert.deepEqual(checkpointKey("dev-1", "GET", "/manifests/12.json"), { key: "boxes/dev-1/manifests/12.json", kind: "manifest" });
  assert.deepEqual(checkpointKey("dev-1", "PUT", "/HEAD"), { key: "boxes/dev-1/HEAD", kind: "head" });
  for (const path of [
    "/objects/../../dev-2/HEAD",
    `/objects/${"A".repeat(64)}`,
    "/objects/abc",
    "/manifests/../HEAD",
    "/manifests/x.json",
    "/HEAD/x",
    "/",
    "/boxes/dev-2/HEAD",
  ]) {
    assert.deepEqual(checkpointKey("dev-1", "GET", path), { status: 404 }, path);
  }
  assert.deepEqual(checkpointKey("dev-1", "DELETE", "/HEAD"), { status: 405 });
  assert.deepEqual(checkpointKey("dev-1", "HEAD", "/HEAD"), { status: 405 });
  assert.deepEqual(checkpointKey("../dev-2", "GET", "/HEAD"), { status: 403 });
  assert.deepEqual(checkpointKey("a/b", "GET", "/HEAD"), { status: 403 });
  assert.deepEqual(checkpointKey(undefined, "GET", "/HEAD"), { status: 403 });
});

test("the gateway stores objects, manifests and HEAD", async () => {
  const bucket = new MemoryBucket();
  const gateway = new R2Gateway({ props: { deviceId: "dev-1" } }, { CKPT: bucket });
  const call = (method, path, data) =>
    gateway.fetch(new Request(`http://r2.zeron.internal${path}`, { method, body: data }));

  const bytes = body("pack bytes");
  const name = sha(bytes);
  assert.equal((await call("HEAD", `/objects/${name}`)).status, 404);
  let res = await call("PUT", `/objects/${name}`, bytes);
  assert.deepEqual(await res.json(), { existed: false });
  assert.deepEqual([...bucket.objects.keys()], [`boxes/dev-1/objects/${name}`]);
  res = await call("PUT", `/objects/${name}`, bytes);
  assert.deepEqual(await res.json(), { existed: true });
  assert.equal((await call("HEAD", `/objects/${name}`)).status, 200);
  res = await call("GET", `/objects/${name}`);
  assert.equal(await res.text(), "pack bytes");

  // Content must match its name.
  res = await call("PUT", `/objects/${"b".repeat(64)}`, bytes);
  assert.equal(res.status, 400);

  res = await call("PUT", "/manifests/3.json", '{"seq":3}');
  assert.equal(res.status, 200);
  assert.equal(await (await call("GET", "/manifests/3.json")).text(), '{"seq":3}');
  assert.equal((await call("GET", "/manifests/4.json")).status, 404);

  assert.equal((await call("PUT", "/HEAD", '{"seq":-1}')).status, 400);
  assert.equal((await call("PUT", "/HEAD", "nope")).status, 400);
  assert.equal((await call("PUT", "/HEAD", '{"seq":3}')).status, 200);
  assert.deepEqual(await (await call("GET", "/HEAD")).json(), { seq: 3 });

  assert.equal((await call("GET", "/../dev-2/HEAD")).status, 404);
  const stranger = new R2Gateway({ props: {} }, { CKPT: bucket });
  assert.equal((await stranger.fetch(new Request("http://r2.zeron.internal/HEAD"))).status, 403);
});

// ── the alarm ────────────────────────────────────────────────────────────────

test("alarm decisions", () => {
  const idleMs = 15 * 60_000;
  const now = 10_000_000_000;
  const at = (over) => decideAlarm({ running: true, status: null, now, idleMs, startedAt: now, lastOkAt: null, ...over });
  assert.equal(at({ running: false }), "stopped");
  assert.equal(at({ status: { busy: true, lastActivityAt: 0 } }), "renew");
  assert.equal(at({ status: { busy: false, lastActivityAt: now - idleMs + 1 } }), "renew");
  assert.equal(at({ status: { busy: false, lastActivityAt: now - idleMs } }), "sleep");
  assert.equal(at({ status: { busy: false } }), "sleep");
  // Silent engine: booting is fine for a while, a hung one is put down.
  assert.equal(at({ startedAt: now - 60_000 }), "renew");
  assert.equal(at({ startedAt: now - BOOT_GRACE_MS - idleMs }), "sleep");
  assert.equal(at({ startedAt: now - 2 * idleMs, lastOkAt: now - 1000 }), "renew");
  assert.equal(at({ idleMs: 60_000, startedAt: now - BOOT_GRACE_MS + 1 }), "renew");
});

test("wake starts the container with its environment, timeout, gateway and alarm", async () => {
  const { box, container, data, alarm } = fakeBox();
  const before = Date.now();
  assert.deepEqual(await box.wake({ deviceId: "dev-1", credential: "cred", idleMinutes: 20 }), { state: "waking" });
  const [[, options]] = container.named("start");
  assert.equal(options.enableInternet, true);
  const token = data.get("box").controlToken;
  assert.match(token, /^[0-9a-f]{64}$/);
  assert.deepEqual(options.env, {
    ZERON_DEVICE_ID: "dev-1",
    ZERON_DEVICE_CREDENTIAL: "cred",
    ZERON_EDGE_URL: "https://edge.example",
    ZERON_CONTROL_TOKEN: token,
    ZERON_CLOUD_CONTROL_PORT: "8787",
    ZERON_CHECKPOINT_URL: "http://r2.zeron.internal",
  });
  assert.deepEqual(container.named("setInactivityTimeout"), [["setInactivityTimeout", 20 * 60_000]]);
  assert.deepEqual(container.named("interceptOutboundHttp"), [
    ["interceptOutboundHttp", "r2.zeron.internal", { gateway: true, props: { deviceId: "dev-1" } }],
  ]);
  assert.ok(alarm() >= before + ALARM_MS);

  // A second wake while running doesn't start another container.
  assert.deepEqual(await box.wake({ deviceId: "dev-1", credential: "cred", idleMinutes: 20 }), { state: "waking" });
  assert.equal(container.named("start").length, 1);
});

test("the alarm renews a busy box and puts an idle one to sleep", async () => {
  const { box, container, data, alarm } = fakeBox();
  await box.wake({ deviceId: "dev-1", credential: "cred", idleMinutes: 15 });
  const token = data.get("box").controlToken;

  container.engine = { busy: true, lastActivityAt: 0 };
  await box.alarm();
  assert.equal(data.get("box").state, "running");
  assert.equal(container.named("signal").length, 0);
  assert.equal(container.named("setInactivityTimeout").length, 2);
  assert.ok(alarm() > Date.now());
  const [, port, url, method, sentToken] = container.named("control")[0];
  assert.deepEqual([port, url, method, sentToken], [8787, "http://container/zeron/cloud/status", "GET", token]);

  container.engine = { busy: false, lastActivityAt: Date.now() - 16 * 60_000 };
  await box.alarm();
  const controls = container.named("control").map((c) => `${c[3]} ${c[2]}`);
  assert.equal(controls.at(-1), "POST http://container/zeron/cloud/checkpoint?final=1");
  assert.deepEqual(container.named("signal"), [["signal", 15]]);
  assert.equal(data.get("box").state, "asleep");

  container.running = false;
  assert.deepEqual(await box.status(), { state: "asleep", startedAt: data.get("box").startedAt, lastActivityAt: data.get("box").lastActivityAt, busy: false });
});

test("stop checkpoints and stops on the next alarm", async () => {
  const { box, container, data, alarm } = fakeBox();
  await box.wake({ deviceId: "dev-1", credential: "cred", idleMinutes: 15 });
  container.engine = { busy: true, lastActivityAt: Date.now() };
  assert.deepEqual(await box.stop(), { state: "checkpointing" });
  assert.ok(alarm() <= Date.now());
  await box.alarm();
  // Even though the engine is busy: the user asked.
  assert.deepEqual(container.named("signal"), [["signal", 15]]);
  assert.equal(data.get("box").state, "asleep");
  assert.equal(data.get("box").stopRequested, false);
});

test("status reports a running engine and the Durable Object re-arms after a restart", async () => {
  const container = new FakeContainer();
  const first = fakeBox(container);
  await first.box.wake({ deviceId: "dev-1", credential: "cred", idleMinutes: 15 });
  container.engine = { busy: false, lastActivityAt: 123 };
  const status = await first.box.status();
  assert.equal(status.state, "running");
  assert.equal(status.lastActivityAt, 123);

  // A new instance over the same storage, container still running.
  const restarted = new ZeronBox(
    {
      container,
      storage: { get: async () => first.data.get("box") },
      exports: { R2Gateway: ({ props }) => ({ gateway: true, props }) },
      blockConcurrencyWhile: (fn) => fn(),
    },
    {},
  );
  await new Promise((r) => setTimeout(r, 0));
  assert.equal(restarted instanceof ZeronBox, true);
  assert.equal(container.named("setInactivityTimeout").length, 2);
  assert.equal(container.named("interceptOutboundHttp").length, 2);
});
