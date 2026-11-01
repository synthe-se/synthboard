#!/bin/sh
# The daily loop: opens the sealed traces sent from Synthboard's settings over
# Taildrop, reports how typing went, and replays every gesture through the
# current decoder. Plain traces exist only on the encrypted volume, which is
# open for the length of this script.
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
personal="$root/data/raw/personal"
archive="$personal/traces.tsv"
inbox="$personal/inbox"
cd "$root"
# Build scripts run as the owner, so nothing is built with the volume open.
cargo build -q --release -p synthswipe-bench
"$root/tools/vault.sh" open
trap '"$root/tools/vault.sh" close' EXIT
mkdir -p "$inbox"

tailscale file get --conflict=rename "$inbox" > /dev/null 2>&1 || true

set -- "$inbox"/*.sealed*
if [ -e "$1" ]; then
    opened="$personal/opened.tmp"
    for file in "$@"; do
        ./target/release/trace_keys open "$file" "$opened" "$personal"/trace*.key
    done
    ./target/release/traces ingest "$archive" "$opened"
    rm -f "$@" "$opened"
else
    echo "Nothing new. Send your traces from Synthboard's settings."
fi

./target/release/traces report "$archive" --show-words

echo
echo "Replay through the current decoder:"
./target/release/synthswipe-bench data/raw --min-weight 2 --personal 2>/dev/null | tail -1
