#!/bin/sh
# Fails unless the release keeps its promises: no permission, not debuggable, no
# backups, the expected components only, memory tagging, a hardened library, its
# own signing key, and nothing unwanted or known vulnerable in the dependencies.
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
outputs="$root/android/app/build/outputs/apk/release"
apk="$outputs/app-release.apk"
unsigned="$outputs/app-release-unsigned.apk"
hashes="$root/tools/release-hashes.txt"
sdk="$HOME/Library/Android/sdk"
tools=$(ls -d "$sdk"/build-tools/* | tail -1)
readelf=$(ls "$sdk"/ndk/*/toolchains/llvm/prebuilt/*/bin/llvm-readelf | tail -1)
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT
fail() { echo "FAIL: $1"; exit 1; }
digest() { shasum -a 256 | cut -d ' ' -f 1; }
expected() { awk -v name="$1" '$2 == name { print $1 }' "$hashes"; }
# The functions that call a symbol through the PLT, sorted and named.
callers() { awk -v target="<$1@plt>" '/^[0-9a-f]+ <.*>:$/ { name = substr($2, 2, length($2) - 3) }
    index($0, target) && !/>:$/ { print name }' "$scratch/code" | sort -u; }

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

"$tools/apksigner" verify --print-certs "$apk" > "$scratch/signer"
grep -q "SHA-256 digest: 8f200e94eb4d58efd42eff171f440101203ab0ed4e1462410cfb7bef109fc7fc" "$scratch/signer" || fail "not signed with our key"

unzip -q -o "$apk" 'lib/arm64-v8a/*' -d "$scratch"
library="$scratch/lib/arm64-v8a/libsynthboard_ffi.so"
"$readelf" -lW "$library" > "$scratch/segments"
"$readelf" -dW "$library" > "$scratch/dynamic"
grep -q GNU_RELRO "$scratch/segments" || fail "the library has no RELRO"
grep -q BIND_NOW "$scratch/dynamic" || fail "the library binds lazily"
! grep GNU_STACK "$scratch/segments" | grep -q ' RWE ' || fail "the library's stack is executable"
! strings "$library" | grep -qE '/Users/|/private/|claude-' || fail "the library names this machine's paths"

# `wipe.rs` only sees ONNX Runtime's blocks if every `free` in the library goes through it.
bin=$(dirname "$readelf")
"$bin/llvm-nm" -D --defined-only "$library" | grep -q ' T __wrap_free$' || fail "free is not wrapped"
"$bin/llvm-objdump" -d --no-show-raw-insn "$library" > "$scratch/code"
[ "$(callers free)" = "__wrap_free" ] || fail "something frees without the wipe: $(callers free)"
# `realloc` frees a moved block unwiped, so it stays where no text or gesture goes.
unknown=$(callers realloc | grep -vE '^__emutls_get_address$|__double_or_nothing|money_get' || true)
[ -z "$unknown" ] || fail "realloc has a new caller: $unknown"

# Tracked, so that a change shows in a diff and the clean tree below covers it. These
# are public facts. The archive is linked rather than packed, so its file is compared.
for asset in assets/neural.bin assets/trace.pub; do
    [ "$(unzip -p "$apk" "$asset" | digest)" = "$(expected "$asset")" ] || fail "$asset is not the expected file"
done
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
[ "$(stat -f %m "$unsigned")" -ge "$(git log -1 --format=%ct)" ] || fail "the APK is older than HEAD"
contents() { unzip -v "$1" | awk '$8 ~ /\// && $8 !~ /^META-INF/ { print $7, $8 }' | sort; }
[ "$(contents "$apk")" = "$(contents "$unsigned")" ] || fail "the signed APK is not this build"
echo "Release checks passed."
