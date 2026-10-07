#!/usr/bin/env bash
# Build libtabular.so (arm64-v8a) untuk Android lewat cargo-ndk.
# Prasyarat: rustup target aarch64-linux-android, cargo-ndk, Android NDK r28.
# Lihat android/README.md untuk langkah integrasi ke proyek Gradle.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

# Pilih NDK: hormati ANDROID_NDK_HOME bila sudah diset, kalau tidak cari r28 di SDK default.
if [[ -z "${ANDROID_NDK_HOME:-}" ]]; then
    SDK="${ANDROID_SDK_ROOT:-${ANDROID_HOME:-$HOME/Library/Android/sdk}}"
    CANDIDATE="$(ls -d "$SDK"/ndk/28.* 2>/dev/null | sort -V | tail -n1 || true)"
    if [[ -z "$CANDIDATE" ]]; then
        echo "error: ANDROID_NDK_HOME is not set and no NDK 28.x found under $SDK/ndk" >&2
        exit 1
    fi
    export ANDROID_NDK_HOME="$CANDIDATE"
fi
export ANDROID_NDK_ROOT="$ANDROID_NDK_HOME"

command -v cargo-ndk >/dev/null 2>&1 || { echo "error: cargo-ndk not found (cargo install cargo-ndk)" >&2; exit 1; }
rustup target list --installed | grep -q '^aarch64-linux-android$' \
    || { echo "error: run 'rustup target add aarch64-linux-android' first" >&2; exit 1; }

TARGET_ABI="${TARGET_ABI:-arm64-v8a}"
OUT_DIR="${OUT_DIR:-android/app/src/main/jniLibs}"

echo "==> NDK: $ANDROID_NDK_HOME"
echo "==> cargo ndk -t $TARGET_ABI -o $OUT_DIR build --release"
cargo ndk -t "$TARGET_ABI" -o "$OUT_DIR" build --release "$@"

cat <<MSG

Done. Shared library written to: $OUT_DIR/$TARGET_ABI/libtabular.so

Next steps:
  1. Create (or open) an Android Gradle project under android/ whose module
     'app' contains src/main/jniLibs (this directory) - see android/README.md.
  2. Add the NativeActivity entry to AndroidManifest.xml with
     android.app.lib_name = "tabular" and android:hasCode="false".
  3. ./gradlew assembleDebug, then adb install -r app/build/outputs/apk/debug/app-debug.apk
  4. Logs: adb logcat -s tabular RustStdoutStderr
MSG
