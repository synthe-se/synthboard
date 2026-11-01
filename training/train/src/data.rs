//! Reads the flat records written by `synthswipe-bench --export` and turns them into model
//! inputs. The format is described in `crates/synthswipe-bench/src/export.rs`.

use std::path::Path;
use synthswipe::{FEATURES, FRAMES, INPUT_VALUES, Key, Layout, MODEL_KEYS, POSITION_SCALE, Sample, frame_features};

pub const KEYS: usize = MODEL_KEYS.len();
/// Keys plus the CTC blank, class 0.
pub const CLASSES: usize = KEYS + 1;

const U32_BYTES: usize = 4;
/// Three `f32`: x, y and milliseconds, or a key code, x and y.
const TRIPLE_BYTES: usize = 3 * U32_BYTES;
const MINIMUM_SAMPLES: usize = 2;

/// A key this far above the other, in pitches, shows a row swapped between layouts.
const ROW_GAP: f32 = 0.3;

// Augmentation: ranges for what a change of phone or hand does, in key pitches.
const SCALE_SPREAD: f32 = 0.08;
/// Row height relative to key width: 1.4 on FUTO QWERTY, 2.1 on its AZERTY.
const TALL_LOW: f32 = 0.8;
const TALL_HIGH: f32 = 1.7;
const SHIFT_X_SPREAD: f32 = 0.25;
const SHIFT_Y_SPREAD: f32 = 0.15;
const HAND_STRETCH_X: f32 = 0.06;
const HAND_STRETCH_Y: f32 = 0.08;
const HAND_SHEAR: f32 = 0.05;
const HAND_SHIFT_SPREAD: f32 = 0.1;
const POINT_NOISE: f32 = 0.02;
/// Log of the widest change of speed.
const SPEED_SPREAD: f32 = 0.35;

/// SplitMix64, by Steele, Lea and Flood. Its multipliers come from that paper.
const GOLDEN_GAMMA: u64 = 0x9E37_79B9_7F4A_7C15;
const MIX_FIRST: u64 = 0xBF58_476D_1CE4_E5B9;
const MIX_SECOND: u64 = 0x94D0_49BB_1331_11EB;
/// Spreads gesture indices over the seed space. A SplitMix-family constant.
const INDEX_SPREAD: u64 = 0xA24B_AED4_963E_E407;
/// 24 random bits fill an `f32` mantissa.
const UNIT_SHIFT: u32 = 40;
const UNIT_RANGE: f32 = (1u64 << 24) as f32;
/// Keeps the logarithm of Box-Muller away from zero.
const SMALLEST_UNIT: f32 = 1e-6;

pub struct Set {
    bytes: Vec<u8>,
    pub offsets: Vec<usize>,
}

#[derive(Debug, Clone)]
pub struct Record {
    pub session: u32,
    pub word: String,
    /// Class of each folded key, 1 to `KEYS`.
    pub labels: Vec<i32>,
    pub key_xy: [[f32; 2]; KEYS],
    pub key_on: [bool; KEYS],
    /// x, y, milliseconds.
    pub samples: Vec<[f32; 3]>,
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn u32(&mut self) -> Option<u32> {
        let end = self.at.checked_add(U32_BYTES)?;
        let value = u32::from_le_bytes(self.bytes.get(self.at..end)?.try_into().ok()?);
        self.at = end;
        Some(value)
    }
    fn f32(&mut self) -> Option<f32> {
        self.u32().map(f32::from_bits)
    }
    fn u8(&mut self) -> Option<usize> {
        let value = *self.bytes.get(self.at)?;
        self.at += 1;
        Some(usize::from(value))
    }
    fn skip(&mut self, count: usize) -> Option<()> {
        self.at = self.at.checked_add(count).filter(|&end| end <= self.bytes.len())?;
        Some(())
    }
    /// Steps over one record and says whether all of it is there.
    fn skip_record(&mut self) -> Option<()> {
        self.skip(U32_BYTES)?;
        let word = self.u8()?;
        self.skip(word)?;
        let keys = self.u8()?;
        self.skip(U32_BYTES * keys)?;
        let places = self.u8()?;
        self.skip(TRIPLE_BYTES * places)?;
        let samples = self.u32()? as usize;
        self.skip(TRIPLE_BYTES.checked_mul(samples)?)
    }
}

impl Set {
    pub fn load(dir: &Path, name: &str) -> Self {
        let path = dir.join(format!("{name}.bin"));
        let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let mut offsets = Vec::new();
        let mut reader = Reader { bytes: &bytes, at: 0 };
        while reader.at < bytes.len() {
            let start = reader.at;
            if reader.skip_record().is_none() {
                eprintln!("{name}: stopped at a cut record, byte {start}");
                break;
            }
            offsets.push(start);
        }
        Self { bytes, offsets }
    }

    pub fn record(&self, index: usize) -> Option<Record> {
        let mut r = Reader { bytes: &self.bytes, at: *self.offsets.get(index)? };
        let session = r.u32()?;
        let length = r.u8()?;
        let word = std::str::from_utf8(self.bytes.get(r.at..r.at + length)?).ok()?.to_owned();
        r.skip(length)?;
        let key_count = r.u8()?;
        let mut labels = Vec::with_capacity(key_count);
        let mut known = true;
        for _ in 0..key_count {
            match MODEL_KEYS.find(char::from_u32(r.u32()?)?) {
                Some(class) => labels.push(class as i32 + 1),
                None => known = false,
            }
        }
        let places = r.u8()?;
        let (mut key_xy, mut key_on) = ([[0.0; 2]; KEYS], [false; KEYS]);
        for _ in 0..places {
            let code = char::from_u32(r.u32()?)?;
            let (x, y) = (r.f32()?, r.f32()?);
            if let Some(key) = MODEL_KEYS.find(code) {
                key_xy[key] = [x, y];
                key_on[key] = true;
            }
        }
        let count = r.u32()? as usize;
        let mut samples = Vec::with_capacity(count);
        for _ in 0..count {
            samples.push([r.f32()?, r.f32()?, r.f32()?]);
        }
        (known && !labels.is_empty() && samples.len() >= MINIMUM_SAMPLES)
            .then_some(Record { session, word, labels, key_xy, key_on, samples })
    }
}

fn key(r: &Record, label: char) -> [f32; 2] {
    MODEL_KEYS.find(label).map_or([0.0; 2], |index| r.key_xy[index])
}

pub fn is_azerty(r: &Record) -> bool {
    key(r, 'a')[1] < key(r, 'q')[1] - ROW_GAP
}

/// The record's layout, the way the keyboard hands one to the decoder.
pub fn layout_of(r: &Record) -> Option<Layout> {
    let keys = MODEL_KEYS.chars().enumerate().filter(|(i, _)| r.key_on[*i]);
    Layout::new(keys.map(|(i, label)| Key { label, x: r.key_xy[i][0], y: r.key_xy[i][1] })).ok()
}

pub fn samples_of(r: &Record) -> Vec<Sample> {
    r.samples.iter().map(|s| Sample { x: s[0], y: s[1], t_ms: s[2].max(0.0) as u32 }).collect()
}

/// Fast and seedable per gesture, so that batches do not depend on which rayon thread built them.
#[derive(Clone)]
pub struct Rng(pub u64);
impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(GOLDEN_GAMMA);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(MIX_FIRST);
        z = (z ^ (z >> 27)).wrapping_mul(MIX_SECOND);
        z ^ (z >> 31)
    }
    fn unit(&mut self) -> f32 {
        (self.next() >> UNIT_SHIFT) as f32 / UNIT_RANGE
    }
    fn signed(&mut self) -> f32 {
        self.unit() * 2.0 - 1.0
    }
    fn gauss(&mut self) -> f32 {
        let (u, v) = (self.unit().max(SMALLEST_UNIT), self.unit());
        (-2.0 * u.ln()).sqrt() * (std::f32::consts::TAU * v).cos()
    }
}

/// A change of phone moves keys and gesture together. A change of hand moves the gesture alone.
struct Change {
    both_scale: f32,
    both_shift: [f32; 2],
    tall: f32,
    hand: [f32; 4],
    hand_shift: [f32; 2],
    speed: f32,
}

impl Change {
    const NONE: Self = Self { both_scale: 1.0, both_shift: [0.0; 2], tall: 1.0, hand: [1.0, 0.0, 0.0, 1.0], hand_shift: [0.0; 2], speed: 1.0 };

    fn draw(g: &mut Rng) -> Self {
        Self {
            both_scale: 1.0 + SCALE_SPREAD * g.signed(),
            tall: (TALL_LOW.ln() + g.unit() * (TALL_HIGH / TALL_LOW).ln()).exp(),
            both_shift: [SHIFT_X_SPREAD * g.gauss(), SHIFT_Y_SPREAD * g.gauss()],
            hand: [1.0 + HAND_STRETCH_X * g.signed(), HAND_SHEAR * g.signed(), HAND_SHEAR * g.signed(), 1.0 + HAND_STRETCH_Y * g.signed()],
            hand_shift: [HAND_SHIFT_SPREAD * g.gauss(), HAND_SHIFT_SPREAD * g.gauss()],
            speed: (SPEED_SPREAD * g.signed()).exp(),
        }
    }
}

/// Fills `x` (`INPUT_VALUES`, feature first, what the model reads), `keys` (`KEYS * 2`) and
/// `mask`. The frames come from the decoder's own code, so training cannot drift from the phone.
pub fn featurize(r: &Record, mut rng: Option<&mut Rng>, x: &mut [f32], keys: &mut [f32], mask: &mut [f32]) {
    let on = r.key_on.iter().filter(|&&o| o).count().max(1) as f32;
    let mut center = [0.0f32; 2];
    for (xy, _) in r.key_xy.iter().zip(&r.key_on).filter(|(_, o)| **o) {
        center[0] += xy[0];
        center[1] += xy[1];
    }
    center = [center[0] / on, center[1] / on];

    let change = rng.as_deref_mut().map_or(Change::NONE, Change::draw);
    let noise = if rng.is_some() { POINT_NOISE } else { 0.0 };
    let points: Vec<[f32; 3]> = r.samples.iter().map(|s| {
        let (px, py) = (s[0] - center[0], (s[1] - center[1]) * change.tall);
        let mut hand_x = change.hand[0] * px + change.hand[1] * py + change.hand_shift[0];
        let mut hand_y = change.hand[2] * px + change.hand[3] * py + change.hand_shift[1];
        if let Some(g) = rng.as_deref_mut() {
            hand_x += noise * g.gauss();
            hand_y += noise * g.gauss();
        }
        [hand_x * change.both_scale + change.both_shift[0], hand_y * change.both_scale + change.both_shift[1], s[2] * change.speed]
    }).collect();
    // A gesture the decoder refuses stays zeros, which the keyboard would never have fed it.
    if let Some(values) = frame_features(&points) {
        x.copy_from_slice(&values);
    }
    for k in 0..KEYS {
        keys[2 * k] = ((r.key_xy[k][0] - center[0]) * change.both_scale + change.both_shift[0]) * POSITION_SCALE;
        keys[2 * k + 1] = ((r.key_xy[k][1] - center[1]) * change.tall * change.both_scale + change.both_shift[1]) * POSITION_SCALE;
        mask[k] = f32::from(u8::from(r.key_on[k]));
    }
}

pub struct Batch {
    pub n: usize,
    pub x: Vec<f32>,
    pub keys: Vec<f32>,
    pub mask: Vec<f32>,
    pub y: Vec<i32>,
    pub max_l: usize,
    pub lens: Vec<i32>,
}

pub fn batch(records: &[Record], augment: Option<u64>) -> Batch {
    use rayon::prelude::*;
    let n = records.len();
    let max_l = records.iter().map(|r| r.labels.len()).max().unwrap_or(1);
    let (mut x, mut keys, mut mask) = (vec![0.0; n * FEATURES * FRAMES], vec![0.0; n * KEYS * 2], vec![0.0; n * KEYS]);
    x.par_chunks_mut(INPUT_VALUES)
        .zip(keys.par_chunks_mut(KEYS * 2))
        .zip(mask.par_chunks_mut(KEYS))
        .zip(records.par_iter().enumerate())
        .for_each(|(((x, k), m), (i, r))| {
            let mut rng = augment.map(|s| Rng(s ^ (i as u64).wrapping_mul(INDEX_SPREAD)));
            featurize(r, rng.as_mut(), x, k, m);
        });
    let mut y = vec![0; n * max_l];
    let mut lens = Vec::with_capacity(n);
    for (i, r) in records.iter().enumerate() {
        y[i * max_l..i * max_l + r.labels.len()].copy_from_slice(&r.labels);
        lens.push(r.labels.len() as i32);
    }
    Batch { n, x, keys, mask, y, max_l, lens }
}
