#!/bin/sh
# The daily loop: collects gesture traces, reports how typing went, and replays
# every kept gesture through the current decoder.
#
# Traces arrive two ways, and the same gesture is only kept once:
#   - by Taildrop, from the "Send my traces" button in Synthboard's settings;
#   - by cable, for debug builds only, since a release cannot be read over USB.
# They hold typed words in clear, so they stay in the ignored data folder.
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
personal="$root/data/raw/personal"
archive="$personal/traces.tsv"
inbox="$personal/inbox"
mkdir -p "$inbox"

if command -v tailscale > /dev/null; then
    tailscale file get --conflict=rename "$inbox" > /dev/null 2>&1 || true
fi

received=$(mktemp)
trap 'rm -f "$received"' EXIT
if adb exec-out run-as app.synthboard cat files/traces.tsv > "$received" 2>/dev/null && [ -s "$received" ]; then
    cp "$received" "$inbox/cable-$(date +%Y%m%d-%H%M%S).tsv"
fi

set -- "$inbox"/*.tsv
if [ -e "$1" ]; then
    python3 -I "$root/tools/ingest-traces.py" "$archive" "$@"
    rm -f "$@"
else
    echo "Nothing new. Send your traces from Synthboard's settings."
fi

python3 -I "$root/tools/analyze-traces.py" "$archive"

cd "$root"
cargo build -q --release -p synthswipe-bench
echo
echo "Replay through the current decoder:"
./target/release/synthswipe-bench data/raw --min-weight 2 --personal 2>/dev/null | tail -1
