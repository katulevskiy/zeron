# Zeron for Android

A Jetpack Compose viewport onto the zeron mesh, built on the same Rust mobile
core as the iOS app (`crates/mobile`). **Rust decides what to paint and
where; Kotlin paints, scrolls and handles gestures.** The transcript's text
measurement, markdown layout, prefix-sum virtualization and display lists are
the exact code iOS runs — see [`docs/mobile-rewrite.md`](../../docs/mobile-rewrite.md).

It also runs coding agents **on the phone**: the `:runtime` module boots a
proot Linux guest with the Zeron engine and a local edge, and the app talks
to it like any other engine — see [`docs/android.md`](../../docs/android.md).
Three modes, switchable in Settings: your computers (Zeron account), this
phone, and an offline demo.

The UI is Material 3 Expressive (`MaterialExpressiveTheme`, expressive motion,
flexible app bars, shape-morphing loading indicators, connected button groups,
segmented lists) themed with Zeron's own palette and Geist type.

## Build & run

Requires JDK 21, the Android SDK (platform 37, NDK 29.0.14206865), and Rust
with `cargo install cargo-ndk cargo-zigbuild`, zig, `patchelf`, and
`rustup target add aarch64-linux-android x86_64-linux-android
aarch64-unknown-linux-musl x86_64-unknown-linux-musl`.

```sh
cd apps/android
./gradlew :app:installDebug
```

The `buildCore` task runs `scripts/android/build-core.sh`, which builds
`crates/mobile` for Android (`jniLibs`) and generates its Kotlin bindings into
`target/android-core/`. `-PzeronSkipCore` reuses the last build while
iterating on Kotlin; `ZERON_ANDROID_ABIS=arm64-v8a` builds one ABI.
The on-device engine's payload — proot and its libs, the static musl engine,
the Alpine rootfs — comes from `scripts/android/fetch-proot.sh`,
`build-engine.sh` and `fetch-rootfs.sh` (into `target/android-runtime/`),
which the build runs only while their outputs are missing
(`-PzeronSkipRuntime` never runs them). `:app` packages them with
`useLegacyPackaging = true` and unstripped; see docs/android.md § Build.
`./gradlew :app:testDebugUnitTest` runs the JVM tests.
`scripts/android/gen-icons.sh` rasterizes the shared tool/file SVG icons
(needs `rsvg-convert`). The Geist fonts are read straight from the iOS app's
`Fonts/` folder, so both platforms measure and draw the same bytes.

## Layout

```
core/        AppModel (owns the mode's CoreClient, republishes snapshots as
             flows), CredentialStore, Fonts + AndroidMeasurer (Minikin
             fallback measurement for glyphs Geist lacks), PhoneEngine (the
             :runtime engine as the UI sees it), Agents (harness install and
             agent sign-in over host_call), Notifier (local session and
             file-transfer notifications in phone mode), Transfers +
             TransferCenter (device file transfer: polling, Downloads copies,
             the share outbox)
design/      ZeronTheme (Material 3 Expressive), transcript palette
transcript/  TranscriptState (layout engine + viewport: anchoring, follow the
             tail), Transcript (virtualized rows over LayoutFrame), RowModel
             (canvas painter for Rust display lists, streaming veil, fades),
             Widgets (copy, disclosures, tool rail, shimmer, images…)
ui/          Sign-in, sessions, session + composer, new session, search,
             settings, phone setup + on-device engine, coding agents,
             transfers, the Share to Zeron sheet (ShareActivity)
```

`../runtime` (`sh.zeron.runtime`) is the on-device engine: guest bootstrap,
`RuntimeService`, health and logs, behind `ZeronRuntime.get(context)`.

## Launch extras

Mirrors the iOS launch arguments:

```sh
adb shell am start -n sh.zeron.android/.MainActivity \
  --ez demo true --es route chat:chat-veil
```

| Extra | Effect |
| --- | --- |
| `--ez demo true` | Offline demo workspace (Rust `DemoHost`) |
| `--ez fast true` / `--ez longreply true` | Demo stream speed / reply length |
| `--ez big true` / `--ez huge true` | Demo transcripts with 120 / 600 turns |
| `--ez phone true` | Phone mode (the on-device engine) |
| `--es route chat:<id>` / `new` / `search` / `settings` / `engine` / `agents` / `transfers` | Open a screen at launch |
| `--ez signedout true` | Clear stored credentials and the chosen mode |
| `--es wallpaper <path>` / `none` | Set (or clear) the wallpaper from a file the app can read, e.g. `adb push art.jpg /data/local/tmp/ && adb shell run-as sh.zeron.android cp /data/local/tmp/art.jpg files/` then `--es wallpaper /data/user/0/sh.zeron.android/files/art.jpg` |
| `--es wallpaper-effect <none\|dither\|ascii\|halftone\|scanlines>` | Wallpaper effect |
