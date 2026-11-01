#!/bin/sh
# Signs the release with the key on the encrypted volume, which must be open.
# Another signer sets SYNTHBOARD_SIGNING_DIR to a directory holding synthboard.jks and
# its password file, and SYNTHBOARD_SIGNER_SHA256 to that certificate's digest.
# Pass "new-key" once to make the key and the lineage from the old debug key, so
# that the installed keyboard accepts the update and keeps its data.
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
signing=${SYNTHBOARD_SIGNING_DIR:-/Volumes/SynthboardVault/signing}
store="$signing/synthboard.jks"
pass="$signing/password"
lineage="$signing/lineage"
. "$root/tools/android-env.sh"
tools=$(android_build_tools)
outputs="$root/android/app/build/outputs/apk/release"

if [ "${1:-}" = new-key ]; then
    [ ! -e "$store" ] || { echo "The key already exists."; exit 1; }
    mkdir -m 700 -p "$signing"
    (umask 077; head -c 32 /dev/urandom | base64 > "$pass")
    keytool -genkeypair -keyalg RSA -keysize 4096 -validity 10000 -alias synthboard \
        -dname "CN=Synthboard" -keystore "$store" -storetype PKCS12 \
        -storepass:file "$pass" -keypass:file "$pass"
    "$tools/apksigner" rotate --out "$lineage" \
        --old-signer --ks "$HOME/.android/debug.keystore" --ks-pass pass:android \
        --ks-key-alias androiddebugkey --key-pass pass:android --set-rollback false \
        --new-signer --ks "$store" --ks-pass "file:$pass" --ks-key-alias synthboard
fi

# A lineage only matters to a key that replaced an installed one.
set --
[ ! -e "$lineage" ] || set -- --lineage "$lineage"
"$tools/apksigner" sign --ks "$store" --ks-pass "file:$pass" --ks-key-alias synthboard \
    "$@" --v1-signing-enabled false --v2-signing-enabled false --out "$outputs/app-release.apk" "$outputs/app-release-unsigned.apk"
signed_by_release_key "$outputs/app-release.apk" || { echo "Signed, but not with the expected certificate."; exit 1; }
