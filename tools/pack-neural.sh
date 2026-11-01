#!/bin/sh
# Packs the model for the keyboard: the key head, then the encoder in ONNX
# Runtime's format, from the training tree on the owner's Mac.
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
model="$root/data/raw/neural-spike/model"
out="$root/android/app/src/main/assets/neural.bin"

# The key head has one size, which the Rust loader fixes: a stale file must not pass.
eval "$("$root/target/release/shared_numbers")"
[ "$(wc -c < "$model/head.bin" | tr -d ' ')" = "$model_head_bytes" ] || { echo "head.bin is not $model_head_bytes bytes."; exit 1; }
(printf %s "$model_magic"; cat "$model/head.bin" "$model/encoder.int8.ort") > "$out"
wc -c "$out"
