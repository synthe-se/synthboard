#!/bin/sh
# The daily loop: opens the sealed traces sent from Synthboard's settings over
# Taildrop, reports how typing went, and replays every gesture through the
# current decoder. Plain traces exist only here, in the ignored data folder.
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
personal="$root/data/raw/personal"
archive="$personal/traces.tsv"
inbox="$personal/inbox"
mkdir -p "$inbox"
cd "$root"
cargo build -q --release -p synthswipe-bench

tailscale file get --conflict=rename "$inbox" > /dev/null 2>&1 || true

set -- "$inbox"/*.sealed*
if [ -e "$1" ]; then
    opened=$(mktemp)
    trap 'rm -f "$opened"' EXIT
    for file in "$@"; do
        ./target/release/trace_keys open "$personal/trace.key" "$file" "$opened"
    done
    python3 -I "$root/tools/ingest-traces.py" "$archive" "$opened"
    rm -f "$@"
else
    echo "Nothing new. Send your traces from Synthboard's settings."
fi

python3 -I "$root/tools/analyze-traces.py" "$archive"

echo
echo "Replay through the current decoder:"
./target/release/synthswipe-bench data/raw --min-weight 2 --personal 2>/dev/null | tail -1
