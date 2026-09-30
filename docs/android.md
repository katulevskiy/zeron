# Android — agents that run on the phone

The Android app does both jobs:

1. **Remote control** — the same viewer the iOS app is: a `zeron-client` peer
   that drives engines on other devices through the production edge.
2. **On-device engine** — the phone runs its own `zeron headless` plus real
   agent CLIs (Claude Code, Codex, Grok, OpenCode…) inside a user-space Linux
   guest, so agents execute *on the phone* with no computer involved.

## Topology (on-device mode)

```
┌──────────── Android app process ────────────┐
│ Compose UI ── zeron-mobile (UniFFI, JNI .so) │
│                 └─ zeron-client (Live backend, Credentials::Local)
│                        │  ws/http 127.0.0.1:27655 + bearer token
│ RuntimeService (foreground, specialUse)      │
│   └─ spawns: libproot.so ─┐                  │
└───────────────────────────┼──────────────────┘
                            ▼  proot guest (Alpine arm64/x86_64 rootfs)
            /opt/zeron/lib/libzeron.so headless   (static musl engine)
              ├─ local edge   127.0.0.1:27655  (chat2 rooms, registry room,
              │                                 device relay — Rust port of edge/)
              ├─ engine IPC   127.0.0.1:27654  (token-gated)
              └─ harness subprocesses: claude / codex / grok / git / node …
```

The engine and the client speak **the exact production protocols** — chat2
rows, registry row-frames, DeviceRoom relay — to a *local edge* that the
engine hosts on loopback. Neither the engine's sync code nor the client's sync
code learns anything new; only the edge moved onto the phone. The same local
edge is the seed of the self-hosting contract ARCHITECTURE §1 defers.

### Why the engine runs inside the guest

Android forbids `execve` of app-writable files (targetSdk ≥ 29), which is what
every harness install writes. proot's loader maps guest programs instead of
exec'ing them, so everything the engine spawns (vendor installers, `node`,
`git`, the CLIs) just works. Running the engine itself inside the guest means
**zero engine changes for process spawning**: it believes it is on a Linux VPS.

Executables the app ships live in `nativeLibraryDir` (the only exec-allowed
location) under `lib*.so` names, extracted to disk
(`useLegacyPackaging = true`):

| File | What |
| --- | --- |
| `libproot.so` | Termux proot (bionic), NEEDED patched to `libtalloc.so`, RUNPATH removed |
| `libproot-loader.so`, `libproot-loader32.so` | proot loaders (`PROOT_LOADER`, `PROOT_LOADER_32`) |
| `libtalloc.so`, `libandroid-shmem.so` | proot's shared deps (SONAME patched) |
| `libzeron.so` | `zeron` built `--no-default-features` for `*-unknown-linux-musl` (static) |
| `libzeron_mobile.so` | the UniFFI core (loaded by JNA/System.loadLibrary, not exec'd) |

`scripts/android/fetch-proot.sh` downloads + patches proot;
`scripts/android/build-engine.sh` builds the musl engine with `cargo zigbuild`.

## Runtime contract

Host paths (app-private): `filesDir/runtime/`
- `rootfs/` — Alpine minirootfs, extracted from `assets/rootfs-<abi>.tar.gz`
- `tmp/` — `PROOT_TMP_DIR`
- `state.json` — bootstrap version, generated secrets

Guest layout:
- user `zeron` with the app's uid/gid (added to `/etc/passwd`), HOME `/home/zeron`
- `ZERON_DATA_DIR=/home/zeron/.zeron`
- projects default to `/home/zeron/projects`
- `nativeLibraryDir` bound at `/opt/zeron/lib`; `/usr/local/bin/zeron` → `/opt/zeron/lib/libzeron.so`

proot invocation (engine; `Guest.kt` is the source of truth):
```
libproot.so --kill-on-exit --link2symlink -r rootfs -w /home/zeron \
  -b /dev -b /proc -b /sys -b /dev/urandom:/dev/random \
  -b <filesDir>/runtime/tmp:/dev/shm -b /proc/self/fd:/dev/fd \
  -b <nativeLibraryDir>:/opt/zeron/lib -b <filesDir>/runtime/tmp:/tmp \
  [-b <filesDir>/runtime/proc/<f>:/proc/<f> for f in stat loadavg uptime vmstat, when SELinux hides them] \
  /usr/bin/env -i HOME=/home/zeron USER=zeron LOGNAME=zeron SHELL=/bin/bash \
    LANG=C.UTF-8 TERM=xterm-256color TMPDIR=/tmp USE_BUILTIN_RIPGREP=0 \
    PATH=/home/zeron/.local/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
    ZERON_DATA_DIR=/home/zeron/.zeron ZERON_DEVICE_NAME="<model>" \
    ZERON_DEVICE_PLATFORM=android ZERON_NO_LOGIN_SHELL=1 \
    ZERON_LOCAL_EDGE_PORT=27655 ZERON_LOCAL_EDGE_TOKEN=<secret> \
    ZERON_IPC_PORT=27654 ZERON_IPC_TOKEN=<secret2> \
    /opt/zeron/lib/libzeron.so headless
```
Android's seccomp policy rejects `fork`/`vfork` from apps on x86_64 (arm64 has
no such syscalls), which breaks every musl shell pipeline; `fetch-proot.sh`
therefore rebuilds the x86_64 proot from source with a patch that rewrites
them to `clone`. The arm64 proot is Termux's binary, patched only for loading.
Bootstrap-only package installs run with `-0` (fake root):
`apk add bash git nodejs npm curl ca-certificates ripgrep libgcc libstdc++ openssh-client procps coreutils python3 py3-pip make unzip less`.
Configuration also installs a no-op `/usr/local/bin/sudo` (every guest file belongs to the app uid, so `apk add` needs no privilege) and, when absent, global agent instructions describing the guest (`~/.claude/CLAUDE.md`, `~/.codex/AGENTS.md`, `~/.config/opencode/AGENTS.md`).
Nothing else runs as fake root — Claude Code refuses permission bypass as uid 0.

`/etc/resolv.conf` is written from the active network's DNS servers
(`ConnectivityManager.getLinkProperties`), falling back to 1.1.1.1 / 8.8.8.8.

### Engine switches (all opt-in; desktop behaviour unchanged)

| Env | Effect |
| --- | --- |
| `ZERON_LOCAL_EDGE_PORT` + `ZERON_LOCAL_EDGE_TOKEN` | `zeron headless` starts the embedded local edge on `127.0.0.1:<port>` and runs the engine against it in `Development` scope with that bearer (no WorkOS) |
| `ZERON_IPC_TOKEN` | the IPC server rejects upgrades without `?token=` / `Authorization: Bearer`; `zeron mcp` / `zeron sync` send it |
| `ZERON_DEVICE_PLATFORM` | overrides the platform string on this engine's device row |

Loopback on Android is shared by **every app on the device**, so both
listeners are token-gated. The app generates both secrets on first run
(`SecureRandom`, 32 bytes hex) and keeps them in app-private storage.

Local edge details (`crates/localedge`):
- Binds `127.0.0.1` only; state is one SQLite file,
  `$ZERON_DATA_DIR/local-edge/edge.db`, so rooms survive engine/app restarts.
- The token must be ≥ 16 URL-safe characters (`[A-Za-z0-9._~-]`; hex is
  fine) — the engine splices it into WebSocket URLs unencoded. `zeron headless`
  exits with an error otherwise. It is accepted as `Authorization: Bearer` or
  `?token=` and compared in constant time.
- Unauthenticated routes: `GET /health` (`{"ok":true,"auth":"local"}` — the
  engine's and client's reachability probes send no bearer; also the app's
  health check) and `GET /releases/latest.txt` (the running version, so the
  engine's updater reads "up to date"). Everything else is `401` without the
  token.
- Single tenant: the engine runs as user/org `local` (store under
  `$ZERON_DATA_DIR/orgs/local/local/`, independent of the token, so rotating
  it keeps all data); every `/registry/{org}/…` path is the one registry room.
  `/auth/*` answers `501` (no WorkOS); APNs `push-target` is accepted and
  dropped.
- The engine injects `ZERON_IPC_TOKEN` explicitly into the `zeron mcp` server
  it gives agents (harnesses filter MCP server environments).
- The engine's registry device row carries `ZERON_DEVICE_PLATFORM`. Viewers
  count a device as an execution host when its row advertises engine
  capabilities (every engine writes them at boot), falling back to the
  platform only for capability-less rows — so the phone's engine is a host and
  viewer rows stay viewers.

### Client side

`CoreClient(CoreConfig{ edge_url: "http://127.0.0.1:27655", platform: "android", … },
Credentials.Local{ token })`. Credentials::Local is a new variant: the bearer is
the token verbatim, identity is the fixed local user/org (`local` / `local`).

Remote-control mode is the unmodified WorkOS flow against the production edge
(`zeron://` callback scheme). The app keeps one `CoreClient` per mode and lets
the user switch; a later milestone lets the phone's engine sign into the
account so it appears as a device beside the desktops.

### Runtime API (`:runtime` module → `:app`)

```kotlin
package sh.zeron.runtime

object ZeronRuntime { fun get(context: Context): RuntimeController }

interface RuntimeController {
    val state: StateFlow<RuntimeState>
    val isSupportedAbi: Boolean
    fun start()                 // bootstrap if needed, then run RuntimeService
    fun stop()
    suspend fun reset()         // wipe the guest (keeps nothing)
    fun logTail(lines: Int = 200): String
    suspend fun exec(command: String, asRoot: Boolean = false,
                     timeoutMs: Long = 600_000): ExecResult   // one-off `sh -lc` in the guest
}

sealed interface RuntimeState {
    data object NotInstalled : RuntimeState
    data class Bootstrapping(val step: String, val progress: Float?) : RuntimeState
    data object Starting : RuntimeState
    data class Running(val edgeUrl: String, val edgeToken: String,
                       val ipcPort: Int, val ipcToken: String,
                       val deviceName: String) : RuntimeState
    data object Stopped : RuntimeState
    data class Failed(val reason: String, val logTail: String) : RuntimeState
}

data class ExecResult(val exitCode: Int, val output: String)
```

## Android platform constraints

- **Foreground service** (`foregroundServiceType="specialUse"`) keeps the guest
  alive; its notification shows engine state and working agents.
- **Phantom process killer** (Android 12+) caps child processes across apps.
  The engine runs as one proot tree; the app detects kills (engine exit with
  SIGKILL while foregrounded) and surfaces the developer-options switch.
- **Battery**: `REQUEST_IGNORE_BATTERY_OPTIMIZATIONS` is requested only when
  the user starts the engine.
- **Distribution**: proot + downloaded rootfs has Play precedent (UserLAnd);
  sideload/F-Droid builds are the fallback.
- **Licences**: proot is GPL-2.0 and talloc LGPL-3.0, shipped as separate
  executables; listed in `THIRD_PARTY_NOTICES.md` with source links.

## Milestones

1. **Engine side** — local edge crate (`crates/localedge`), `zeron headless`
   embedding, IPC token, platform override, `Credentials::Local`; an e2e test
   drives engine (mock harness) ⇄ local edge ⇄ `zeron-client`.
2. **Runtime** — `apps/android` Gradle project, proot/rootfs packaging,
   bootstrap, `RuntimeService`, health/logs; verified on the emulator:
   engine up, `/health` ok, a harness installs and `--version` runs.
3. **UI** — the Material 3 Expressive Compose app over `zeron-mobile`
   (see § UI).
4. **Integration** — on-device mode end-to-end.

## UI

One app, three modes, chosen on the first-run screen and switchable in
Settings → Where agents run (`AppModel.chooseMode`):

| Mode | Client | Data dir |
| --- | --- | --- |
| Your computers (account) | `Credentials.WorkOs`, production edge | `filesDir/core` |
| This phone | `Credentials.Local(Running.edgeToken)`, `Running.edgeUrl` | `filesDir/phone` |
| Demo | `Credentials.Demo` (Rust `DemoHost`) | `filesDir/demo` |

Phone mode (`core/PhoneEngine.kt`, `ui/EngineScreens.kt`):
- A client exists only while the runtime is `Running`; otherwise the setup
  screen shows the state (bootstrap step + progress, failure reason + log
  tail + the child-process hint) with Set up / Start / Try again / Reset.
- Set up / Start asks for the notification permission, then the battery
  exemption (`RuntimePermissions`). Launch restarts the engine unless the
  user stopped it (the `phoneAutostart` setting).
- Settings → On-device engine: state, Stop/Start, Reset, battery and
  notification status, live log.
- New sessions default to a harness the device has installed; the project
  sheet adds "Clone repository" (`git clone` through `RuntimeController.exec`
  into `/home/zeron/projects`, then `createProject`) and "Empty project".
- `core/Notifier.kt` posts local notifications (finished / needs input /
  failed) while the app is in the background; tapping one opens the session.

Settings → Coding agents (`ui/AgentsScreen.kt`, any engine device, so
account mode manages remote hosts too) speaks `host_call`: `ListHarnesses`,
`InstallHarness` / `CancelInstall` (a relay timeout falls back to polling the
catalog), `CheckHarnessUpdates` on open and pull-to-refresh, per-agent
`ApplyHarnessUpdate` and the header's Update all (`ApplyAllHarnessUpdates`,
progress by polling `ListHarnessUpdates`), `UninstallHarness` (the confirmation
dialog shows the engine's `dryRun` list; accounts stay), `ListAgentAccounts` (plan label),
`StartAgentLogin` → Custom Tab → `PollAgentLogin`, or paste-code
`CompleteAgentLogin`, and `ForgetAgentAccount`. Reply parsing is in
`core/Agents.kt` (JVM unit tests: `./gradlew :app:testDebugUnitTest`).

### Status (2026-09-29)

- The engine, runtime and the earlier UI ran on a real arm64 phone: Claude
  Code (Max) and Codex signed in and ran sessions on-device.
- The combined app is verified on an Android 16 x86_64 emulator: clean
  install → "Run agents on this phone" → notification + battery prompts →
  guest set up in ~10 s → Coding agents → OpenCode installed → a cloned
  repository → a session on OpenCode Zen "Big Pickle" wrote and ran
  `hello.py` and streamed the reply; relaunch autostarts the engine; Stop,
  Reset and re-setup; Demo (sessions, agent sign-in) and the account sign-in
  screen.

Not yet verified:
- **The combined app on arm64 hardware**, and account-mode WorkOS sign-in and
  agent browser sign-in end to end: the emulator's `system_server` aborts on a
  GPU assertion whenever the app backgrounds (a Custom Tab opening), so
  background notifications are unverified too.
- **A real phantom-process kill.** Detection was exercised with a simulated
  SIGKILL only.
- **Release builds.** Per-ABI APK splits, and Play's 16 KB alignment for
  `libproot-loader32.so`, are both open.
- **Capsules.** Moving work between machines is the next design, not in v1.

Emulator note: boot with `-feature -ReadColorBufferDma -feature -GLDMA2`
(swiftshader); otherwise `system_server` aborts on
`hasReadColorBufferDma`. `adb exec-out screencap` still hits the assertion in
its own process; use `adb emu screenrecord screenshot <file>`.

## Build

```
cd apps/android && ./gradlew :app:assembleDebug
```

`:app`'s `preBuild` runs, when their outputs are missing,
`scripts/android/fetch-proot.sh`, `fetch-rootfs.sh` and `build-engine.sh`
(→ `target/android-runtime/{jniLibs,assets}`) and always
`build-core.sh` (→ `target/android-core`). Rerun a script by hand to refresh
its output; `-PzeronSkipRuntime` / `-PzeronSkipCore` skip them. `:app`
packages both directories, `useLegacyPackaging = true` (the runtime's
executables must be extracted to `nativeLibraryDir`), and keeps `libzeron.so`,
`libproot*.so`, `libtalloc.so` and `libandroid-shmem.so` unstripped — the
strip pass corrupts the patchelf'd libs.

Toolchain: JDK 21, Android SDK platform 37 + NDK 29.0.14206865, rustup
targets `aarch64-linux-android x86_64-linux-android
aarch64-unknown-linux-musl x86_64-unknown-linux-musl`, `cargo-ndk`,
`cargo-zigbuild` + zig, `patchelf`.
