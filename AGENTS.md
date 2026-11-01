# Synthboard: rules for anyone changing this repository

Synthboard is a privacy-first, offline Android swipe keyboard with a reusable open-source decoder. These rules apply to every change, by humans or agents.

## Measure

- The owner's daily typing is the only KPI. Public benchmarks only catch regressions and never prove quality.
- When a complaint arrives, reproduce it on real gestures from the sealed traces before guessing. Unit tests use perfect gestures; humans do not.
- Predictability counts as much as accuracy: the same gesture in the same context gives the same word.

## Engineering

- Almost no timers. Each one needs a written justification beside it. The shell owns a single deadline that the engine sets.
- Event-based: the engine is a state machine that takes `Event` and returns `Command`. No polling.
- Absolute DRY: one piece of logic lives in exactly one place. A second copy is a bug, including across Rust and Kotlin.
- Logic belongs in Rust. Kotlin stays a thin shell that draws, forwards events and calls Android.
- Latest stable dependencies, each one justified. New external data or code is logged in `PROVENANCE.md`.
- Low memory and optimised logic: flat storage, no allocation in hot paths, no linear scan where an index fits. Measure on the device.
- It must never crash. No `unwrap`, no indexing, no panics in production paths; the FFI layer still guards every call. Loaders accept any bytes.
- Layouts and languages are data. AZERTY is the owner's layout, never a hard-wired assumption; QWERTY users are first-class. French and English stay active together and the language lean only tilts ranking.
- Research a domain before building it. Finish features against a behaviour inventory; do not present a stage as an endpoint.
- Keyboard product, library and weights from FUTO stay out of options, comparisons and dependencies. Its public swipe datasets are welcome.

## Privacy and security

The model is GrapheneOS: private by default and defence in depth.

- No permissions, ever, and no network code. No telemetry, analytics or third party, for traces either.
- Password fields are never read, predicted, learned or traced. Fields that forbid learning, such as addresses, learn and trace nothing.
- The keyboard never reads the clipboard. Selection, copy and paste belong to the system's own menu.
- Password managers appear through Android's inline suggestions: surfaces the keyboard hosts and cannot read.
- Text leaves the engine only as commits to the field. Traces are sealed with the public key before they leave the engine, and plain text never reaches the Android side.
- Memory is encrypted at rest. Session text is zeroised when the keyboard closes.
- Personal data stays in the ignored `data/raw/personal/`. Secret keys and plain traces never enter Git, logs or a chat. Never print or pass a recovery phrase.
- The release stays non-debuggable, without backups, and passes `tools/check-release.sh`.

## Repository

- Code, comments, identifiers and commits are in English. User-facing strings live in `values/` and `values-fr/`.
- `PROVENANCE.md` and `AGENTS.md` are the only tracked Markdown. Research notes, reports and baselines stay ignored and must stay on disk.
- Comments state why, not what. Keep them short and do not narrate changes.
- Commit or push only when the owner asks. Never bypass a denied permission.

## Checks before calling a change done

```sh
cargo test --workspace
cargo clippy --workspace --all-targets
cd crates/synthboard-ffi && boltffi generate kotlin   # after changing any #[data] type
./tools/test-shell.sh                                 # `record` only for an intended visual change
cd fuzz && RUSTFLAGS="-Clink-arg=-Wl,-undefined,dynamic_lookup" cargo +nightly fuzz run fuzz_engine --sanitizer none -- -max_total_time=120
cargo mutants --in-diff <diff>                        # surviving mutants need a test or a reason
./tools/check-release.sh                              # after `assembleRelease`
```

Every new behaviour gets a test, and new input paths get fuzzed. State plainly what was verified on the device and what was not.
