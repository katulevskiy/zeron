#!/bin/bash
# Build the Rust mobile core (crates/mobile) for Android and generate its
# Kotlin bindings — the same library the iOS app links.
#
#   scripts/android/build-core.sh [abi…]        # default: arm64-v8a x86_64
#
# Outputs:
#   target/android-core/jniLibs/<abi>/libzeron_mobile.so   (:app jniLibs srcDir)
#   apps/android/app/src/main/java/uniffi/zeron_core/zeron_core.kt
#     — committed (like apps/ios/Zeron/Core/Generated), so the Gradle build
#       never needs cargo; only rewritten when it changed.
#
# Needs the Android NDK (ANDROID_NDK_HOME) + cargo-ndk
# (`cargo install cargo-ndk`, `rustup target add aarch64-linux-android
# x86_64-linux-android`). CARGO_TARGET_DIR is honoured (build dirs are shared
# by several engineers; point it somewhere private to avoid lock contention).
# ZERON_SKIP_CORE=1 reuses the last built libraries (Kotlin-only iteration).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
OUT="$ROOT/target/android-core"
TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
PROFILE="${ZERON_CORE_PROFILE:-mobile}"
KOTLIN_OUT="$ROOT/apps/android/app/src/main/java"
ABIS=("$@")
[[ ${#ABIS[@]} -eq 0 ]] && ABIS=(arm64-v8a x86_64)

if [[ "${ZERON_SKIP_CORE:-}" == "1" && -f "$OUT/jniLibs/${ABIS[0]}/libzeron_mobile.so" ]]; then
  echo "note: ZERON_SKIP_CORE=1 — reusing $OUT/jniLibs"
  exit 0
fi

if ! command -v cargo-ndk >/dev/null || [[ -z "${ANDROID_NDK_HOME:-}" ]]; then
  echo "error: cargo-ndk and ANDROID_NDK_HOME are required" >&2
  exit 1
fi

cd "$ROOT"
export CARGO_TARGET_DIR="$TARGET_DIR"
mkdir -p "$OUT/jniLibs"

ndk_args=()
for abi in "${ABIS[@]}"; do ndk_args+=(-t "$abi"); done
# API 29 = the app's minSdk.
cargo ndk "${ndk_args[@]}" -P 29 -o "$OUT/jniLibs" \
  build --locked -p zeron-mobile --lib --profile "$PROFILE"

# The `mobile` profile keeps line tables (symbolized crashes from the target
# dir's copy); the packaged copies drop them — 160 MB → ~30 MB per ABI.
# ZERON_CORE_STRIP=0 keeps them (native debugging).
if [[ "${ZERON_CORE_STRIP:-1}" == "1" ]]; then
  STRIP="$(ls "$ANDROID_NDK_HOME"/toolchains/llvm/prebuilt/*/bin/llvm-strip | head -1)"
  for abi in "${ABIS[@]}"; do "$STRIP" --strip-debug "$OUT/jniLibs/$abi/libzeron_mobile.so"; done
fi

cargo build --locked -p zeron-mobile --bin uniffi-bindgen --features bindgen --profile mobile

# Bindings come from the metadata embedded in the Android library itself, so
# they always match what ships.
case "${ABIS[0]}" in
  arm64-v8a) TRIPLE=aarch64-linux-android ;;
  x86_64) TRIPLE=x86_64-linux-android ;;
  *) echo "error: unsupported abi ${ABIS[0]}" >&2; exit 1 ;;
esac
LIB="$TARGET_DIR/$TRIPLE/$PROFILE/libzeron_mobile.so"
GEN="$OUT/kotlin"
rm -rf "$GEN"
mkdir -p "$GEN"
"$TARGET_DIR/mobile/uniffi-bindgen" generate --library "$LIB" --language kotlin \
  --no-format --out-dir "$GEN" >/dev/null

rel="uniffi/zeron_core/zeron_core.kt"
mkdir -p "$(dirname "$KOTLIN_OUT/$rel")"
cmp -s "$GEN/$rel" "$KOTLIN_OUT/$rel" || cp "$GEN/$rel" "$KOTLIN_OUT/$rel"
echo "jniLibs: $OUT/jniLibs"
echo "kotlin:  $KOTLIN_OUT/$rel"
