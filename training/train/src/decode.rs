//! CTC on the lexicon trie, for scoring a checkpoint from a batch of logits. It sums over
//! alignments, the way the training loss does. `prod` scores the shipped decoder instead.

use crate::data::CLASSES;
use std::collections::HashMap;
use synthswipe::{FRAMES, MODEL_KEYS};

/// Beyond this gap, in nats, the smaller term changes a log-sum by less than an `f32` ulp matters.
const NEGLIGIBLE_GAP: f32 = -12.0;

pub struct Trie {
    label: Vec<u8>,
    child_start: Vec<u32>,
    children: Vec<u32>,
    /// Surfaces ending at a node, as word ids.
    ends: Vec<Vec<u32>>,
    pub words: Vec<String>,
    pub weights: Vec<f32>,
}

impl Trie {
    pub fn new(entries: &[(String, f32)]) -> Self {
        let mut label = vec![0u8];
        let mut kids: Vec<Vec<u32>> = vec![vec![]];
        let mut ends: Vec<Vec<u32>> = vec![vec![]];
        let mut lookup: HashMap<(u32, u8), u32> = HashMap::new();
        let (mut words, mut weights) = (vec![], vec![]);
        for (word, weight) in entries {
            let keys: Option<Vec<u8>> = synthswipe::fold(word).into_iter().map(|c| MODEL_KEYS.find(c).map(|i| i as u8 + 1)).collect();
            let Some(keys) = keys else { continue };
            if keys.is_empty() {
                continue;
            }
            let mut node = 0u32;
            for k in keys {
                node = *lookup.entry((node, k)).or_insert_with(|| {
                    let id = label.len() as u32;
                    label.push(k);
                    kids.push(vec![]);
                    ends.push(vec![]);
                    kids[node as usize].push(id);
                    id
                });
            }
            ends[node as usize].push(words.len() as u32);
            words.push(word.clone());
            weights.push(*weight);
        }
        let (mut child_start, mut children) = (vec![0u32], vec![]);
        for k in &kids {
            children.extend_from_slice(k);
            child_start.push(children.len() as u32);
        }
        Self { label, child_start, children, ends, words, weights }
    }
}

fn add(a: f32, b: f32) -> f32 {
    let (hi, lo) = if a > b { (a, b) } else { (b, a) };
    let gap = lo - hi;
    if gap >= NEGLIGIBLE_GAP { hi + gap.exp().ln_1p() } else { hi }
}

/// Reused between gestures, one per thread.
pub struct Scratch {
    blank: Vec<f32>,
    solid: Vec<f32>,
    touched: Vec<u32>,
    stamp: Vec<u32>,
    now: u32,
}

impl Scratch {
    pub fn new(trie: &Trie) -> Self {
        let n = trie.label.len();
        Self { blank: vec![0.0; n], solid: vec![0.0; n], touched: vec![], stamp: vec![0; n], now: 0 }
    }
    fn touch(&mut self, node: u32) {
        let i = node as usize;
        if self.stamp[i] != self.now {
            self.stamp[i] = self.now;
            self.blank[i] = f32::NEG_INFINITY;
            self.solid[i] = f32::NEG_INFINITY;
            self.touched.push(node);
        }
    }
}

/// `lp` is `FRAMES * CLASSES` log-probabilities. Returns (word id, log P) for the words alive at
/// the last frame.
pub fn search(trie: &Trie, lp: &[f32], beam: usize, s: &mut Scratch) -> Vec<(u32, f32)> {
    let mut active: Vec<(u32, f32, f32)> = vec![(0, 0.0, f32::NEG_INFINITY)];
    for t in 0..FRAMES {
        let row = &lp[t * CLASSES..(t + 1) * CLASSES];
        s.now += 1;
        s.touched.clear();
        for &(n, b, v) in &active {
            let all = add(b, v);
            s.touch(n);
            let i = n as usize;
            s.blank[i] = add(s.blank[i], all + row[0]);
            if n != 0 {
                s.solid[i] = add(s.solid[i], v + row[trie.label[i] as usize]);
            }
            let (a, z) = (trie.child_start[i] as usize, trie.child_start[i + 1] as usize);
            for &c in &trie.children[a..z] {
                s.touch(c);
                let j = c as usize;
                s.solid[j] = add(s.solid[j], all + row[trie.label[j] as usize]);
            }
        }
        active.clear();
        for &n in &s.touched {
            active.push((n, s.blank[n as usize], s.solid[n as usize]));
        }
        if active.len() > beam {
            active.select_nth_unstable_by(beam, |x, y| add(y.1, y.2).total_cmp(&add(x.1, x.2)));
            active.truncate(beam);
        }
    }
    let mut out = vec![];
    for &(n, b, v) in &active {
        for &w in &trie.ends[n as usize] {
            out.push((w, add(b, v)));
        }
    }
    out
}
