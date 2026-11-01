//! One-way sealing of trace records, with HPKE. Each record gets a fresh key
//! pair whose secret half is erased at once, so the phone that wrote a record
//! cannot read it back: only the holder of the owner's secret key can. Anyone
//! who later takes the phone apart finds nothing but ciphertext.

use hpke::aead::ChaCha20Poly1305 as Suite;
use hpke::kdf::HkdfSha256;
use hpke::kem::X25519HkdfSha256;
use hpke::{Deserializable, Kem, OpModeR, OpModeS, Serializable, single_shot_open, single_shot_seal};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

const KEY_BYTES: usize = 32;
const VERSION: u8 = 2;
const KEY_ID_BYTES: usize = 4;
const HEADER_BYTES: usize = 1 + KEY_ID_BYTES;
const INFO: &[u8] = b"synthboard trace v2";
/// Records grow in steps of this size, so that their length says little about
/// the words inside.
const PADDED_BYTES: usize = 128;
const PADDING_START: u8 = 0x80;

type PublicKey = <X25519HkdfSha256 as Kem>::PublicKey;
type PrivateKey = <X25519HkdfSha256 as Kem>::PrivateKey;
type EncappedKey = <X25519HkdfSha256 as Kem>::EncappedKey;

/// A failure that reveals nothing about the record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SealError;

/// Names a public key in its records, so that a rotated key can be told apart.
fn key_id(public: &[u8; KEY_BYTES]) -> [u8; KEY_ID_BYTES] {
    let digest = Sha256::new_with_prefix(b"synthboard key id").chain_update(public).finalize();
    let mut id = [0; KEY_ID_BYTES];
    id.iter_mut().zip(digest).for_each(|(byte, from)| *byte = from);
    id
}

fn header(public: &[u8; KEY_BYTES]) -> [u8; HEADER_BYTES] {
    let mut bytes = [VERSION; HEADER_BYTES];
    bytes.iter_mut().skip(1).zip(key_id(public)).for_each(|(byte, from)| *byte = from);
    bytes
}

fn padded(plain: &[u8]) -> Zeroizing<Vec<u8>> {
    let length = (plain.len() / PADDED_BYTES + 1) * PADDED_BYTES;
    let mut bytes = Zeroizing::new(Vec::with_capacity(length));
    bytes.extend_from_slice(plain);
    bytes.push(PADDING_START);
    bytes.resize(length, 0);
    bytes
}

fn unpadded(mut bytes: Vec<u8>) -> Result<Vec<u8>, SealError> {
    let end = bytes.iter().rposition(|&byte| byte != 0).ok_or(SealError)?;
    if bytes.get(end) != Some(&PADDING_START) {
        return Err(SealError);
    }
    bytes.truncate(end);
    Ok(bytes)
}

/// Makes the owner's key pair, as `(secret, public)`.
///
/// # Errors
/// Fails if the system has no randomness to give.
pub fn generate_keys() -> Result<([u8; KEY_BYTES], [u8; KEY_BYTES]), SealError> {
    let (secret, public) = X25519HkdfSha256::gen_keypair();
    let secret: [u8; KEY_BYTES] = secret.to_bytes().into();
    Ok((secret, public.to_bytes().into()))
}

/// Seals `plain` for the owner of `public`, with HPKE base mode (RFC 9180).
///
/// # Errors
/// Fails on a public key that cannot be used.
pub fn seal(public: &[u8; KEY_BYTES], plain: &[u8]) -> Result<Vec<u8>, SealError> {
    let recipient = PublicKey::from_bytes(public).map_err(|_| SealError)?;
    let header = header(public);
    let (encapped, sealed) = single_shot_seal::<Suite, HkdfSha256, X25519HkdfSha256>(
        &OpModeS::Base,
        &recipient,
        INFO,
        &padded(plain),
        &header,
    )
    .map_err(|_| SealError)?;
    let mut record = Vec::with_capacity(HEADER_BYTES + KEY_BYTES + sealed.len());
    record.extend_from_slice(&header);
    record.extend_from_slice(&encapped.to_bytes());
    record.extend_from_slice(&sealed);
    Ok(record)
}

fn open_current(secret: &PrivateKey, record: &[u8]) -> Result<Vec<u8>, SealError> {
    let (header, rest) = record.split_first_chunk::<HEADER_BYTES>().ok_or(SealError)?;
    let (encapped, sealed) = rest.split_first_chunk::<KEY_BYTES>().ok_or(SealError)?;
    let encapped = EncappedKey::from_bytes(encapped).map_err(|_| SealError)?;
    let plain = single_shot_open::<Suite, HkdfSha256, X25519HkdfSha256>(
        &OpModeR::Base,
        secret,
        &encapped,
        INFO,
        sealed,
        header,
    )
    .map_err(|_| SealError)?;
    unpadded(plain)
}

/// Opens a record with the owner's secret key.
///
/// # Errors
/// Fails on a wrong key or on any change to the record.
pub fn open(secret: &[u8; KEY_BYTES], record: &[u8]) -> Result<Vec<u8>, SealError> {
    let private = PrivateKey::from_bytes(secret).map_err(|_| SealError)?;
    let public: [u8; KEY_BYTES] = X25519HkdfSha256::sk_to_pk(&private).to_bytes().into();
    if record.starts_with(&header(&public)) {
        return open_current(&private, record);
    }
    #[cfg(feature = "legacy")]
    return legacy::open(secret, record);
    #[cfg(not(feature = "legacy"))]
    Err(SealError)
}

/// Records written before HPKE. Only the owner's tools still read them, so the
/// phone's library carries none of this.
#[cfg(feature = "legacy")]
mod legacy {
    use chacha20poly1305::aead::{Aead, KeyInit};
    use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
    use hkdf::Hkdf;
    use sha2::Sha256;
    use x25519_dalek::{PublicKey, StaticSecret};
    use zeroize::Zeroizing;

    use super::{KEY_BYTES, SealError};

    const NONCE_BYTES: usize = 12;
    const CONTEXT: &[u8] = b"synthboard trace record v1";

    pub fn open(secret: &[u8; KEY_BYTES], record: &[u8]) -> Result<Vec<u8>, SealError> {
        let (ephemeral, rest) = record.split_first_chunk::<KEY_BYTES>().ok_or(SealError)?;
        let (nonce, sealed) = rest.split_first_chunk::<NONCE_BYTES>().ok_or(SealError)?;
        let shared = StaticSecret::from(*secret).diffie_hellman(&PublicKey::from(*ephemeral));
        if !shared.was_contributory() {
            return Err(SealError);
        }
        let mut key = Zeroizing::new([0; KEY_BYTES]);
        Hkdf::<Sha256>::new(Some(ephemeral), shared.as_bytes())
            .expand(CONTEXT, key.as_mut())
            .map_err(|_| SealError)?;
        ChaCha20Poly1305::new(&Key::from(*key)).decrypt(&Nonce::from(*nonce), sealed).map_err(|_| SealError)
    }
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

    const VECTOR_SECRET: &str = "f059188f3949f9f282adec2057313abe82e9b64ec484129d793ea741b5cd5f41";
    const VECTOR_RECORD: &str = "0296c449b9b7989801087bd02a6ca1c82b3e709518501119eb96a85dff01975dddf1cad41ee13def6e2f24086e4ca5bb85ba850dbf114b3c72e0e118cb48637260a580bc3107f241da3e88542de5f4978e5774ed88ae8c2d6e2bc2af40c7c5be6e31979f2d9da0821d69b53a6c52c8f157fdd5baf47df813dc3b474695387ba67d59f715dcd346b375d120fa609cad9a0baa0058c3bf04cf85d79bf47b8514efed07336f8c601136ab8f9786311cac964622988c6b";
    #[cfg(feature = "legacy")]
    const LEGACY_RECORD: &str = "73ffd5aa53899b9d72623341028b45fac24279ef605c7badfa7cf517e716be5e0707070707070707070707070a2e83bdd01ebf32afb96452c1007d65155612601db53b";

    #[test]
    fn the_format_never_changes_silently() {
        let secret: [u8; KEY_BYTES] = from_hex(VECTOR_SECRET).unwrap().try_into().unwrap();
        assert_eq!(open(&secret, &from_hex(VECTOR_RECORD).unwrap()).unwrap(), b"je suis");
    }

    #[cfg(feature = "legacy")]
    #[test]
    fn older_records_still_open() {
        let secret: [u8; KEY_BYTES] = from_hex(VECTOR_SECRET).unwrap().try_into().unwrap();
        assert_eq!(open(&secret, &from_hex(LEGACY_RECORD).unwrap()).unwrap(), b"je suis");
    }

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
    fn record_lengths_hide_word_lengths() {
        let (_, public) = generate_keys().unwrap();
        let short = seal(&public, b"oui").unwrap();
        let long = seal(&public, b"anticonstitutionnellement").unwrap();
        assert_eq!(short.len(), long.len());
    }

    #[test]
    fn padding_survives_any_text_length() {
        let (secret, public) = generate_keys().unwrap();
        for length in [0, 1, 126, 127, 128, 129, 255, 256] {
            let plain = vec![0; length];
            assert_eq!(open(&secret, &seal(&public, &plain).unwrap()).unwrap(), plain, "{length}");
        }
    }

    #[test]
    fn records_name_their_version_and_key() {
        let (_, public) = generate_keys().unwrap();
        let (_, other) = generate_keys().unwrap();
        let record = seal(&public, b"texte").unwrap();
        assert_eq!(record[0], VERSION);
        assert_eq!(record[1..HEADER_BYTES], key_id(&public));
        assert_ne!(key_id(&public), key_id(&other));
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
