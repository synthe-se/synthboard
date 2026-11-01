#!/bin/sh
# Fails unless the release keeps its promises: no permission, not debuggable, no
# network code, no plain cloud path, and no known vulnerable dependency.
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
apk="$root/android/app/build/outputs/apk/release/app-release.apk"
aapt=$(ls "$HOME"/Library/Android/sdk/build-tools/*/aapt2 | tail -1)

permissions=$("$aapt" dump permissions "$apk" | grep -c "uses-permission" || true)
[ "$permissions" = 0 ] || { echo "FAIL: the APK declares permissions"; exit 1; }

"$aapt" dump xmltree "$apk" --file AndroidManifest.xml > /tmp/synthboard-manifest.$$
trap 'rm -f /tmp/synthboard-manifest.$$' EXIT
if grep -q "debuggable" /tmp/synthboard-manifest.$$; then
    echo "FAIL: the release is debuggable"
    exit 1
fi
grep -q 'allowBackup.*=0\|allowBackup.*false' /tmp/synthboard-manifest.$$ || { echo "FAIL: backups are allowed"; exit 1; }

cd "$root"
if cargo tree -p synthboard-ffi --edges normal --prefix none | grep -qE '^(reqwest|hyper|ureq|tokio|rustls|openssl|curl|native-tls) '; then
    echo "FAIL: network code in the native library"
    exit 1
fi
cargo audit -q
echo "Release checks passed."
