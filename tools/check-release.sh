#!/bin/sh
# Fails unless the release keeps its promises: no permission, not debuggable, no
# backups, the expected components only, memory tagging, a hardened library, its
# own signing key, and nothing unwanted or known vulnerable in the dependencies.
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
apk="$root/android/app/build/outputs/apk/release/app-release.apk"
sdk="$HOME/Library/Android/sdk"
tools=$(ls -d "$sdk"/build-tools/* | tail -1)
readelf=$(ls "$sdk"/ndk/*/toolchains/llvm/prebuilt/*/bin/llvm-readelf | tail -1)
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT
fail() { echo "FAIL: $1"; exit 1; }

permissions=$("$tools/aapt2" dump permissions "$apk" | grep -c "uses-permission" || true)
[ "$permissions" = 0 ] || fail "the APK declares permissions"

"$tools/aapt2" dump xmltree "$apk" --file AndroidManifest.xml > "$scratch/manifest"
! grep -q "debuggable" "$scratch/manifest" || fail "the release is debuggable"
! grep -q "directBootAware" "$scratch/manifest" || fail "the keyboard runs before the first unlock"
grep -q 'allowBackup.*=0\|allowBackup.*false' "$scratch/manifest" || fail "backups are allowed"
grep -q 'memtagMode' "$scratch/manifest" || fail "memory tagging is off"

exported=$(awk '/E: (activity|service|provider|receiver)/ { name = "" }
    /android:name\(/ { if (match($0, /="[^"]*"/)) name = substr($0, RSTART + 2, RLENGTH - 3) }
    /android:exported\(.*=(true|0xffffffff)/ { print name }' "$scratch/manifest" | sort | tr '\n' ' ')
[ "$exported" = "app.synthboard.SettingsActivity app.synthboard.SynthboardService " ] ||
    fail "unexpected exported components: $exported"

"$tools/apksigner" verify --print-certs "$apk" > "$scratch/signer"
! grep -q "Android Debug" "$scratch/signer" || fail "signed with the debug key"

unzip -q -o "$apk" 'lib/arm64-v8a/*' -d "$scratch"
library="$scratch/lib/arm64-v8a/libsynthboard_ffi.so"
"$readelf" -lW "$library" > "$scratch/segments"
"$readelf" -dW "$library" > "$scratch/dynamic"
grep -q GNU_RELRO "$scratch/segments" || fail "the library has no RELRO"
grep -q BIND_NOW "$scratch/dynamic" || fail "the library binds lazily"
! grep GNU_STACK "$scratch/segments" | grep -q ' RWE ' || fail "the library's stack is executable"
! strings "$library" | grep -q "$HOME" || fail "the library names this machine's paths"

cd "$root"
elsewhere=$(grep -rl "unsafe " crates --include='*.rs' | grep -v 'crates/synthboard-ffi/src/jni.rs' || true)
[ -z "$elsewhere" ] || fail "unsafe code outside the JNI door: $elsewhere"
cargo deny check
cargo vet --locked
cargo audit -q
echo "Release checks passed."
