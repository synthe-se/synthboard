//! One-way sealing of trace records. Each record gets a fresh key pair whose
//! secret half is erased at once, so the phone that wrote a record cannot read
//! it back: only the holder of the owner's secret key can. Anyone who later
//! takes the phone apart finds nothing but ciphertext.

use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use hkdf::Hkdf;
use sha2::Sha256;
use x25519_dalek::{PublicKey, StaticSecret};
use zeroize::Zeroizing;

const KEY_BYTES: usize = 32;
const NONCE_BYTES: usize = 12;
const CONTEXT: &[u8] = b"synthboard trace record v1";

/// A failure that reveals nothing about the record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SealError;

fn random<const N: usize>() -> Result<[u8; N], SealError> {
    let mut bytes = [0; N];
    getrandom::fill(&mut bytes).map_err(|_| SealError)?;
    Ok(bytes)
}

fn cipher(shared: &[u8], ephemeral: &[u8; KEY_BYTES]) -> Result<ChaCha20Poly1305, SealError> {
    let mut key = Zeroizing::new([0; KEY_BYTES]);
    Hkdf::<Sha256>::new(Some(ephemeral), shared)
        .expand(CONTEXT, key.as_mut())
        .map_err(|_| SealError)?;
    Ok(ChaCha20Poly1305::new(&Key::from(*key)))
}

/// Makes the owner's key pair, as `(secret, public)`.
///
/// # Errors
/// Fails if the system has no randomness to give.
pub fn generate_keys() -> Result<([u8; KEY_BYTES], [u8; KEY_BYTES]), SealError> {
    let secret = StaticSecret::from(random::<KEY_BYTES>()?);
    Ok((secret.to_bytes(), PublicKey::from(&secret).to_bytes()))
}

/// Seals `plain` for the owner of `public`.
///
/// # Errors
/// Fails if the system has no randomness to give.
pub fn seal(public: &[u8; KEY_BYTES], plain: &[u8]) -> Result<Vec<u8>, SealError> {
    let secret = StaticSecret::from(random::<KEY_BYTES>()?);
    let ephemeral = PublicKey::from(&secret).to_bytes();
    let shared = secret.diffie_hellman(&PublicKey::from(*public));
    if !shared.was_contributory() {
        return Err(SealError);
    }
    let nonce = random::<NONCE_BYTES>()?;
    let sealed = cipher(shared.as_bytes(), &ephemeral)?
        .encrypt(&Nonce::from(nonce), plain)
        .map_err(|_| SealError)?;
    let mut record = Vec::with_capacity(KEY_BYTES + NONCE_BYTES + sealed.len());
    record.extend_from_slice(&ephemeral);
    record.extend_from_slice(&nonce);
    record.extend_from_slice(&sealed);
    Ok(record)
}

/// Opens a record with the owner's secret key.
///
/// # Errors
/// Fails on a wrong key or on any change to the record.
pub fn open(secret: &[u8; KEY_BYTES], record: &[u8]) -> Result<Vec<u8>, SealError> {
    let (ephemeral, rest) = record.split_first_chunk::<KEY_BYTES>().ok_or(SealError)?;
    let (nonce, sealed) = rest.split_first_chunk::<NONCE_BYTES>().ok_or(SealError)?;
    let shared = StaticSecret::from(*secret).diffie_hellman(&PublicKey::from(*ephemeral));
    if !shared.was_contributory() {
        return Err(SealError);
    }
    cipher(shared.as_bytes(), ephemeral)?
        .decrypt(&Nonce::from(*nonce), sealed)
        .map_err(|_| SealError)
}

/// Lowercase hex, for one record per line in an append-only text file.
#[must_use]
pub fn to_hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    bytes
        .iter()
        .flat_map(|byte| [byte >> 4, byte & 15])
        .map(|digit| char::from(DIGITS.get(usize::from(digit)).copied().unwrap_or(b'0')))
        .collect()
}

/// The reverse of [`to_hex`], or nothing on any invalid input.
#[must_use]
pub fn from_hex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    text.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&[high, low]| {
            let high = char::from(high).to_digit(16)?;
            let low = char::from(low).to_digit(16)?;
            u8::try_from(high * 16 + low).ok()
        })
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn the_owner_reads_what_was_sealed() {
        let (secret, public) = generate_keys().unwrap();
        let record = seal(&public, "je suis".as_bytes()).unwrap();
        assert_eq!(open(&secret, &record).unwrap(), "je suis".as_bytes());
    }

    #[test]
    fn nobody_else_reads_it() {
        let (_, public) = generate_keys().unwrap();
        let (other, _) = generate_keys().unwrap();
        let record = seal(&public, b"prive").unwrap();
        assert_eq!(open(&other, &record), Err(SealError));
    }

    #[test]
    fn the_text_never_appears_in_the_record() {
        let (_, public) = generate_keys().unwrap();
        let record = seal(&public, b"bonjour bonjour bonjour").unwrap();
        assert!(!record.windows(7).any(|window| window == b"bonjour"));
    }

    #[test]
    fn two_records_of_the_same_text_look_unrelated() {
        let (_, public) = generate_keys().unwrap();
        assert_ne!(seal(&public, b"oui").unwrap(), seal(&public, b"oui").unwrap());
    }

    #[test]
    fn any_change_to_a_record_is_detected() {
        let (secret, public) = generate_keys().unwrap();
        let record = seal(&public, b"texte").unwrap();
        for index in 0..record.len() {
            let mut changed = record.clone();
            changed[index] ^= 1;
            assert_eq!(open(&secret, &changed), Err(SealError), "byte {index}");
        }
        assert_eq!(open(&secret, &record[..record.len() - 1]), Err(SealError));
        assert_eq!(open(&secret, &[]), Err(SealError));
    }

    #[test]
    fn a_low_order_public_key_is_refused() {
        assert_eq!(seal(&[0; KEY_BYTES], b"texte"), Err(SealError));
    }

    #[test]
    fn hex_round_trips_and_refuses_junk() {
        assert_eq!(to_hex(&[0, 15, 255]), "000fff");
        assert_eq!(from_hex("000fff"), Some(vec![0, 15, 255]));
        assert_eq!(from_hex("0"), None);
        assert_eq!(from_hex("zz"), None);
        assert_eq!(from_hex("é"), None);
    }
}
