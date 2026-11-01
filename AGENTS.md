# Synthboard: rules for anyone changing this repository

Synthboard is a privacy-first, offline Android swipe keyboard with a reusable open-source decoder. These rules apply to every change, by humans or agents.

## Measure

- The owner's daily typing is the only KPI. Public benchmarks only catch regressions and never prove quality.
- When a complaint arrives, reproduce it on real gestures from the sealed traces before guessing. Unit tests use perfect gestures; humans do not.
- Predictability counts as much as accuracy: the same gesture in the same context gives the same word.

## Engineering

- Almost no timers. Each one needs a written justification beside it. The shell owns a single deadline that the engine sets.
- Event-based: the engine is a state machine that takes `Event` and returns `Command`. No polling. The live preview is the one pull, and only a moving finger triggers it.
- Absolute DRY: one piece of logic lives in exactly one place. A second copy is a bug, including across Rust and Kotlin.
- No magic numbers. A number with a meaning has a name and, unless the name says it all, a comment saying where it comes from: a unit, a measurement, a specification or a tuning run. Only 0, 1, -1, 2 as a half or double, and drawing data such as an icon's points stay bare.
- Logic belongs in Rust. Kotlin stays a thin shell that draws, forwards events, calls Android, and owns threads and files.
- Latest stable dependencies, each one justified. New external data or code is logged in `PROVENANCE.md`.
- Low memory and optimised logic: flat storage, no allocation in hot paths, no linear scan where an index fits. Measure on the device.
- It must never crash. No `unwrap`, no indexing, no panics in production paths; the FFI layer still guards every call. Loaders accept any bytes.
- `unsafe` lives in `crates/synthboard-ffi/src/jni.rs` and `wipe.rs` and nowhere else. ONNX Runtime, static C++ reached through `synthswipe-onnx`, is the one exception to safe native code: it reads only our signed model, and its blocks are wiped through `--wrap=free`. Types that cross to Kotlin are declared with `wire_struct!` or `wire_enum!`, and the Kotlin is generated from them at build time.
- Layouts and languages are data. AZERTY is the owner's layout, never a hard-wired assumption; QWERTY users are first-class. French and English stay active together and the language lean only tilts ranking.
- Research a domain before building it. Finish features against a behaviour inventory; do not present a stage as an endpoint.
- Keyboard product, library and weights from FUTO stay out of options, comparisons and dependencies. Its public swipe datasets are welcome.

## Privacy and security

The model is GrapheneOS: private by default and defence in depth.

- No permissions, ever, and no network code. No telemetry, analytics or third party, for traces either.
- Password fields are never read, predicted, learned or traced. Fields that forbid learning, such as addresses, learn and trace nothing.
- The keyboard never reads the clipboard. Selection, copy and paste belong to the system's own menu.
- Password managers appear through Android's inline suggestions: surfaces the keyboard hosts and cannot read.
- Typed text leaves the engine only as commits to the field and as words for the shell to draw. Traces are sealed with the public key before they leave the engine, and plain text never reaches the Android side.
- Memory is encrypted at rest with a key that needs an unlocked phone, and leaves RAM whenever the keyboard hides. Native memory is zeroed as it is freed, by `wipe.rs`, whether Rust or ONNX Runtime allocated it.
- Personal data and signing keys live on the encrypted volume only (`tools/vault.sh`), reached through the ignored `data/raw/personal`. Secret keys and plain traces never enter Git, logs or a chat. Never print or pass a recovery phrase.
- `docs/security.md` is the threat model. A change to what is stored, read or signed updates it.
- The release stays non-debuggable, without backups, signed by `tools/sign-release.sh`, and passes `tools/check-release.sh`.

## Repository

- Code, comments, identifiers and commits are in English. User-facing strings live in `values/` and `values-fr/`.
- `README.md`, `PROVENANCE.md`, `AGENTS.md` and `docs/security.md` are the only tracked Markdown. Research notes, reports and baselines stay ignored and must stay on disk.
- Comments state why, not what. Doc comments on types that cross a boundary may say what each part is. Keep them short and do not narrate changes.
- Commit or push only when the owner asks. Never bypass a denied permission.

## Checks before calling a change done

```sh
cargo test --workspace
cargo clippy --workspace --all-targets
./tools/test-shell.sh                                 # `record` only for an intended visual change
cd fuzz && RUSTFLAGS="-Clink-arg=-Wl,-undefined,dynamic_lookup" cargo +nightly fuzz run fuzz_engine --sanitizer none -- -max_total_time=120
cargo mutants --in-diff <diff>                        # surviving mutants need a test or a reason
./tools/sign-release.sh && ./tools/check-release.sh   # after `assembleRelease`, with the volume open
```

Every new behaviour gets a test, and new input paths get fuzzed. State plainly what was verified on the device and what was not.
