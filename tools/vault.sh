#!/bin/sh
# The encrypted volume holding personal data and the signing key: plain traces,
# what the keyboard learned, the trace key. A build script runs as the owner in
# this tree, so these stay locked away except while a tool needs them.
#   vault.sh create   makes the volume, asking for a new passphrase
#   vault.sh open     unlocks it, asking for the passphrase
#   vault.sh close    locks it
#   vault.sh adopt    moves data/raw/personal onto it, once
set -eu

# hdiutil, tmutil, ditto and the passphrase dialog exist on macOS only.
[ "$(uname -s)" = Darwin ] || { echo "vault.sh only works on macOS: it drives hdiutil and an encrypted sparse bundle."; exit 1; }

root=$(cd "$(dirname "$0")/.." && pwd)
image="$HOME/Library/Application Support/Synthboard/vault.sparsebundle"
volume=/Volumes/SynthboardVault
link="$root/data/raw/personal"

is_open() { [ -d "$volume" ]; }

# macOS asks in a masked dialog, and the answer goes straight to hdiutil.
ask() {
    osascript -e "text returned of (display dialog \"$1\" default answer \"\" with hidden answer with title \"Synthboard\")"
}

case "${1:-}" in
create)
    [ ! -e "$image" ] || { echo "The volume already exists."; exit 1; }
    mkdir -p "$(dirname "$image")"
    first=$(ask "New passphrase for the encrypted volume")
    [ "$first" = "$(ask "The same passphrase again")" ] || { echo "The two entries differ."; exit 1; }
    printf %s "$first" | hdiutil create -quiet -stdinpass -encryption AES-256 -type SPARSEBUNDLE -fs APFS \
        -size 2g -volname SynthboardVault "$image"
    tmutil addexclusion "$image"
    ;;
open)
    is_open || ask "Passphrase of the encrypted volume" | tr -d '\n' | hdiutil attach -quiet -stdinpass -nobrowse "$image"
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
