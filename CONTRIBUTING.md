# Contributing

Thank you for looking. Synthboard is a personal project that moves fast, so a short issue before a large change saves everyone time.

[`AGENTS.md`](AGENTS.md) is the rule book, for people and agents alike: read it before changing anything. A security problem goes through the reporting route in [`docs/security.md`](docs/security.md), not a public issue.

## Running the checks

The Rust toolchain installs itself from `rust-toolchain.toml`. You also need [`cargo-deny`](https://github.com/EmbarkStudios/cargo-deny), [`cargo-vet`](https://github.com/mozilla/cargo-vet) and, for fuzzing, [`cargo-fuzz`](https://github.com/rust-fuzz/cargo-fuzz) with a nightly toolchain.

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo deny check
cargo vet --locked
cd fuzz && cargo +nightly fuzz run fuzz_engine -- -max_total_time=120
```

These are what CI runs, and none of them needs private data. On macOS the fuzz run also needs the `RUSTFLAGS` and `--sanitizer none` shown in `AGENTS.md`.

`cargo vet` audits are certified by the owner by hand. Do not add entries to `supply-chain/audits.toml`.

## Signing off

Every commit carries a [Developer Certificate of Origin](https://developercertificate.org/) sign-off: your statement that you may contribute the change under the project's licence. `git commit -s` adds the line. Use your real name.

## What a stranger can build today

From a clean clone, with only Rust: every crate, the tests, the lints and the fuzzers. The app is not a one-line build yet.

For the Android shell and its tests (`./tools/test-shell.sh`) you also need:

- A JDK able to run the Gradle version in `android/gradle/wrapper/gradle-wrapper.properties`.
- The Android SDK and the NDK of `android/gradle.properties`, found through `ANDROID_HOME`.
- `rustup component add rust-src`, because the library rebuilds the standard library.
- `data/raw/ort/android/libonnxruntime.a`, a static ONNX Runtime.
- The assets ignored in `.gitignore`, in `android/app/src/main/assets`.

The native build runs on macOS and Linux. `tools/vault.sh` stays macOS only.

What still cannot be produced from this repository alone:

- **ONNX Runtime.** `tools/build-ort.sh` needs a finished earlier build as its seed tree, for the fetched dependencies, the protobuf outputs and the reduced kernels. Nothing here makes that tree from nothing, and it is not published.
- **The generated assets.** `tools/pack-lexicon.sh`, `tools/pack-grammar.py` and `tools/pack-emoji.py` make them from the datasets in [`PROVENANCE.md`](PROVENANCE.md), which you download yourself under `data/raw`. Some of those licences do not allow publishing the result, as that file says.
- **`neural.bin`.** It is not published. `training/` makes one from public gestures.
- **The owner's release.** The signing key and `trace.pub` live on an encrypted volume and stay there. Personal traces are never published.

To sign and check a build of your own, leave `trace.pub` out, which removes tracing and the Development section, and set your own key:

```sh
export SYNTHBOARD_SIGNING_DIR=/path/holding/synthboard.jks/and/password
export SYNTHBOARD_SIGNER_SHA256=<your certificate's SHA-256 digest, lower case>
./tools/sign-release.sh && ./tools/check-release.sh public
```

`tools/release-hashes.txt` pins the owner's `neural.bin` and `libonnxruntime.a`, so a build with your own files needs your hashes there. `tools/check-release.sh` also needs [`cargo-audit`](https://github.com/rustsec/rustsec/tree/main/cargo-audit) and a clean tree.

## Training the model

`training/` holds two workspaces of their own: `train`, which trains and scores on [Burn](https://burn.dev/), and `quant`, which makes the file the keyboard reads. They read the public datasets of [`PROVENANCE.md`](PROVENANCE.md), which `synthswipe-bench` flattens first. FUTO's swipe datasets alone are enough, and that is the default. HeliBoard's are optional, and share-alike.

**Weights trained on personal traces must never be published.** Personal traces stay out of this pipeline altogether: `neural-train` refuses a path through a `personal` directory, and nothing here reads one.

```sh
cargo run --release -p synthswipe-bench -- data/raw --export data/raw/neural

cd training/train
lexicon=../../android/app/src/main/assets/lexicon.csv
cargo run --release -- train --data ../../data/raw/neural --lexicon $lexicon --out ../../data/raw/runs --name mine --epochs 8
checkpoint=../../data/raw/runs/mine/ema.safetensors
model=../../data/raw/neural-spike/model
mkdir -p $model
cargo run --release -- head $checkpoint $model
cargo run --release -- export $checkpoint $model --data ../../data/raw/neural --lexicon $lexicon

cd ../quant
cargo run --release -- quant ../../data/raw/neural-spike/model encoder.float.onnx encoder.int8.onnx
cargo run --release -- convert ../../data/raw/neural-spike/model encoder.int8.onnx encoder.int8.ort
cd ../.. && ./tools/pack-neural.sh    # after `cargo build --release -p synthswipe-bench`
```

- `--features metal` trains on Apple's GPU. Without it, training runs on the CPU, which is far slower.
- `--sources` lists what to train on, as exported file names or `futo-5-rest`, `futo-5-en` and `heliboard-half`. A name given twice weighs twice. Each set must be exported first.
- `neural-quant run` and `neural-train evalq` score the int8 model. `neural-train eval` scores a checkpoint on the held-out parts, and `futo-test` stays unread.
- `--features onnx` adds `prod`, which runs a packed `neural.bin` through the shipped decoder, and `hostile`, which feeds its loader damaged files. They need `ORT_LIB_PATH` set to the directory of the static ONNX Runtime.
