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
| Typed text | Engine RAM while the keyboard shows | Zeroised on hide and on engine replacement. Native buffers wiped after each call. Never written or logged | As commits to the field, nothing else |
| Password and PIN fields | Never read | No request for context, no mirror, no learning, no trace, always out of screenshots | Never |
| Learned words | `memory.sealed` | Sealed in Rust to a device public key. In the clear only inside Rust, only while the keyboard shows | Manual export by the owner |
| Traces | `traces.sealed`, append only | Sealed in Rust before reaching Android. The phone cannot reopen them | A tap in settings, to Tailscale only |
| Personal data on the Mac | Encrypted volume | Passphrase known only to the owner, closed when not in use | Never |

The keyboard declares no permission and contains no network code. `tools/check-release.sh` and `deny.toml` enforce it.

## Cryptography

- **Traces:** HPKE base mode, RFC 9180: DHKEM(X25519, HKDF-SHA256), HKDF-SHA256, ChaCha20-Poly1305. Info is `synthboard trace v2`. AAD is the header: a version byte and a 4-byte key id. Plaintext is padded to multiples of 128 bytes, to hide word lengths. Records have no sender authentication, so a forged record is possible for anyone with the public key: they are data to analyse, never instructions.
- **Older records:** the earlier format stays readable by the owner's tools, behind the `legacy` feature. The phone's library has no such code.
- **Memory:** the same HPKE records, sealed to a key pair of this phone. Saving needs only the public key, so it works on a locked phone and nothing waits in RAM. The secret key is stored under AES-256-GCM with a Keystore key that needs an unlocked phone, with the format as AAD. Kotlin only ever holds sealed bytes, and the secret key in an array it wipes, except in an export the owner asks for.
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
- Rust: `unsafe_code` forbidden everywhere but `crates/synthboard-ffi/src/jni.rs`, three short functions that move bytes, overflow checks on in release, panics stopped at the FFI boundary and silenced, because a panic message could quote what was typed. `panic = "unwind"` is deliberate.
- Boundary: messages travel in buffers outside the Java heap, which the garbage collector never copies and which are wiped after each call. A hand-written codec in safe Rust bounds every length and validates all text, with Kotlin generated from the Rust types with no procedural macro. `fuzz_wire` feeds it hostile bytes.
- Supply chain: `cargo deny`, `cargo vet` and `cargo audit` in the release checks. Any version bump of a crypto crate means reading its diff.

## Known limits

- A field's text reaches the keyboard as a Java `String`, which Android creates in our heap and which cannot be wiped. Reading is therefore limited to 256 characters each side of the cursor, and none in secret fields. Committed text goes the same way, through the platform's own API.
- Memory tagging disables hardened_malloc's zeroing on free, so a freed Rust buffer keeps its bytes until reused. Tagging is kept because it stops an exploit inside the process from reaching them, and because this process handles text from other apps. A memory dump could still see what was not wiped, which is why text is wiped explicitly. Neither covers the Java heap.
- Wiping is best effort where a buffer grows, since the old allocation is freed as it was.

## Standards

Measured against OWASP ASVS 5 level 2 and MASVS where they apply. Waived on purpose: MASVS-RESILIENCE, since there is nothing to hide and the code is open, and ASVS security logging, since a keyboard that logs is the problem.

## Updates

Only the owner publishes builds. Dependencies stay on their latest versions unless this page says otherwise. A flaw in anything listed here is fixed before any new feature.
