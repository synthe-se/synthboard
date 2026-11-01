# Provenance

Every external item that enters this repository is logged here, so that the licence can be chosen later with full knowledge.

## Data

| Source | Licence | Local path | Revision | Use |
|---|---|---|---|---|
| HeliBoard gesture data, codeberg.org/eclexic/heliboard-gesture-data | CC BY-SA 4.0 | `data/raw/heliboard-gesture-data` | `7d39bc9831d088192e133dae9ea6cd2a6127bae4` | Train and test |
| swipe.futo.org, huggingface.co/datasets/futo-org/swipe.futo.org | MIT | `data/raw/swipe.futo.org` | `d71bf5fd7f45b3e7c2ed2d76a21b0dbd3b4ba566` | Train and test |
| HeliBoard word lists, codeberg.org/eclexic/heliboard-word-list-csvs | GPL-3.0 | `data/raw/heliboard-word-list-csvs` | `76213fd8d4e6da7fec59e61b2d81574b1431ed13` | Benchmark lexicons, and the source of the shipped `lexicon.csv`. Must be replaced or its licence accepted before any publication |
| OPUS OpenSubtitles v2018, object.pouta.csc.fi/OPUS-OpenSubtitles/v2018/mono | No open licence found; rights holders may ask for removal | `data/raw/ngrams` (aggregate counts only) | v2018, first 300M tokens per language | Next words and context table. Private use only: must be replaced before any publication |
| Lexique 3.83, lexique.org | CC BY-SA 4.0 | `data/raw/lexique` | 3.83 | French verb agreement |
| Swipe model `neural.bin`, trained here on the FUTO and HeliBoard gestures above | Derived from CC BY-SA 4.0 data | `data/raw/neural-spike` | run `a128fr`, 2026-11-01 | Word reading. Generated, not committed. Must be retrained without HeliBoard or its licence accepted before any publication |
| Unicode emoji-test.txt | Unicode License V3 | `data/raw/emoji` | 18.0 | Emoji catalogue |
| CLDR annotations, cldr-annotations-full | Unicode License V3 | `data/raw/emoji` | 49.0.0 | Emoji search keywords |

The assets in `android/app/src/main/assets` are generated from these sources by `tools/` and are not committed, apart from `grammar.tsv`, which is written by hand. Personal traces and learned words stay in `data/raw/personal`, which is ignored.

## Code

| Source | Licence | Files here | What was taken |
|---|---|---|---|

Code from a GPL source is allowed for private use, but it forces the GPL on any later publication.

## Dependencies

Every direct dependency, and why it earns its place. `cargo machete` finds none unused. Everything else in `Cargo.lock` comes through these.

| Crate | Used by | Why |
|---|---|---|
| `hpke` | `synthboard-seal` | RFC 9180, so that sealing is not a home-made construction. It brings the RustCrypto and dalek crates |
| `sha2` | `synthboard-seal` | Key ids. Already present through `hpke` |
| `zeroize` | `synthboard-core`, `synthboard-seal`, `synthboard-ffi` | Wiping the mirrors and keys in place |
| `unicode-normalization` | `synthswipe` | Folding accents to their keys |
| `unicode-segmentation` | `synthboard-core` | One backspace removes one grapheme, emoji included |
| `chacha20poly1305`, `hkdf`, `x25519-dalek` | `synthboard-seal`, `legacy` feature only | Opening records made before HPKE, on the Mac. Not in the phone's library |
| `ort` | `synthswipe-onnx` | Runs the model, on the static ONNX Runtime 1.31.1 that `tools/build-ort.sh` builds. Chosen for speed on the A520 |
| `libfuzzer-sys` | `fuzz/` | The fuzzing harness, never shipped |
| `rayon`, `ruzstd`, `serde`, `serde_json` | `synthswipe-bench` | Mac tools only: parallel replay and reading the datasets |
| `androidx.autofill` | Android app | The style bundle password managers require for inline suggestions |
| Robolectric, Roborazzi, JUnit, AndroidX Test | Android tests | The shell's tests, never shipped |

`zeroize_derive` and `syn` run at build time only because `hpke` requires them for one derive.

### Linked into the phone's library

ONNX Runtime 1.31.1, MIT, commit `6af1a932e2286d5c58166085d24c68783033edee`, source in `data/raw/ort/ortsrc`. `tools/build-ort.sh` merges it into `libonnxruntime.a` with what it depends on, at the versions of its `cmake/deps.txt`. MLAS is part of it. The licences below were read from each source tree.

| Component | Version | Licence | In the archive |
|---|---|---|---|
| Abseil | 20250814.0 | Apache-2.0 | Compiled |
| cpuinfo | `66ee79c0` | BSD-2-Clause | Compiled |
| FlatBuffers | 23.5.26 | Apache-2.0 | Compiled |
| KleidiAI | 1.30.0 | Apache-2.0 | Compiled |
| ONNX | 1.22.0 | Apache-2.0 | Compiled |
| Protocol Buffers, lite runtime | 33.6 | BSD-3-Clause | Compiled |
| Eigen | `1d8b82b0` | MPL-2.0, some files under others | Headers |
| date | 3.0.1 | MIT | Headers |
| GSL | 4.2.1 | MIT | Headers |
| Boost.Mp11 | 1.82.0 | BSL-1.0 | Headers |
| nlohmann/json | 3.11.3 | MIT | Headers |
| SafeInt | 3.0.28 | MIT | Headers |

The NDK and the Rust compiler are pinned to their latest stable versions, in `android/gradle.properties` and `rust-toolchain.toml`.
