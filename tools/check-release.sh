#!/bin/sh
# Fails unless the release keeps its promises: no permission, not debuggable, no
# backups, the expected components only, memory tagging, a hardened library, its
# own signing key, and nothing unwanted or known vulnerable in the dependencies.
# Pass "public" for a build without the owner's trace key: it must then hold none.
# SYNTHBOARD_SIGNER_SHA256 replaces the owner's certificate digest for another signer.
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
outputs="$root/android/app/build/outputs/apk/release"
apk="$outputs/app-release.apk"
unsigned="$outputs/app-release-unsigned.apk"
hashes="$root/tools/release-hashes.txt"
. "$root/tools/android-env.sh"
tools=$(android_build_tools)
readelf=$(android_ndk_bin)/llvm-readelf
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT
fail() { echo "FAIL: $1"; exit 1; }
digest() { shasum -a 256 | cut -d ' ' -f 1; }
# GNU `stat -f` means something else, so its `-c` is tried first and BSD's form is the fallback.
modified() { stat -c %Y "$1" 2>/dev/null || stat -f %m "$1"; }
expected() { awk -v name="$1" '$2 == name { print $1 }' "$hashes"; }
# The packed library has no local names, so calls are counted rather than named.
calls() { grep -v ">:$" "$scratch/code" | grep -c "<$1@plt>"; }

permissions=$("$tools/aapt2" dump permissions "$apk" | grep -c "uses-permission" || true)
[ "$permissions" = 0 ] || fail "the APK declares permissions"

"$tools/aapt2" dump xmltree "$apk" --file AndroidManifest.xml > "$scratch/manifest"
! grep -q "debuggable" "$scratch/manifest" || fail "the release is debuggable"
! grep -q "directBootAware" "$scratch/manifest" || fail "the keyboard runs before the first unlock"
grep -q 'allowBackup.*=0\|allowBackup.*false' "$scratch/manifest" || fail "backups are allowed"
grep -q 'memtagMode.*=2' "$scratch/manifest" || fail "memory tagging is not synchronous"
grep -q 'usesCleartextTraffic.*=0\|usesCleartextTraffic.*false' "$scratch/manifest" || fail "cleartext traffic is allowed"
grep -q 'nativeHeapZeroInitialized.*=-1\|nativeHeapZeroInitialized.*true\|nativeHeapZeroInitialized.*0xffffffff' "$scratch/manifest" || fail "the native heap is not zeroed"
grep -q 'dataExtractionRules' "$scratch/manifest" || fail "no rule against data extraction"

exported=$(awk '/E: (activity|service|provider|receiver)/ { name = "" }
    /android:name\(/ { if (match($0, /="[^"]*"/)) name = substr($0, RSTART + 2, RLENGTH - 3) }
    /android:exported\(.*=(true|0xffffffff)/ { print name }' "$scratch/manifest" | sort | tr '\n' ' ')
[ "$exported" = "app.synthboard.SettingsActivity app.synthboard.SynthboardService " ] ||
    fail "unexpected exported components: $exported"

signed_by_release_key "$apk" || fail "not signed with our key"

unzip -q -o "$apk" 'lib/arm64-v8a/*' -d "$scratch"
library="$scratch/lib/arm64-v8a/libsynthboard_ffi.so"
"$readelf" -lW "$library" > "$scratch/segments"
"$readelf" -dW "$library" > "$scratch/dynamic"
grep -q GNU_RELRO "$scratch/segments" || fail "the library has no RELRO"
grep -q BIND_NOW "$scratch/dynamic" || fail "the library binds lazily"
# A strong symbol nothing provides stops the library loading, and the keyboard with it.
[ -z "$("$(dirname "$readelf")/llvm-nm" -D --undefined-only "$library" | grep -E "^ +U " | grep -v "@" | grep -v " __android_log_")" ] ||
    fail "the library needs symbols nothing provides"
"$readelf" -n "$library" | grep -q "aarch64 feature: BTI" || fail "the library has no branch targets"
"$(dirname "$readelf")/llvm-objdump" -d "$library" | grep -q paciasp || fail "the library signs no return address"
! grep GNU_STACK "$scratch/segments" | grep -q ' RWE ' || fail "the library's stack is executable"
! strings "$library" | grep -qE '/Users/|/home/|/private/|claude-' || fail "the library names this machine's paths"

# `wipe.rs` only sees ONNX Runtime's blocks if every `free` in the library goes through it.
bin=$(dirname "$readelf")
"$bin/llvm-nm" -D --defined-only "$library" | grep -q ' T __wrap_free$' || fail "free is not wrapped"
"$bin/llvm-objdump" -d --no-show-raw-insn "$library" > "$scratch/code"
[ "$(calls free)" = 1 ] || fail "something frees without the wipe"
# `realloc` frees a moved block unwiped, so it stays where no text or gesture goes.
# Seven: `__emutls_get_address`, and six in libc++ `money_get` and `__double_or_nothing`.
[ "$(calls realloc)" = 7 ] || fail "realloc has new callers: $(calls realloc)"

# Tracked, so that a change shows in a diff and the clean tree below covers it. These
# are public facts. The archive is linked rather than packed, so its file is compared.
check_asset() {
    [ "$(unzip -p "$apk" "$1" | digest)" = "$(expected "$1")" ] || fail "$1 is not the expected file"
}
check_asset assets/neural.bin
trace_key=assets/trace.pub
if [ "${1:-}" = public ]; then
    # Without this key the engine traces nothing, so its absence is what a public build promises.
    ! unzip -Z1 "$apk" | grep -qx "$trace_key" || fail "a public build carries $trace_key"
else
    check_asset "$trace_key"
fi
[ "$(digest < "$root/data/raw/ort/android/libonnxruntime.a")" = "$(expected libonnxruntime.a)" ] ||
    fail "libonnxruntime.a is not the expected file"

cd "$root"
elsewhere=$(grep -rlE '\bunsafe\b' crates --include='*.rs' | grep -vE 'crates/synthboard-ffi/src/(jni|wipe|lib)\.rs' || true)
[ -z "$elsewhere" ] || fail "unsafe code outside the two files allowed: $elsewhere"
! grep -rqE 'std::net|java\.net' crates android/app/src || fail "network code"
! "$bin/llvm-nm" -D --undefined-only "$library" | grep -qE " (socket|connect|sendto|recvfrom|getaddrinfo)(@|$)" || fail "the library imports the network"
[ -f android/gradle/verification-metadata.xml ] || fail "Gradle dependencies are not verified"
grep -q distributionSha256Sum android/gradle/wrapper/gradle-wrapper.properties || fail "the Gradle distribution is not pinned"
cargo deny check
cargo vet --locked
cargo audit -q
[ -z "$(git status --porcelain)" ] || fail "the tree has uncommitted changes"
# Signing touches the signed file at any later time, so the build is judged on what it wrote.
[ "$(modified "$unsigned")" -ge "$(git log -1 --format=%ct)" ] || fail "the APK is older than HEAD"
contents() { unzip -v "$1" | awk '$8 ~ /\// && $8 !~ /^META-INF/ { print $7, $8 }' | sort; }
[ "$(contents "$apk")" = "$(contents "$unsigned")" ] || fail "the signed APK is not this build"
echo "Release checks passed."
