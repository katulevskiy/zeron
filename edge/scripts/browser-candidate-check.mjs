// Real local Worker + real DO HTTP contract. Not a real-engine/WorkOS smoke.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { setTimeout as sleep } from "node:timers/promises";
import { rm } from "node:fs/promises";

const port = Number(process.env.BROWSER_CHECK_PORT ?? 27751);
const base = `http://127.0.0.1:${port}`;
const state = `.wrangler/browser-candidate-check-${crypto.randomUUID()}`;
const worker = spawn(process.execPath, ["node_modules/wrangler/bin/wrangler.js", "dev", "--local", "--ip", "127.0.0.1", "--port", String(port), "--local-upstream", `127.0.0.1:${port}`, "--upstream-protocol", "http", "--persist-to", state, "-c", "wrangler.browser-candidate.jsonc", "--var", "AUTH_MODE:dev", "--var", `BROWSER_DEV_ORIGIN:${base}`, "--var", "BROWSER_DEV_OWNER_SUBJECT:browser-e2e-owner", "--var", "BROWSER_SESSION_KEY:loopback-fixture-only"], { stdio: ["ignore", "pipe", "pipe"] });
let log = "";
worker.stdout.on("data", (data) => { log += data; });
worker.stderr.on("data", (data) => { log += data; });
const request = (path, init = {}) => fetch(`${base}${path}`, { ...init, signal: AbortSignal.timeout(5000) });
const sockets = [];
const deadline = Date.now() + 45_000;
try {
  for (;;) {
    try { if ((await request("/health")).ok) break; } catch { /* wait for local startup */ }
    assert(Date.now() < deadline && worker.exitCode === null, "local worker did not become ready");
    await sleep(200);
  }
  assert.deepEqual(await (await request("/api/browser/session")).json(), { authenticated: false });
  assert.equal((await request("/api/browser/login", { method: "POST", headers: { origin: base } })).status, 501);
  assert.equal((await request("/api/browser/dev-login", { method: "POST", headers: { origin: "http://evil.invalid" } })).status, 403);
  const login = await request("/api/browser/dev-login", { method: "POST", headers: { origin: base } });
  assert.equal(login.status, 200);
  const cookie = login.headers.get("set-cookie").split(";")[0];
  assert(login.headers.get("set-cookie").includes("HttpOnly"));
  assert(!login.headers.get("set-cookie").includes("Secure"));
  const identity = await login.json();
  const headers = { cookie, origin: base, "x-csrf-token": identity.csrfToken };
  assert.equal((await (await request("/api/browser/session", { headers })).json()).ownerId, "browser-e2e-owner");
  const device = `candidate-${crypto.randomUUID()}`;
  const host = new WebSocket(`${base.replace("http", "ws")}/device/${device}/ws?role=host&token=browser-e2e-owner&name=Candidate`);
  sockets.push(host);
  await Promise.race([once(host, "open"), sleep(5000).then(() => { throw new Error("native host upgrade timed out"); })]);
  assert.deepEqual(await (await request("/api/browser/devices", { headers })).json(), { devices: [{ id: device, name: "Candidate", online: true }] });
  const forged = await request(`/device/${device}/status?token=other-owner`, { headers: { "x-zeron-auth-user": "browser-e2e-owner", "x-zeron-room-kind": "workspace", "x-comet-browser-session-store": "1" } });
  assert.equal(forged.status, 403);
  assert.equal((await request("/api/browser/activity", { method: "POST", headers: { cookie, origin: base } })).status, 401);
  assert.equal((await request("/api/browser/activity", { method: "POST", headers: { ...headers, origin: "http://evil.invalid" } })).status, 401);
  assert.equal((await request("/api/browser/activity", { method: "POST", headers })).status, 200);
  for (const path of ["/api/browser/unknown", "/tail/private", "/blob/private/part"]) {
    const response = await request(path, { headers: { accept: "text/html" } });
    assert(response.status >= 400);
    assert(response.headers.get("content-type").includes("application/json"));
  }
  const deepLink = await request("/settings/accounts", { headers: { accept: "text/html" } });
  assert.equal(deepLink.status, 200);
  assert(deepLink.headers.get("content-type").includes("text/html"));
  assert.equal((await request("/api/browser/logout", { method: "POST", headers })).status, 200);
  assert.deepEqual(await (await request("/api/browser/session", { headers })).json(), { authenticated: false });
  assert.equal(host.readyState, WebSocket.OPEN);
  console.log("PASS real Wrangler/workerd: loopback cookie/session, native host registration + owned discovery, forged identity denied, Origin/CSRF, API-not-SPA, React deep link, durable logout preserves host");
} catch (error) {
  console.error(log);
  throw error;
} finally {
  for (const socket of sockets) socket.close();
  worker.kill("SIGTERM");
  if (worker.exitCode === null) await once(worker, "exit").catch(() => {});
  await rm(state, { recursive: true, force: true });
}
