#!/bin/sh
# Packs the model for the keyboard: the key head, then the encoder in ONNX
# Runtime's format, from the training tree on the owner's Mac.
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
model="$root/data/raw/neural-spike/model"
out="$root/android/app/src/main/assets/neural.bin"

(printf SYNEURO1; cat "$model/head.bin" "$model/encoder.int8.ort") > "$out"
wc -c "$out"
