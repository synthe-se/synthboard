//! Runs the file the keyboard ships, through the decoder of `crates/synthswipe`: the same
//! features, runtime, search and blend.

use crate::data::{layout_of, samples_of};
use crate::{Inputs, eval_sets, shared, FULL_EVAL};
use synthswipe::{Candidate, FREQUENCY_WEIGHT, Mix, NETWORK_WEIGHT, POOL, Params, decode_in_context, floor_of};

/// (network, frequency) weights tried on the odd rows. The shipped pair came from this table.
const WEIGHTS: [(f32, f32); 15] = [
    (0.0, 0.0), (0.25, 0.0), (0.5, 0.0), (1.0, 0.0),
    (0.25, 0.02), (0.5, 0.02), (1.0, 0.02),
    (0.25, 0.04), (0.5, 0.04), (1.0, 0.04),
    (0.25, 0.07), (0.5, 0.07), (1.0, 0.07),
    (0.5, 0.1), (1.0, 0.1),
];
/// Rounds of the three kinds of damage, as in `docs/security.md`.
const HOSTILE_ROUNDS: usize = 300;
const DAMAGE_KINDS: usize = 3;
const FLIPPED_BYTES: usize = 64;
const HOSTILE_SEED: u64 = 7;

fn lower(c: &Candidate<'_>) -> String {
    c.surface.to_lowercase()
}

struct Row {
    word: String,
    geometry: String,
    search: String,
    blends: Vec<String>,
}

fn share(rows: &[&Row], found: impl Fn(&Row) -> bool) -> f64 {
    100.0 * rows.iter().filter(|r| found(r)).count() as f64 / rows.len().max(1) as f64
}

/// Reads the top word at each weight, on the held-out sets. The odd rows choose, the even score.
pub fn prod(i: &Inputs, model: &str) {
    let file = std::fs::read(model).expect("the model file reads");
    let net = synthswipe_onnx::load(&file).expect("the model loads");
    let sh = shared(i);
    let params = Params::default();
    let lexicon = &sh.lexicon;
    for e in eval_sets(i, FULL_EVAL) {
        let rows: Vec<Row> = e.records.iter().filter_map(|r| {
            let layout = layout_of(r)?;
            let samples = samples_of(r);
            let geometry = decode_in_context(&layout, lexicon, &samples, &params, POOL, |_| 0.0).unwrap_or_default();
            let reading = net.read(&layout, &samples)?;
            let blends = WEIGHTS.iter().map(|&(weight, frequency)| {
                let mix = Mix { weight, frequency, floor: floor_of(&geometry) };
                reading.blend(lexicon, &geometry, &params, mix, |_| 0.0).first().map(lower).unwrap_or_default()
            }).collect();
            Some(Row {
                word: r.word.clone(),
                search: reading.words(lexicon, FREQUENCY_WEIGHT).first().map(lower).unwrap_or_default(),
                geometry: geometry.first().map(lower).unwrap_or_default(),
                blends,
            })
        }).collect();
        let even: Vec<&Row> = rows.iter().step_by(2).collect();
        let odd: Vec<&Row> = rows.iter().skip(1).step_by(2).collect();
        println!("{}: n {} geometry {:.2} search alone {:.2}", e.name, even.len(), share(&even, |r| r.geometry == r.word), share(&even, |r| r.search == r.word));
        for (index, weight) in WEIGHTS.iter().enumerate() {
            let chosen = if *weight == (NETWORK_WEIGHT, FREQUENCY_WEIGHT) { " shipped" } else { "" };
            println!(
                "   weight {weight:?}: odd {:.2} even {:.2}{chosen}",
                share(&odd, |r| r.blends[index] == r.word), share(&even, |r| r.blends[index] == r.word),
            );
        }
    }
}

/// Hostile bytes to the runtime's loader: a refusal is fine, an abort is not.
pub fn hostile(model: &str) {
    let good = std::fs::read(model).expect("the model file reads");
    let head_length = synthswipe::MODEL_MAGIC.len() + synthswipe::MODEL_HEAD_BYTES;
    let body = good.len() - head_length;
    let mut rng = crate::data::Rng(HOSTILE_SEED);
    let (mut refused, mut loaded) = (0, 0);
    for round in 0..HOSTILE_ROUNDS {
        let mut b = good.clone();
        match round % DAMAGE_KINDS {
            0 => b.truncate(head_length + 1 + (rng.next() as usize) % (body - 1)),
            1 => {
                for _ in 0..FLIPPED_BYTES {
                    let at = head_length + (rng.next() as usize) % body;
                    b[at] = rng.next() as u8;
                }
            }
            _ => {
                for x in &mut b[head_length..] {
                    *x = rng.next() as u8;
                }
            }
        }
        if synthswipe_onnx::load(&b).is_some() {
            loaded += 1;
        } else {
            refused += 1;
        }
    }
    println!("good file after those: {}", synthswipe_onnx::load(&good).is_some());
    println!("hostile model files: refused {refused}, loaded {loaded}, no abort");
}
