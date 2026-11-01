//! Trains the swipe model on gestures exported by `synthswipe-bench --export`, and hands the
//! result to `training/quant`. Only public gestures go in: see the training section of
//! `CONTRIBUTING.md`.

mod data;
mod decode;
mod model;
#[cfg(feature = "onnx")]
mod shipped;

use burn::grad_clipping::GradientClippingConfig;
use burn::module::Module;
use burn::nn::loss::CTCLossConfig;
use burn::optim::{AdamWConfig, GradientsParams};
use burn::prelude::*;
use burn::tensor::TensorData;
use burn_onnx::export::OnnxExporter;
use burn_store::{ModuleSnapshot, SafetensorsStore};
use data::{Batch, CLASSES, Record, Rng, Set, batch, is_azerty, layout_of, samples_of};
use decode::{Scratch, Trie, search};
use model::{Deploy, Encoder, ema, inputs, values};
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Instant;
use synthswipe::{FRAMES, Lexicon, Params, decode_in_context};

const USAGE: &str = "usage:
  neural-train train --data DIR --lexicon FILE --out DIR [--name N] [--width W] [--epochs E]
                     [--cap C] [--seed S] [--sources A,B,C] [--resume CHECKPOINT] [--lr R]
  neural-train eval CHECKPOINT --data DIR --lexicon FILE [--width W] [--limit L]
  neural-train head CHECKPOINT OUT_DIR [--width W]
  neural-train export CHECKPOINT OUT_DIR --data DIR [--width W]
  neural-train evalq CHECKPOINT OUTPUTS_DIR --data DIR --lexicon FILE [--width W]
  neural-train prod MODEL --data DIR --lexicon FILE      (feature onnx)
  neural-train hostile MODEL                             (feature onnx)";

const BATCH: usize = 256;
const EVAL_BATCH: usize = 512;
/// A wide beam, so that the search does not hide what a checkpoint can do.
const BEAM: usize = 256;
const BASE_LR: f64 = 2e-3;
const MIN_LR: f64 = 1e-5;
const WARMUP_STEPS: usize = 500;
const WEIGHT_DECAY: f32 = 1e-2;
const GRADIENT_NORM: f32 = 5.0;
const EMA_EVERY: usize = 10;
const EMA_DECAY: f32 = 0.99;
const LOG_EVERY: usize = 500;
/// How far the printed loss remembers earlier steps.
const LOSS_MEMORY: f32 = 0.98;
/// Batches built ahead of the backend.
const FEED_DEPTH: usize = 8;
const FEED_SEED_MULTIPLIER: u64 = 31;
const FEED_SEED_SHIFT: u32 = 20;

/// Geometry candidates compared with the network.
const GEOMETRY_TOP: usize = 32;
const TOP_THREE: usize = 3;
/// Frequency weights tried on the odd gestures, to read the score on the even ones.
const WEIGHT_STEPS: [f32; 8] = [0.0, 0.01, 0.02, 0.04, 0.07, 0.1, 0.15, 0.2];

const DEFAULT_WIDTH: usize = 128;
const DEFAULT_CAP: usize = 500;
/// FUTO alone, so that a default run needs no share-alike data.
const DEFAULT_SOURCES: &str = "futo-train,futo-2,futo-3,futo-4,futo-5-en";
/// Gestures of a set scored after each epoch, and in a final or stand-alone evaluation.
const EPOCH_EVAL: usize = 2000;
const FULL_EVAL: usize = 8000;

/// One FUTO-5 AZERTY session in this many stays held out.
const HELDOUT_EVERY: u32 = 5;
const FNV_OFFSET: u64 = 0xcbf2_9ce4_8423_2325;
const FNV_PRIME: u64 = 0x0100_0000_01b3;

// Paired bootstrap, on 1000 resamples: its 2.5% and 97.5% points make a 95% interval.
const RESAMPLES: usize = 1000;
const INTERVAL_LOW: usize = 25;
const INTERVAL_HIGH: usize = 974;
const BOOTSTRAP_SEED: u64 = 11;
/// Below this, sessions are too few to resample, so gestures are.
const MINIMUM_SESSIONS: usize = 20;

// Calibration: at most this many gestures of a word, from tuned rows only, never scored rows.
const CALIBRATION_PER_SET: usize = 256;
const CALIBRATION_TOTAL: usize = 1024;
const CALIBRATION_PER_WORD: usize = 4;
const CALIBRATION_STRIDE: usize = 7;

/// `--data` and `--lexicon` for the commands that read gestures.
struct Inputs {
    dir: PathBuf,
    lexicon: Option<PathBuf>,
    /// The HeliBoard group held for tuning, where that optional set is present.
    tune: Option<u32>,
}

/// Personal traces never train a model that could be published, so a path that names them stops
/// here. This is a guard against a slip, not a proof: nothing else reads such a path.
fn public_only(path: &Path) -> PathBuf {
    let real = path.canonicalize().unwrap_or_else(|e| fail(&format!("{}: {e}", path.display())));
    // Both forms, since `data/raw/personal` is a link to the encrypted volume.
    if path.components().chain(real.components()).any(|part| part.as_os_str() == "personal") {
        fail("personal data must not enter training");
    }
    real
}

fn fail(message: &str) -> ! {
    eprintln!("{message}\n{USAGE}");
    std::process::exit(2);
}

fn has_set(dir: &Path, name: &str) -> bool {
    dir.join(format!("{name}.bin")).exists()
}

impl Inputs {
    fn new(args: &Args) -> Self {
        let dir = public_only(Path::new(args.need("--data")));
        let lexicon = args.flag("--lexicon").map(|path| public_only(Path::new(path)));
        let tune = has_set(&dir, HELIBOARD).then(|| smaller_group(&Set::load(&dir, HELIBOARD)));
        Self { dir, lexicon, tune }
    }

    fn set(&self, name: &str) -> Set {
        Set::load(&self.dir, name)
    }

    fn entries(&self) -> Vec<(String, f32, u8)> {
        let path = self.lexicon.as_ref().unwrap_or_else(|| fail("--lexicon is required"));
        let text = std::fs::read_to_string(path).unwrap_or_else(|e| fail(&format!("{}: {e}", path.display())));
        text.lines().filter_map(|l| {
            let mut fields = l.rsplitn(3, ',');
            let language = fields.next()?.parse().ok()?;
            let weight = fields.next()?.parse().ok()?;
            Some((fields.next()?.to_owned(), weight, language))
        }).collect()
    }
}

const FUTO_DEV: &str = "futo-dev";
const FUTO_FIVE: &str = "futo-5";
const HELIBOARD: &str = "heliboard-fr";

// ---------- evaluation sets ----------
struct Eval {
    name: String,
    records: Vec<Record>,
}

fn every_nth(set: &Set, want: usize, keep: impl Fn(&Record) -> bool + Sync) -> Vec<Record> {
    let all: Vec<Record> = (0..set.offsets.len()).into_par_iter().filter_map(|i| set.record(i)).filter(|r| keep(r)).collect();
    let step = (all.len() / want.max(1)).max(1);
    all.into_iter().step_by(step).take(want).collect()
}

fn fnv(s: &str) -> u64 {
    s.bytes().fold(FNV_OFFSET, |h, b| (h ^ u64::from(b)).wrapping_mul(FNV_PRIME))
}

/// Which half of the French tune group a gesture falls in.
fn half_key(r: &Record) -> u64 {
    fnv(&r.word) ^ r.samples.len() as u64
}

/// The export hashes the group name, and `tune` is the smaller of the two groups.
fn smaller_group(set: &Set) -> u32 {
    let mut counts: HashMap<u32, usize> = HashMap::new();
    for i in 0..set.offsets.len() {
        if let Some(r) = set.record(i) {
            *counts.entry(r.session).or_default() += 1;
        }
    }
    counts.into_iter().min_by_key(|e| e.1).map_or(0, |e| e.0)
}

/// Heldout parts only. `futo-test` and the HeliBoard `test` group stay unread. A set that was not
/// exported is left out, since HeliBoard's share-alike data is optional.
fn eval_sets(i: &Inputs, limit: usize) -> Vec<Eval> {
    let mut sets = vec![Eval { name: "futo-dev en qwerty".into(), records: every_nth(&i.set(FUTO_DEV), limit, |_| true) }];
    if let Some(tune) = i.tune {
        let records = every_nth(&i.set(HELIBOARD), limit, |r| r.session == tune && half_key(r).is_multiple_of(2) && is_azerty(r));
        sets.push(Eval { name: "heliboard fr azerty".into(), records });
    }
    let records = every_nth(&i.set(FUTO_FIVE), limit, |r| is_azerty(r) && r.session % HELDOUT_EVERY == 0);
    sets.push(Eval { name: "futo-5 fr azerty".into(), records });
    sets
}

#[cfg(feature = "metal")]
fn device() -> Device {
    Device::metal(Default::default())
}

#[cfg(not(feature = "metal"))]
fn device() -> Device {
    Device::flex()
}

/// Log-probabilities per record, `FRAMES * CLASSES` each.
fn forward_all(m: &Encoder, records: &[Record], d: &Device) -> Vec<Vec<f32>> {
    let mut out = Vec::with_capacity(records.len());
    for chunk in records.chunks(EVAL_BATCH) {
        let b = batch(chunk, None);
        let i = inputs(&b, d);
        split_gestures(&values(m.log_probs(i.x, i.keys, i.mask)), b.n, &mut out);
    }
    out
}

/// Cuts [T, B, CLASSES] values into one `FRAMES * CLASSES` row per gesture.
fn split_gestures(values: &[f32], gestures: usize, out: &mut Vec<Vec<f32>>) {
    for g in 0..gestures {
        let mut one = vec![0.0; FRAMES * CLASSES];
        for t in 0..FRAMES {
            let at = (t * gestures + g) * CLASSES;
            one[t * CLASSES..(t + 1) * CLASSES].copy_from_slice(&values[at..at + CLASSES]);
        }
        out.push(one);
    }
}

struct Shared {
    trie: Trie,
    lexicon: Arc<Lexicon>,
}

fn shared(i: &Inputs) -> Shared {
    let e = i.entries();
    let plain: Vec<(String, f32)> = e.iter().map(|(w, f, _)| (w.to_lowercase(), *f)).collect();
    Shared { trie: Trie::new(&plain), lexicon: Arc::new(Lexicon::with_languages(e.iter().map(|(w, f, l)| (w.as_str(), *f, *l)))) }
}

fn geometry(r: &Record, lexicon: &Lexicon, top: usize) -> Vec<String> {
    let Some(layout) = layout_of(r) else { return vec![] };
    decode_in_context(&layout, lexicon, &samples_of(r), &Params::default(), top, |_| 0.0)
        .unwrap_or_default().iter().map(|c| c.surface.to_lowercase()).collect()
}

fn hit(top: &[String], word: &str, k: usize) -> bool {
    top.iter().take(k).any(|w| w == word)
}

fn ranked(cands: &[(u32, f32)], t: &Trie, beta: f32) -> Vec<String> {
    let mut v: Vec<(f32, u32)> = cands.iter().map(|&(w, s)| (s + beta * t.weights[w as usize], w)).collect();
    v.sort_by(|a, b| b.0.total_cmp(&a.0));
    v.iter().take(TOP_THREE).map(|e| t.words[e.1 as usize].clone()).collect()
}

/// Paired 95% interval on (a − b) in points, resampling sessions or, with too few, gestures.
fn paired(a: &[bool], b: &[bool], sessions: &[u32]) -> (f64, f64) {
    let distinct: HashSet<u32> = sessions.iter().copied().collect();
    let group: Vec<u32> = if distinct.len() < MINIMUM_SESSIONS { (0..a.len() as u32).collect() } else { sessions.to_vec() };
    let mut map: HashMap<u32, (f64, f64)> = HashMap::new();
    for i in 0..a.len() {
        let e = map.entry(group[i]).or_default();
        e.0 += f64::from(u8::from(a[i])) - f64::from(u8::from(b[i]));
        e.1 += 1.0;
    }
    let groups: Vec<(f64, f64)> = map.into_values().collect();
    let mut rng = Rng(BOOTSTRAP_SEED);
    let mut diffs: Vec<f64> = (0..RESAMPLES).map(|_| {
        let (mut d, mut n) = (0.0, 0.0);
        for _ in 0..groups.len() {
            let e = groups[(rng.next() % groups.len() as u64) as usize];
            d += e.0;
            n += e.1;
        }
        100.0 * d / n
    }).collect();
    diffs.sort_by(f64::total_cmp);
    (diffs[INTERVAL_LOW], diffs[INTERVAL_HIGH])
}

/// The share of even gestures found, at the frequency weight that does best on the odd ones.
fn best_beta(cands: &[Vec<(u32, f32)>], records: &[Record], t: &Trie) -> f32 {
    let odd_hits = |b: f32| cands.iter().zip(records).skip(1).step_by(2).filter(|(c, r)| hit(&ranked(c, t, b), &r.word, 1)).count();
    WEIGHT_STEPS.iter().copied().max_by_key(|&b| odd_hits(b)).unwrap_or(0.0)
}

/// Scores one set from its log-probabilities. `compare` adds geometry and a paired interval.
fn report(e: &Eval, lp: &[Vec<f32>], sh: &Shared, compare: bool) {
    let t = &sh.trie;
    let cands: Vec<Vec<(u32, f32)>> = lp.par_iter().map_init(|| Scratch::new(t), |s, l| search(t, l, BEAM, s)).collect();
    let beta = best_beta(&cands, &e.records, t);
    let tops: Vec<Vec<String>> = cands.iter().map(|c| ranked(c, t, beta)).collect();
    let even: Vec<usize> = (0..e.records.len()).step_by(2).collect();
    let n = even.len() as f64;
    let in_lexicon: Vec<usize> = even.iter().copied().filter(|&i| sh.lexicon.contains(&e.records[i].word)).collect();
    let rate = |k: usize, of: &[usize]| 100.0 * of.iter().filter(|&&i| hit(&tops[i], &e.records[i].word, k)).count() as f64 / of.len().max(1) as f64;
    println!(
        "{}: n {} in-lexicon {:.1}% beta {beta} | neural top1 {:.2} top3 {:.2} (in-lexicon {:.2})",
        e.name, even.len(), 100.0 * in_lexicon.len() as f64 / n, rate(1, &even), rate(TOP_THREE, &even), rate(1, &in_lexicon),
    );
    if !compare {
        return;
    }
    let geo: Vec<Vec<String>> = e.records.par_iter().map(|r| geometry(r, &sh.lexicon, GEOMETRY_TOP)).collect();
    let g1: Vec<bool> = even.iter().map(|&i| hit(&geo[i], &e.records[i].word, 1)).collect();
    let g3 = even.iter().filter(|&&i| hit(&geo[i], &e.records[i].word, TOP_THREE)).count();
    let n1: Vec<bool> = even.iter().map(|&i| hit(&tops[i], &e.records[i].word, 1)).collect();
    let sessions: Vec<u32> = even.iter().map(|&i| e.records[i].session).collect();
    let (lo, hi) = paired(&n1, &g1, &sessions);
    let found = g1.iter().filter(|&&h| h).count();
    println!(
        "   geometry top1 {:.2} top3 {:.2} | neural − geometry 95% [{lo:+.2}, {hi:+.2}] ({} sessions)",
        100.0 * found as f64 / n, 100.0 * g3 as f64 / n, sessions.iter().collect::<HashSet<_>>().len(),
    );
}

fn evaluate(m: &Encoder, sets: &[Eval], sh: &Shared, d: &Device, compare: bool) {
    for e in sets {
        report(e, &forward_all(m, &e.records, d), sh, compare);
    }
}

// ---------- training ----------
fn epoch_order(pool: &[Record], cap: usize, seed: u64) -> Vec<u32> {
    let mut rng = Rng(seed);
    let mut order: Vec<u32> = (0..pool.len() as u32).collect();
    for i in (1..order.len()).rev() {
        order.swap(i, (rng.next() % (i as u64 + 1)) as usize);
    }
    let mut seen: HashMap<&str, usize> = HashMap::new();
    order.retain(|&i| {
        let count = seen.entry(pool[i as usize].word.as_str()).or_default();
        *count += 1;
        *count <= cap
    });
    order
}

struct Options {
    out: PathBuf,
    name: String,
    width: usize,
    epochs: usize,
    cap: usize,
    sources: Vec<String>,
    seed: u64,
    resume: Option<String>,
    lr: f64,
}

type Keep = Box<dyn Fn(&Record) -> bool + Sync>;

/// A name is an exported file, or one of these parts of a file. A name given twice counts twice.
fn train_pool(i: &Inputs, sources: &[String]) -> Vec<Record> {
    let tune = i.tune;
    let mut pool = vec![];
    for name in sources {
        let (file, keep): (&str, Keep) = match name.as_str() {
            "futo-5-rest" => (FUTO_FIVE, Box::new(|r| !(is_azerty(r) && r.session % HELDOUT_EVERY == 0))),
            "futo-5-en" => (FUTO_FIVE, Box::new(|r| !is_azerty(r))),
            "heliboard-half" => (HELIBOARD, Box::new(move |r| Some(r.session) == tune && half_key(r) % 2 == 1)),
            other => (other, Box::new(|_| true)),
        };
        let set = i.set(file);
        let got: Vec<Record> = (0..set.offsets.len()).into_par_iter().filter_map(|j| set.record(j)).filter(|r| keep(r)).collect();
        eprintln!("{name}: {}", got.len());
        pool.extend(got);
    }
    pool
}

fn train(i: &Inputs, o: Options) {
    let out = o.out.join(&o.name);
    std::fs::create_dir_all(&out).unwrap_or_else(|e| fail(&format!("{}: {e}", out.display())));
    let sh = shared(i);
    let pool = Arc::new(train_pool(i, &o.sources));
    let sets = eval_sets(i, EPOCH_EVAL);
    let d = device().autodiff();
    d.seed(o.seed);
    let plain = d.clone().without_autodiff();
    let mut m = Encoder::new(o.width, &d);
    if let Some(path) = &o.resume {
        let r = m.load_from(&mut SafetensorsStore::from_file(path)).expect("the checkpoint loads");
        println!("resumed: {} applied, {} errors", r.applied.len(), r.errors.len());
    }
    println!("{} params {} width {} pool {}", o.name, m.num_params(), o.width, pool.len());
    let mut shadow = m.clone().valid();
    let mut opt = AdamWConfig::new().with_weight_decay(WEIGHT_DECAY)
        .with_grad_clipping(Some(GradientClippingConfig::Norm(GRADIENT_NORM))).init();
    let ctc = CTCLossConfig::new().with_zero_infinity(true).init();
    let per_epoch = epoch_order(&pool, o.cap, o.seed).len();
    let total = o.epochs * per_epoch / BATCH;
    println!("{per_epoch} gestures per epoch after cap {}, {total} steps", o.cap);
    let started = Instant::now();
    let mut step = 0;
    for epoch in 0..o.epochs {
        let order = epoch_order(&pool, o.cap, o.seed ^ epoch as u64);
        let (tx, rx) = std::sync::mpsc::sync_channel::<Batch>(FEED_DEPTH);
        let feeder = pool.clone();
        let seed = o.seed.wrapping_mul(FEED_SEED_MULTIPLIER).wrapping_add(epoch as u64);
        std::thread::spawn(move || {
            for (index, chunk) in order.as_chunks::<BATCH>().0.iter().enumerate() {
                let records: Vec<Record> = chunk.iter().map(|&j| feeder[j as usize].clone()).collect();
                if tx.send(batch(&records, Some(seed ^ (index as u64) << FEED_SEED_SHIFT))).is_err() {
                    return;
                }
            }
        });
        let mut smooth = 0.0f32;
        for b in rx {
            let input = inputs(&b, &d);
            let y = Tensor::<2, Int>::from_data(TensorData::new(b.y.clone(), [b.n, b.max_l]), &d);
            let il = Tensor::<1, Int>::from_data(TensorData::new(vec![FRAMES as i32; b.n], [b.n]), &d);
            let tl = Tensor::<1, Int>::from_data(TensorData::new(b.lens.clone(), [b.n]), &d);
            let loss = ctc.forward(m.log_probs(input.x, input.keys, input.mask), y, il, tl).mean();
            let v: f32 = loss.clone().into_scalar();
            assert!(v.is_finite(), "non-finite loss at step {step}: {v}");
            smooth = if step == 0 { v } else { LOSS_MEMORY * smooth + (1.0 - LOSS_MEMORY) * v };
            let g = GradientsParams::from_grads(loss.backward(), &m);
            let progress = (step as f64 / total.max(1) as f64).min(1.0);
            let warm = ((step + 1) as f64 / WARMUP_STEPS as f64).min(1.0);
            let lr = warm * (MIN_LR + 0.5 * (o.lr - MIN_LR) * (1.0 + (std::f64::consts::PI * progress).cos()));
            m = opt.step(lr, m, g);
            step += 1;
            if step % EMA_EVERY == 0 {
                shadow = ema(shadow, &m.clone().valid(), EMA_DECAY);
            }
            if step % LOG_EVERY == 0 {
                println!("step {step}/{total} loss {smooth:.3} lr {lr:.5} {:.0} gestures/s", (step * BATCH) as f64 / started.elapsed().as_secs_f64());
            }
        }
        println!("--- epoch {} at {:.0}s", epoch + 1, started.elapsed().as_secs_f64());
        shadow.save_into(&mut SafetensorsStore::from_file(out.join("ema.safetensors")).overwrite(true)).expect("the checkpoint is written");
        evaluate(&shadow, &sets, &sh, &plain, false);
    }
    println!("=== final, full evaluation");
    evaluate(&shadow, &eval_sets(i, FULL_EVAL), &sh, &plain, true);
}

fn checkpoint(path: &str, width: usize, d: &Device) -> Encoder {
    let mut m = Encoder::new(width, d);
    let r = m.load_from(&mut SafetensorsStore::from_file(path)).expect("the checkpoint loads");
    println!("loaded {} tensors, {} errors", r.applied.len(), r.errors.len());
    m
}

fn bytes(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

fn write(path: String, contents: Vec<u8>) {
    std::fs::write(&path, contents).unwrap_or_else(|e| fail(&format!("{path}: {e}")));
}

/// Gestures as the graph reads them: [n, FEATURES, FRAMES].
fn graph_input(records: &[Record]) -> Vec<f32> {
    batch(records, None).x
}

/// Writes the float graph for one gesture, the calibration gestures and the evaluation gestures.
fn export(i: &Inputs, path: &str, width: usize, out: &str) {
    let d = Device::flex();
    let m = checkpoint(path, width, &d);
    let deploy: Deploy = m.deploy(&d);
    let sets = eval_sets(i, FULL_EVAL);

    let dev = i.set(FUTO_DEV);
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut calibration: Vec<Record> = vec![];
    for e in &sets {
        calibration.extend(e.records.iter().skip(1).step_by(2).take(CALIBRATION_PER_SET).cloned());
    }
    for index in (1..dev.offsets.len()).step_by(CALIBRATION_STRIDE) {
        if calibration.len() >= CALIBRATION_TOTAL {
            break;
        }
        let Some(r) = dev.record(index) else { continue };
        let count = seen.entry(r.word.clone()).or_default();
        *count += 1;
        if *count <= CALIBRATION_PER_WORD {
            calibration.push(r);
        }
    }
    write(format!("{out}/calibration.bin"), bytes(&graph_input(&calibration)));
    println!("calibration gestures {}", calibration.len());
    for (index, e) in sets.iter().enumerate() {
        write(format!("{out}/eval{index}.bin"), bytes(&graph_input(&e.records)));
    }

    let first = batch(&sets[0].records[..1], None);
    let input = inputs(&first, &d);
    let graph = deploy.forward(input.x.clone());
    let full = m.logits(input.x.clone(), input.keys.clone(), input.mask.clone());
    let via = m.scores(graph, input.keys, input.mask);
    let gap: f32 = (full - via).abs().max().into_scalar();
    println!("deploy graph against training graph, max gap {gap:.2e}");
    let onnx = OnnxExporter::new().export(&deploy, input.x, Deploy::forward).expect("the graph exports");
    onnx.save(format!("{out}/encoder.float.onnx")).expect("the graph is written");
    println!("exported");
}

/// Scores the outputs another runtime wrote, one file per evaluation set.
fn evalq(i: &Inputs, path: &str, width: usize, outputs: &str) {
    let d = Device::flex();
    let m = checkpoint(path, width, &d);
    let sh = shared(i);
    let row = synthswipe::OUTPUT_VALUES;
    for (index, e) in eval_sets(i, FULL_EVAL).iter().enumerate() {
        let file = format!("{outputs}/out{index}.bin");
        let raw = std::fs::read(&file).unwrap_or_else(|err| fail(&format!("{file}: {err}")));
        let raw: Vec<f32> = raw.as_chunks().0.iter().copied().map(f32::from_le_bytes).collect();
        let mut lp: Vec<Vec<f32>> = vec![];
        for (records, out) in e.records.chunks(EVAL_BATCH).zip(raw.chunks(row * EVAL_BATCH)) {
            let b = batch(records, None);
            let input = inputs(&b, &d);
            let o = Tensor::<3>::from_data(TensorData::new(out.to_vec(), [b.n, row / FRAMES, FRAMES]), &d);
            let scores = m.scores(o, input.keys, input.mask).permute([2, 0, 1]);
            split_gestures(&values(burn::tensor::activation::log_softmax(scores, 2)), b.n, &mut lp);
        }
        report(e, &lp, &sh, false);
    }
}

fn head(path: &str, width: usize, out: &str) {
    let m = checkpoint(path, width, &Device::flex());
    let v = m.head_values();
    println!("head values {}", v.len());
    write(format!("{out}/head.bin"), bytes(&v));
}

/// `--key value` pairs, around positional arguments.
struct Args(Vec<String>);

impl Args {
    fn flag(&self, key: &str) -> Option<&str> {
        let at = self.0.iter().position(|a| a == key)?;
        self.0.get(at + 1).map(String::as_str)
    }
    fn need(&self, key: &str) -> &str {
        self.flag(key).unwrap_or_else(|| fail(&format!("{key} is required")))
    }
    fn get<T: FromStr>(&self, key: &str, default: T) -> T {
        self.flag(key).map_or(default, |v| v.parse().unwrap_or_else(|_| fail(&format!("{key}: cannot read {v}"))))
    }
    fn at(&self, index: usize) -> &str {
        self.0.get(index).filter(|a| !a.starts_with("--")).map_or_else(|| fail("a path is missing"), String::as_str)
    }
}

fn main() {
    let args = Args(std::env::args().skip(1).collect());
    let width = args.get("--width", DEFAULT_WIDTH);
    match args.0.first().map(String::as_str) {
        Some("train") => {
            let options = Options {
                out: PathBuf::from(args.need("--out")),
                name: args.get("--name", "run".to_owned()),
                width,
                epochs: args.get("--epochs", 1),
                cap: args.get("--cap", DEFAULT_CAP),
                seed: args.get("--seed", 1),
                sources: args.flag("--sources").unwrap_or(DEFAULT_SOURCES).split(',').map(str::to_owned).collect(),
                resume: args.flag("--resume").map(str::to_owned),
                lr: args.get("--lr", BASE_LR),
            };
            train(&Inputs::new(&args), options);
        }
        Some("eval") => {
            let i = Inputs::new(&args);
            let d = device();
            let m = checkpoint(args.at(1), width, &d);
            evaluate(&m, &eval_sets(&i, args.get("--limit", FULL_EVAL)), &shared(&i), &d, true);
        }
        Some("head") => head(args.at(1), width, args.at(2)),
        Some("export") => export(&Inputs::new(&args), args.at(1), width, args.at(2)),
        Some("evalq") => evalq(&Inputs::new(&args), args.at(1), width, args.at(2)),
        #[cfg(feature = "onnx")]
        Some("prod") => shipped::prod(&Inputs::new(&args), args.at(1)),
        #[cfg(feature = "onnx")]
        Some("hostile") => shipped::hostile(args.at(1)),
        _ => fail("unknown command"),
    }
}
