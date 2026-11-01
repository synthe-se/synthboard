#!/bin/sh
# Signs the release with the key on the encrypted volume, which must be open.
# Pass "new-key" once to make the key and the lineage from the old debug key, so
# that the installed keyboard accepts the update and keeps its data.
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
signing=/Volumes/SynthboardVault/signing
store="$signing/synthboard.jks"
pass="$signing/password"
lineage="$signing/lineage"
tools=$(ls -d "$HOME"/Library/Android/sdk/build-tools/* | tail -1)
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

"$tools/apksigner" sign --ks "$store" --ks-pass "file:$pass" --ks-key-alias synthboard \
    --lineage "$lineage" --v1-signing-enabled false --v2-signing-enabled false --out "$outputs/app-release.apk" "$outputs/app-release-unsigned.apk"
"$tools/apksigner" verify "$outputs/app-release.apk"
