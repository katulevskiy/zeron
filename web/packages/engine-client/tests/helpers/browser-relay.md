# Real browser relay fixture

`startBrowserRelayFixture` in `browser-relay.ts` launches the **actual edge Worker through Wrangler/workerd**, its real BrowserSessionStore/DeviceRoom/registry/session Durable Objects, and two genuine EngineCore processes using the supported outbound host relay. There is no direct engine listener, fake dispatcher, fake relay, or fake session store. Only the existing MockHarness is scripted.

## Public interface for tickets 08–11

```ts
import { once } from "node:events";
import { startBrowserRelayFixture } from "./helpers/browser-relay";

const fixture = await startBrowserRelayFixture({
  engineLabels: ["engine-a", "engine-b"], // default
  mockDelayMs: 80, // per-event pacing in the existing MockHarness
  // assetsDirectory: "/absolute/path/to/web/packages/app/dist",
});
try {
  const [a, b] = fixture.engines;
  const session = fixture.session;
  const devices = await (await session.request("/api/browser/devices")).json();
  const rawRealSocket = session.openSocket(a!.deviceId);
  await once(rawRealSocket, "open");
  // Consume with the actual product DeviceFrame/RPC codecs, then close it.
  rawRealSocket.terminate();
  await a!.disconnect(); // stop only this engine's real host relay
  await fixture.waitForDevice(session, a!, false);
  await a!.reconnect();
  await fixture.waitForDevice(session, a!);
  await a!.stop(); // stop the entire EngineCore process
  await a!.restart(); // same owned storage/device identity/project/chat
  await fixture.waitForDevice(session, a!);
} finally {
  await fixture.stop(); // idempotent; engines, workerd process group, owned storage
}
```

- `fixture.origin`: isolated loopback edge origin; use for page navigation and API requests. Engines have no HTTP/IPC endpoints: `session.relayUrl(engine.deviceId)` is their actual cookie-only edge WebSocket endpoint.
- Each engine exposes `deviceId`, `ownerId`, `organizationId`, `label`, real `projectRoot` (initialized Git repository with an untracked fixture file), seeded `chatId`/`spaceId`, current `child`, and lifecycle methods above. No fixture Git commits are made.
- `session.request(path, init?)`: actual cookie-authenticated HTTP fetch, same-origin only. For mutations supply the actual `session.csrfToken` in `x-csrf-token`; the fixture does not bypass CSRF.
- `session.openSocket(id)`: existing Node `ws` client with actual Cookie/Origin upgrade headers. It is a raw client connection, not a replacement protocol adapter. `browser-relay.test.ts` demonstrates EngineInfo using production codecs.
- `session.browserCookies()`: actual minted cookie parameters for `await context.addCookies(...)` in Playwright. The cookie is HttpOnly/Strict. Keep test-runner cookie strings out of application JavaScript. Browser code uses the native WebSocket and production RelaySocket normally.
- `fixture.addEngine({label, ownerId?, organizationId?, mockDelayMs?})`: additional real isolated engine; label must be unique. Call `waitForDevice` with its owner's session after launch.
- `fixture.loginAs(owner, organization?)`: change only loopback development-login configuration, restart the actual edge worker at the same origin with the same persistent DO storage/session encryption key, and mint a real cookie for the new owner. Existing session handles retain their cookies; real ownership checks still reject cross-account sockets. This is useful for deterministic session/account/cache-switch tests, not a WorkOS exchange emulator.
- Optional `assetsDirectory` selects an **actual built app** for rendered same-origin tests. This fixture suite itself verifies JS transport, not rendered UI; it does not fabricate app assets.

## Run

Prerequisites: local edge dependencies installed by the backend/parent worker (`edge/node_modules/wrangler`), workspace JS dependencies, Node 24, Cargo and Git. No extra Cargo feature is required. From the repository root (shell timeout 600 seconds):

```sh
# Ensure Node 24 and pnpm are available on PATH.
cd web/packages/engine-client
pnpm exec tsc --noEmit
CARGO_BUILD_JOBS=2 \
  pnpm exec vitest run --config tests/browser-relay.config.ts --maxWorkers=2 --minWorkers=1 --sequence.shuffle --sequence.seed=12
```

The six test scenarios each own an independent real environment, including failure/account/teardown cases; filtered or shuffled execution does not require preceding scenarios.

The fixture builds `cargo build --locked -p zeron-engine --example browser_relay`, resolving Cargo targets via metadata. Cargo's default target directory is supported; optionally set `CARGO_TARGET_DIR` to a writable build-cache directory. All persistent fixture files are under a uniquely created repository `.scratch/browser-relay-*` directory and are removed only after successful teardown. Startup/control failures include child output; process cleanup uses bounded lifecycle helpers. Readiness: engine 15s, edge 60s, device registration 30s; controls 5s. Graceful EOF shutdown gets 1s, then POSIX process-group termination escalates after 1s with a 3s teardown deadline.

## Loopback configuration and security boundary

Wrangler must use `--local --local-upstream 127.0.0.1:<public port> --upstream-protocol http --ip 127.0.0.1 --port <public port> --inspector-port 0`. The full upstream authority is essential: the default deployed hostname fails the loopback guard (501), and omitting the public port rewrites Origin and fails the exact-origin check (403). Neither guard should be weakened for this fixture.

Generated local variables: `AUTH_MODE=dev`, exact `BROWSER_DEV_ORIGIN`, `BROWSER_DEV_OWNER_SUBJECT`, `BROWSER_DEV_ORGANIZATION_ID`, and a random 32-byte/base64 `BROWSER_SESSION_KEY`. Engines authenticate their native host/registry/session joins as `owner@organization`. Browser clients authenticate **only with the real HttpOnly session cookie** minted at `/api/browser/dev-login`, then use `/api/browser/device/:deviceId/ws`. No provider secret or remote deployment is required.

This verifies genuine cookie/session/ownership/relay transport in loopback development mode. It does **not** verify WorkOS PKCE/JWT/provider revocation, production TLS/Secure-cookie behavior, or visual app flows; those belong to the backend security and rendered-flow suites. Linux execution is verified; Windows/macOS execution is not. Fixtures allocate independent loopback ports and stop only their own processes.
