//! Handles the owner's trace keys. Plain traces only ever exist on this Mac.
//!
//! `trace_keys new SECRET_FILE PUBLIC_FILE` makes a key pair, and refuses to
//! replace one. `trace_keys open SECRET_FILE SEALED_FILE OUTPUT` appends every
//! record it can open to OUTPUT, and reports the ones it cannot.

use std::error::Error;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;

use synthboard_seal::{from_hex, generate_keys, open, to_hex};

const OWNER_ONLY: u32 = 0o600;

fn create(secret_path: &str, public_path: &str) -> Result<(), Box<dyn Error>> {
    let (secret, public) = generate_keys().map_err(|_| "no randomness")?;
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(OWNER_ONLY)
        .open(secret_path)?
        .write_all(to_hex(&secret).as_bytes())?;
    fs::write(public_path, to_hex(&public))?;
    Ok(())
}

fn open_all(secret_path: &str, sealed_path: &str, output: &str) -> Result<(), Box<dyn Error>> {
    let hex = fs::read_to_string(secret_path)?;
    let secret: [u8; 32] = from_hex(hex.trim())
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or("invalid secret key")?;
    let mut out = OpenOptions::new().append(true).create(true).mode(OWNER_ONLY).open(output)?;
    let (mut opened, mut failed) = (0, 0);
    for line in fs::read_to_string(sealed_path)?.lines() {
        match from_hex(line).and_then(|record| open(&secret, &record).ok()) {
            Some(plain) => {
                out.write_all(&plain)?;
                out.write_all(b"\n")?;
                opened += 1;
            }
            None => failed += 1,
        }
    }
    println!("{opened} records opened, {failed} refused.");
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let names: Vec<&str> = arguments.iter().map(String::as_str).collect();
    match names.as_slice() {
        ["new", secret, public] => create(secret, public),
        ["open", secret, sealed, output] => open_all(secret, sealed, output),
        _ => Err("usage: trace_keys new SECRET PUBLIC | open SECRET SEALED OUTPUT".into()),
    }
}
