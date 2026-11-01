#![no_main]

use libfuzzer_sys::fuzz_target;
use synthswipe::{Encoder, Key, Layout, Lexicon, MODEL_HEAD_BYTES, Mix, Neural, Params, Sample, split_model};

/// Writes hostile values where the model's output goes.
#[derive(Debug)]
struct Hostile(Vec<f32>);

impl Encoder for Hostile {
    fn encode(&self, _: &[f32], output: &mut [f32]) -> bool {
        output.iter_mut().zip(self.0.iter().cycle()).for_each(|(slot, value)| *slot = *value);
        !self.0.is_empty()
    }
}

fuzz_target!(|data: &[u8]| {
    let _ = split_model(data);
    let mut file = b"SYNEURO1".to_vec();
    file.extend(data.iter().copied().cycle().take(MODEL_HEAD_BYTES));
    file.resize(8 + MODEL_HEAD_BYTES, 0);
    file.push(1);
    let Some((head, _)) = split_model(&file) else { return };
    let values: Vec<f32> = data.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
    let neural = Neural::new(head, Box::new(Hostile(values.clone())));
    let keys = "abcdefghij".chars().enumerate().map(|(i, label)| Key { label, x: i as f32, y: 0.0 });
    let Ok(layout) = Layout::new(keys) else { return };
    let samples: Vec<Sample> = values.chunks_exact(3).map(|p| Sample { x: p[0], y: p[1], t_ms: p[2] as u32 }).collect();
    let lexicon = Lexicon::new([("cafe", 100.0), ("bac", 60.0), ("c'est", 90.0)]);
    if let Some(reading) = neural.read(&layout, &samples) {
        let _ = reading.blend(&lexicon, &[], &Params::default(), Mix::shipped(&[]), |_| 0.0);
        let _ = reading.word_score("cafe");
    }
});
