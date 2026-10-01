# Browser wire contract boundary

`generated/` is produced by the runnable upstream `wiregen` crate, not by copying Roboco DTOs. `cargo run --locked -p wiregen` regenerates it; `cargo run --locked -p wiregen -- --check` compares a fresh isolated output with every checked-in generated file and exits nonzero on missing, changed or extra files. `cargo test --locked -p wiregen` exercises the real CLI, deterministic repeats and deliberate stale fixtures.

## Generated and checked

- Engine identity/scope, devices, spaces, chats/config/source context, live session status.
- Harness/model identifiers/options, RunRequest/worktree directive, normalized agent events and tool calls.
- Durable command kinds/payloads/entries; queued messages and delivery gates.
- SidebarPreferencesState/SidebarPreferences/SidebarSection and SidebarPinChange/SidebarSectionChange from actual upstream declarations. Section membership is wire `session_ids` (snake_case), distinct from the browser's camelCase cache projection.
- Transcript message/part/status types, reset/delta frames, upserts/appends, context usage and replay watermarks.
- UpdateStatus and actual protocol constants/capabilities.
- `methods.ts`: the declared RPC constants from the upstream Rust AST, the required core method list and the explicit fork-only call inventory. A declaration is **not** a promise of browser feature parity. The separate real-engine suite verifies dispatch and core payloads.

The CLI compiles `ts_rs::TS` derives on the owning proto/doc/update declarations. Serde attributes are unchanged. Explicit TS annotations model dynamic JSON as `unknown` and optional tool fields that serialize as absent even without `serde(default)`. Large integers use TypeScript `number`, matching JSON transport (callers must continue respecting JavaScript's safe-integer bound).

## Retained, not generated

`legacy/` contains retained browser declarations that have not yet migrated to upstream export coverage. Their public names remain exported where needed to keep the browser buildable, but their headers do not claim a freshness guarantee. There is no second copy of any generated type. Legacy dependencies on core contracts import `../generated/…`; retire unused declarations rather than preserving fake support.

The full retained inventory is the explicit `legacy/index.ts` export list. Categories: repository/folder/git/workspace-file models; previews; project actions/worktrees; terminals/uploads; connectivity; account/login/catalog settings; queue-edit outcomes; the engine-owned MutateParams union; remote-access models. Many have real upstream equivalents and supported methods, but are **unverified export coverage**, not necessarily unsupported engine features. Migrate one family deliberately using actual owning Rust declarations and remove its retained file; never copy a fork schema into `generated/`.

**Unsupported fork-only calls:** `PrepareSpacePath`, `SetSidebarPins`, `SetSidebarSections`, `WatchSidebarState`. Negative wire tests preserve these reproductions; they are not supported product actions. Ticket10 now uses upstream `ListFolders`, explicit managed `CreateRepo`, and `createSpace` instead of `PrepareSpacePath`; its unused reply shim and alias are retired. Ticket11 uses upstream `WatchSidebarPreferences` and `Mutate` with `{ op: "changeSidebarPin", change: SidebarPinChange }`, not whole-list replacement or invented UI-preference methods. The obsolete sidebar aliases and legacy SidebarStateSnapshot/SidebarSection declarations are retired. Browser presentation/cache SidebarSection remains a separate camelCase projection. The owning Rust SidebarSectionChange also declares Import; generation preserves that schema, but the browser bridge does not emit import or section-reorder actions. Generated method inventory is not proof of advanced parity.

`shims.ts` holds actual ad-hoc JSON reply wrappers and names for dynamic JSON concepts; these do not have independently exportable Rust wire declarations. Core reply shapes are asserted against the real engine in `web/packages/engine-client/tests/wire-contract.test.ts`. The engine-owned MutateParams/HarnessDescriptor declarations remain migration work; the core suite verifies the actual createChat/catalog payloads rather than pretending they were generated.

## Real core check

With Node 24 and browser dependencies installed:

```sh
CARGO_BUILD_JOBS=2 cargo test --locked -p wiregen -p zeron-proto -p zeron-doc -p zeron-update
pnpm -C web typecheck
pnpm -C web/packages/engine-client exec vitest run --config wire-contract.config.ts
```

The last command starts actual Wrangler/workerd session/device stores and a real EngineCore host relay with the existing scripted MockHarness. Production EngineClient/RelaySocket use the cookie-authenticated relay. It verifies core names against generated upstream names; identity, registry streams, project-less chat creation, Run command acknowledgement and response transcript; and genuine unknown-method/error framing for the four fork-only calls. It does not use a fake RPC server or prove live-provider sign-in, complete project/sidebar parity or rendered chat/reconnect by itself.
