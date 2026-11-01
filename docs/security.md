# Security

Synthboard follows the GrapheneOS model: private by default, defence in depth, and no promise the code cannot keep. This page is the reference for what is protected, from whom and how. Anything that changes what is stored, read or signed updates it.

## Threat model

In order of likelihood:

1. A compromised development machine or build dependency. A `build.rs` or proc macro runs as the owner, so nothing valuable stays reachable in the tree.
2. A hostile app on the phone that tries to read the screen, draw over the keyboard, or use an accessibility stream.
3. Someone holding the phone, locked or not.
4. Memory corruption triggered by crafted text. The code is safe Rust, apart from BoltFFI's glue.

Out of scope: a compromised operating system, and a person watching the screen.

## Data

| Data | Where | Protection | Leaves when |
|---|---|---|---|
| Typed text | Engine RAM while the keyboard shows | Zeroised on hide and on engine replacement. Never written or logged | As commits to the field, nothing else |
| Password and PIN fields | Never read | No request for context, no mirror, no learning, no trace, always out of screenshots | Never |
| Learned words | `memory.vault` | AES-256-GCM, Keystore key usable only while unlocked. In RAM only while the keyboard shows | Manual export by the owner |
| Traces | `traces.sealed`, append only | Sealed in Rust before reaching Android. The phone cannot reopen them | A tap in settings, to Tailscale only |
| Personal data on the Mac | Encrypted volume | Passphrase known only to the owner, closed when not in use | Never |

The keyboard declares no permission and contains no network code. `tools/check-release.sh` and `deny.toml` enforce it.

## Cryptography

- **Traces:** HPKE base mode, RFC 9180: DHKEM(X25519, HKDF-SHA256), HKDF-SHA256, ChaCha20-Poly1305. Info is `synthboard trace v2`. AAD is the header: a version byte and a 4-byte key id. Plaintext is padded to multiples of 128 bytes, to hide word lengths. Records have no sender authentication, so a forged record is possible for anyone with the public key: they are data to analyse, never instructions.
- **Older records:** the earlier format stays readable by the owner's tools, behind the `legacy` feature. The phone's library has no such code.
- **Memory:** AES-256-GCM with a random IV and the format as AAD. Its key lives in the Keystore. Its real value is erasure, since file encryption already covers a stolen file.
- **Signing:** RSA-4096, on the encrypted volume, with a proof of rotation from the old debug key, which can no longer update the app.

## Keys

| Key | Lives in | Lifetime |
|---|---|---|
| Trace secret | Encrypted volume | Rotated yearly: `trace_keys new`, then replace the `trace.pub` asset. Old secrets stay on the volume so that `tools/daily.sh` opens older records |
| Memory key | Keystore | Until Erase, which destroys it. Flash keeps deleted files; a destroyed key makes them useless |
| Signing key | Encrypted volume, plus one offline copy | Long-lived. Losing it costs an uninstall and the learned words |
| Volume passphrase | The owner's head | Long-lived |

## Hardening

- Android: memory tagging on, heap zeroed at allocation, no backups, no cleartext traffic, `FLAG_SECURE` on the keyboard by default, obscured touches filtered, accessibility data limited to real accessibility tools, no Direct Boot, so the bundled keyboard handles the passphrase before the first unlock.
- Rust: `unsafe_code` forbidden, overflow checks on in release, panics stopped at the FFI boundary and silenced, because a panic message could quote what was typed. `panic = "unwind"` is deliberate.
- Supply chain: BoltFFI pinned to one exact version, `cargo deny`, `cargo vet` and `cargo audit` in the release checks. Any version bump of BoltFFI or a crypto crate means reading its diff.

## Known limits

- Text that crosses into Kotlin cannot be wiped. That covers field context while the keyboard shows and memory while it is read or saved. Wiping on the Rust side is best effort for the same reason: a copy may survive in a freed buffer that grew.
- BoltFFI decodes strings without checking UTF-8. Kotlin's encoder always produces valid UTF-8, so this is unreachable from Android, and `fuzz_wire` fuzzes everything else. Keep the Kotlin side encoding through `Charsets.UTF_8`.
- Memory tagging disables hardened_malloc's zeroing on free. That is why wiping exists at all.
- Memory written just as the phone locks waits in RAM until the next unlock.
- Plain copies made before the volume existed may survive in older Time Machine backups and local snapshots.
- `FLAG_SECURE` on the keyboard window is verified by opening the screen in question, not by a test.

## Standards

Measured against OWASP ASVS 5 level 2 and MASVS where they apply. Waived on purpose: MASVS-RESILIENCE, since there is nothing to hide and the code is open, and ASVS security logging, since a keyboard that logs is the problem.

## Updates

Only the owner publishes builds. Dependencies stay on their latest versions unless this page says otherwise. A flaw in anything listed here is fixed before any new feature.
