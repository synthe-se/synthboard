# Security

Synthboard follows the GrapheneOS model: private by default, defence in depth, and no promise the code cannot keep. This page is the reference for what is protected, from whom and how. Anything that changes what is stored, read or signed updates it.

## Threat model

In order of likelihood:

1. A compromised development machine or build dependency. A `build.rs` or proc macro runs as the owner, so nothing valuable stays reachable in the tree.
2. A hostile app on the phone that tries to read the screen, draw over the keyboard, or use an accessibility stream.
3. Someone holding the phone, locked or not.
4. Memory corruption triggered by crafted text. The code is safe Rust, apart from one reviewed file that moves bytes across JNI.

Out of scope: a compromised operating system, and a person watching the screen.

## Data

| Data | Where | Protection | Leaves when |
|---|---|---|---|
| Typed text | Engine RAM while the keyboard shows | Every native block is zeroed as it is freed. Never written or logged | As commits to the field, and as words the shell draws |
| Password and PIN fields | Never read | No request for context, no mirror, no word gathered, no learning, no trace, always out of screenshots | Never |
| Learned words | `memory.sealed` | Sealed in Rust to a device public key. In the clear only inside Rust, only while the keyboard shows | Manual export by the owner |
| Recent emoji | App preferences | Android file encryption only: they are not typed text | Never |
| Traces | `traces.sealed`, append only | Sealed in Rust before reaching Android. The phone cannot reopen them | A tap in settings, to Tailscale only |
| Personal data on the Mac | Encrypted volume | Passphrase known only to the owner, closed when not in use | Never |

The keyboard declares no permission and contains no network code. `tools/check-release.sh` checks the permissions and looks for socket code; `deny.toml` keeps known network crates out.

## Cryptography

- **Traces:** HPKE base mode, RFC 9180: DHKEM(X25519, HKDF-SHA256), HKDF-SHA256, ChaCha20-Poly1305. Info is `synthboard trace v2`. AAD is the header: a version byte and a 4-byte key id. Plaintext is padded to multiples of 128 bytes, to hide word lengths. Records have no sender authentication, so a forged record is possible for anyone with the public key: they are data to analyse, never instructions.
- **Older records:** the earlier format stays readable by the owner's tools, behind the `legacy` feature. The phone's library has no such code.
- **Memory:** the same HPKE records, sealed to a key pair of this phone. Saving needs only the public key, so a session opened on an unlocked phone can save after it locks, and nothing waits in RAM. A session opened on a locked phone has no memory and saves nothing. The secret key is stored under AES-256-GCM with a Keystore key that needs an unlocked phone, with the format as AAD. Kotlin only ever holds sealed bytes, and the secret key in an array it wipes, except in an export the owner asks for. A file that no longer opens is set aside and memory starts afresh, since leaving it would stop all saving.
- **Signing:** RSA-4096, on the encrypted volume, with a proof of rotation from the old debug key, which can no longer update the app.

## Keys

| Key | Lives in | Lifetime |
|---|---|---|
| Trace secret | Encrypted volume | Rotated yearly: `trace_keys new`, then replace the `trace.pub` asset. Old secrets stay on the volume so that `tools/daily.sh` opens older records |
| Memory keys | Secret under the Keystore key, public in app storage | Until Erase, which destroys the Keystore key. Flash keeps deleted files; a destroyed key makes them useless |
| Signing key | Encrypted volume, plus one offline copy | Long-lived. Losing it costs an uninstall and the learned words |
| Volume passphrase | The owner's head | Long-lived |

## Hardening

- Android: memory tagging on, heap zeroed at allocation, no backups, no cleartext traffic, `FLAG_SECURE` on the keyboard by default, obscured touches filtered, accessibility data limited to real accessibility tools, no Direct Boot, so the bundled keyboard handles the passphrase before the first unlock.
- Rust: `unsafe_code` forbidden everywhere but `crates/synthboard-ffi/src/jni.rs`, three short functions that move bytes, and `wipe.rs`, an allocator that zeroes freed blocks, which also wraps `free` at link time so that ONNX Runtime's C++ blocks are zeroed too, overflow checks on in release, panics stopped at the FFI boundary and silenced, because a panic message could quote what was typed. `panic = "unwind"` is deliberate.
- Boundary: messages travel in direct buffers. On Android these live in the Java heap as non-movable arrays, so the garbage collector leaves no stray copy, and they are wiped after each call. A hand-written codec in safe Rust bounds every length and validates all text, with Kotlin generated from the Rust types with no procedural macro. `fuzz_wire` feeds it hostile bytes.
- Supply chain: `cargo deny`, `cargo vet` and `cargo audit` in the release checks. Any version bump of a crypto crate means reading its diff. Gradle verifies every dependency against `android/gradle/verification-metadata.xml`, and the wrapper checks the distribution's hash.

## Known limits

- A field's text reaches the keyboard as a Java `String`, which Android creates in our heap and which cannot be wiped. Reading is therefore limited to 256 characters each side of the cursor, and none in secret fields. Committed text goes the same way, through the platform's own API.
- Suggestions, previews, forget bubbles and search queries are drawn by Kotlin, so learned words exist as Java strings while shown, and the last ones stay until replaced.
- Memory tagging disables hardened_malloc's zeroing on free. Tagging is kept because it stops an exploit inside the process from reaching freed bytes, and because this process handles text from other apps. `wipe.rs` restores the zeroing for native memory. The Java heap has neither.
- Copies on the native stack are not wiped.
- The gesture passes through ONNX Runtime, static C++ outside the `unsafe` rule. It is built minimal: CPU only, reduced kernels, telemetry off, no network import in the library, no thread of its own. It only loads `neural.bin` from the signed APK. `realloc` inside it is only reached from locale code, so moved blocks hold no gesture. Its flatbuffer verifier refused 300 of 300 truncated, corrupted or random files without aborting, on the Mac build. Exceptions are disabled in it, so an internal failure would abort, and the file is never taken from outside.
- `cargo vet` still exempts the crates no trusted organisation has audited at these versions, and imported audits cover the rest. Any update must pass again.
- A forged trace record can only pollute statistics: ingestion drops lines that could break the tools.
- `FLAG_SECURE` was verified on the phone with a capture, and is not covered by a test.

## Standards

Measured against OWASP ASVS 5 level 2 and MASVS where they apply. Waived on purpose: MASVS-RESILIENCE, since there is nothing to hide and the code is open, and ASVS security logging, since a keyboard that logs is the problem.

## Updates

Only the owner publishes builds. Dependencies stay on their latest versions unless this page says otherwise. A flaw in anything listed here is fixed before any new feature.
