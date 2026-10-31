#![no_main]

use libfuzzer_sys::fuzz_target;
use synthswipe::{Key, Layout, Lexicon, Params, Sample, decode};

const KEY_BYTES: usize = 8 * 26;
const TOP_K: usize = 3;

fn floats(bytes: &[u8]) -> impl Iterator<Item = f32> + '_ {
    bytes
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
}

fuzz_target!(|data: &[u8]| {
    let Some((layout_bytes, rest)) = data.split_at_checked(KEY_BYTES) else {
        return;
    };
    let (text, gesture_bytes) = rest.split_at(rest.len() / 2);
    let text = String::from_utf8_lossy(text);
    let lexicon = Lexicon::new(text.split_whitespace().map(|word| (word, 1.0)));

    let coordinates: Vec<f32> = floats(layout_bytes).collect();
    let keys = ('a'..='z').zip(coordinates.chunks_exact(2)).map(|(label, pair)| Key {
        label,
        x: pair[0],
        y: pair[1],
    });
    let Ok(layout) = Layout::new(keys) else {
        return;
    };

    let values: Vec<f32> = floats(gesture_bytes).collect();
    let samples: Vec<Sample> = values
        .chunks_exact(2)
        .map(|pair| Sample {
            x: pair[0],
            y: pair[1],
            t_ms: 0,
        })
        .collect();

    let params = Params::default();
    let Ok(first) = decode(&layout, &lexicon, &samples, &params, TOP_K) else {
        return;
    };
    assert!(first.len() <= TOP_K);
    assert!(first.iter().all(|candidate| !candidate.score.is_nan()), "{first:?}");
    assert!(first.iter().all(|candidate| lexicon.contains(candidate.surface)));
    assert!(first.windows(2).all(|pair| pair[0].score >= pair[1].score));
    let second = decode(&layout, &lexicon, &samples, &params, TOP_K);
    assert_eq!(second.as_ref().map(Vec::len), Ok(first.len()));
    let surfaces = |list: &[synthswipe::Candidate<'_>]| {
        list.iter().map(|c| c.surface.to_owned()).collect::<Vec<_>>()
    };
    assert_eq!(second.map(|list| surfaces(&list)), Ok(surfaces(&first)));
});
