#!/bin/sh
# The encrypted volume holding personal data and the signing key: plain traces,
# what the keyboard learned, the trace key. A build script runs as the owner in
# this tree, so these stay locked away except while a tool needs them.
#   vault.sh create   makes the volume; macOS asks for a new passphrase
#   vault.sh open     unlocks it; macOS asks for the passphrase
#   vault.sh close    locks it
#   vault.sh adopt    moves data/raw/personal onto it, once
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
image="$HOME/Library/Application Support/Synthboard/vault.sparsebundle"
volume=/Volumes/SynthboardVault
link="$root/data/raw/personal"

is_open() { [ -d "$volume" ]; }

case "${1:-}" in
create)
    [ ! -e "$image" ] || { echo "The volume already exists."; exit 1; }
    mkdir -p "$(dirname "$image")"
    hdiutil create -quiet -agentpass -encryption AES-256 -type SPARSEBUNDLE -fs APFS \
        -size 2g -volname SynthboardVault "$image"
    tmutil addexclusion "$image"
    ;;
open)
    is_open || hdiutil attach -quiet -agentpass -nobrowse "$image"
    ;;
close)
    ! is_open || hdiutil detach -quiet "$volume"
    ;;
adopt)
    is_open || { echo "Open the volume first."; exit 1; }
    [ ! -L "$link" ] || { echo "Already moved."; exit 1; }
    ditto "$link" "$volume/personal"
    chmod -R go-rwx "$volume/personal"
    # The originals go only once every byte is on the volume.
    diff -r "$link" "$volume/personal"
    rm -rf "$link"
    ln -s "$volume/personal" "$link"
    ;;
*)
    sed -n 2,8p "$0"
    exit 1
    ;;
esac
