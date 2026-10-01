# React browser candidate

This is PR #526's local relay-only React candidate, not a deployed replacement for the canonical #319 browser UI. The separate browser/GPUI and Kratos checkout histories remain independent. Backend provenance, exact bindings, cookie/Origin/CSRF rules, and a loopback launch command are documented in [`../edge/BROWSER-CANDIDATE.md`](../edge/BROWSER-CANDIDATE.md).

## Toolchain and setup

Use **Node 24+**, **pnpm 11.17.0** (pinned in `package.json`), and the repository's pinned Rust toolchain. From the repository root:

```sh
pnpm -C web install --frozen-lockfile
npm ci --prefix edge --ignore-scripts --no-audit --no-fund
pnpm -C web typecheck
pnpm -C web build
pnpm -C web/packages/app exec playwright install --with-deps chromium
```

The candidate Worker serves the built `web/packages/app/dist` and browser APIs/WebSockets at **one explicitly configured origin**. WorkOS requires server-only secrets, the session-store binding/migration, registered callback and host engines connected to the same environment. Missing backend configuration fails closed rather than serving SPA HTML or mounting private engine views. Use the isolated candidate config; do not register production hostnames or deploy it without a separate rollout decision.

For frontend development, `ZERON_DEV_EDGE=http://127.0.0.1:<edge-port>` selects the browser-API proxy upstream before launching Vite. Its historical default is `http://127.0.0.1:27641`; no personal service is required if the variable is supplied. Configure the backend's explicit browser origin to match the origin the browser actually uses. Never expose `AUTH_MODE=dev` outside loopback, and never put provider tokens/session encryption keys in Vite environment variables.

Native engines **do not embed or serve this bundle**. Direct listener, pasted-code browser login and manual engine-address pairing are retired. Native IPC, native host authentication and the owner-scoped cookie relay remain supported. `ZERON_WEB_DIST` is not a native build prerequisite or supported bundle selector.

## Reproducible generation

```sh
cargo run --locked -p wiregen -- --check
cargo run --locked -p zeron-theme --bin zeron-theme-export -- --check
node web/packages/icons/scripts/generate.mjs --check
node web/packages/icons/scripts/generate-file-icons.mjs --check
```

Omit `--check` to regenerate deliberately. The generated core models follow compiled Rust derives; retained legacy declarations are explicitly outside full upstream/parity verification. See [`packages/proto/src/README.md`](packages/proto/src/README.md). Method inventory alone is not evidence that an RPC is dispatched or usable.

## Tests

After building the real app bundle, from the repository root:

```sh
pnpm -C web/packages/app exec vitest run --maxWorkers=2 --minWorkers=1
pnpm -C web/packages/engine-client exec vitest run --project unit
pnpm -C web/packages/engine-client exec vitest run --project conformance --project smoke --maxWorkers=2 --minWorkers=1
pnpm -C web/packages/engine-client exec vitest run --config wire-contract.config.ts
pnpm -C web/packages/engine-client test:relay
pnpm -C web/packages/engine-client exec vitest run --config tests/chat-relay.config.ts
pnpm -C web/packages/app test:projects
pnpm -C web/packages/engine-client test:sidebar
pnpm -C web/packages/engine-client test:sidebar-browser
npm test --prefix edge
npm run test:browser-candidate --prefix edge
cargo test --locked -p zeron-engine --test native_access
```

Real-relay suites start isolated loopback Wrangler/workerd instances and genuine Rust EngineCore host relays, use the existing deterministic MockHarness for prompt responses, and tear down only their own processes/storage. Allow up to ten minutes for a cold Rust fixture build; `CARGO_BUILD_JOBS=2` bounds compilation pressure. Chromium tests exercise the production bundle at desktop and phone sizes. They are not actual WorkOS/AuthKit sign-in, mobile software-keyboard, other native-platform or remote-CI verification. Signed-provider backend fixtures replace only external provider calls; the authorization/session/relay implementation is real.

Editor/terminal/preview and full mobile parity remain separate review slices. Project creation uses upstream managed `CreateRepo`, not arbitrary missing-path creation. Sidebar actions use upstream preference watches and per-item mutation intents; unsupported operations must not appear to succeed locally.
