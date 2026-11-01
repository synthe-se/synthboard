//! Writes gestures as flat records for training, through the loaders the
//! benchmark uses, so that the two can never read a dataset differently.
//!
//! A file is a list of records, little endian: `u32` session hash, `u8` word
//! length then its UTF-8, `u8` key count then its key codes, `u8` layout key
//! count then each key as `u32` code, `f32` x, `f32` y, then `u32` sample count
//! and each sample as `f32` x, `f32` y, `f32` milliseconds. Coordinates are in
//! key pitches, so that every screen size means the same thing.

use std::error::Error;
use std::fs::File;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::{BufWriter, Write};
use std::path::Path;

use synthswipe::{MODEL_KEYS, fold};

use crate::load::Gesture;

/// One output file.
pub struct Writer {
    out: BufWriter<File>,
    pub records: u64,
}

impl Writer {
    pub fn create(directory: &Path, name: &str) -> Result<Self, Box<dyn Error + Send + Sync>> {
        let file = File::create(directory.join(format!("{name}.bin")))?;
        Ok(Self { out: BufWriter::new(file), records: 0 })
    }

    /// Writes one gesture, or skips it if a key or a length does not fit.
    pub fn write(&mut self, gesture: &Gesture, session: &str) -> Result<(), Box<dyn Error + Send + Sync>> {
        let pitch = gesture.layout.pitch();
        let keys: Vec<u32> = fold(&gesture.word).into_iter().map(u32::from).collect();
        let centers: Vec<(char, [f32; 2])> = MODEL_KEYS
            .chars()
            .filter_map(|label| gesture.layout.center(label).map(|center| (label, center)))
            .collect();
        let all_on_layout = fold(&gesture.word).iter().all(|&key| gesture.layout.center(key).is_some());
        let (Ok(word_len), Ok(key_len), Ok(layout_len), Ok(samples)) = (
            u8::try_from(gesture.word.len()),
            u8::try_from(keys.len()),
            u8::try_from(centers.len()),
            u32::try_from(gesture.samples.len()),
        ) else {
            return Ok(());
        };
        if !all_on_layout || samples < 2 || keys.is_empty() {
            return Ok(());
        }
        let mut hasher = DefaultHasher::new();
        session.hash(&mut hasher);
        let out = &mut self.out;
        let hash = hasher.finish().to_le_bytes();
        out.write_all(hash.get(..size_of::<u32>()).unwrap_or_default())?;
        out.write_all(&[word_len])?;
        out.write_all(gesture.word.as_bytes())?;
        out.write_all(&[key_len])?;
        for key in keys {
            out.write_all(&key.to_le_bytes())?;
        }
        out.write_all(&[layout_len])?;
        for (label, [x, y]) in centers {
            out.write_all(&u32::from(label).to_le_bytes())?;
            out.write_all(&(x / pitch).to_le_bytes())?;
            out.write_all(&(y / pitch).to_le_bytes())?;
        }
        out.write_all(&samples.to_le_bytes())?;
        for sample in &gesture.samples {
            out.write_all(&(sample.x / pitch).to_le_bytes())?;
            out.write_all(&(sample.y / pitch).to_le_bytes())?;
            out.write_all(&(sample.t_ms as f32).to_le_bytes())?;
        }
        self.records += 1;
        Ok(())
    }

    pub fn finish(mut self) -> Result<u64, Box<dyn Error + Send + Sync>> {
        self.out.flush()?;
        Ok(self.records)
    }
}
