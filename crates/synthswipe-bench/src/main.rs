//! Benchmark harness. It uses only the public `synthswipe` API, so it stays
//! the first real consumer of the library.
//!
//! Usage: `synthswipe-bench [data/raw]`. Writes a Markdown report to stdout and
//! progress to stderr. `--personal` replays the owner's traces and prints counts;
//! `--show-words` adds the words they got wrong, for the owner's terminal alone.

#![allow(clippy::cast_precision_loss)]

mod export;
mod load;

use std::collections::{BTreeMap, HashMap};
use std::error::Error;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use rayon::prelude::*;
use synthboard_core::{Decoding, Language};
use synthswipe::{Affinities, Grammar, Layout, Params, decode_in_context, fits_gesture, fold};
use synthswipe_bench::revisions;
use synthswipe_bench::traces::{Fate, Reader, Traced, TracedKey, layout_of, parse};

use load::{Cell, FutoFilter, Gesture, Lexicons};

const ENGLISH_FILES: usize = 3;
const FUTO_ENGLISH_TEST_ROWS: usize = 30_000;
const FUTO_ENGLISH_TUNING_ROWS: usize = 4_000;
const TUNING_GESTURES_PER_CELL: usize = 1_000;
const MIN_GESTURES_TO_SLICE: usize = 300;

type Setter = fn(&mut Params, f32);

#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Default, Clone, Copy)]
struct Outcome {
    in_lexicon: bool,
    strict1: bool,
    strict3: bool,
    folded1: bool,
    micros: u64,
}

static GRAMMAR: OnceLock<(Grammar, f32)> = OnceLock::new();

/// Rivals tried for the gesture check: common words of some length, a few per correction.
const COMMON_WEIGHT: f32 = 100.0;
const MIN_RIVAL_KEYS: usize = 4;
const RIVALS_PER_CORRECTION: usize = 10;
/// Confusions shown with `--show-words`.
const SHOWN_MISSES: usize = 10;
/// The keyboard's assets, from the data directory.
const ASSETS: &str = "../../android/app/src/main/assets";

fn grammar_score(history: &[String], word: &str) -> f32 {
    GRAMMAR.get().map_or(0.0, |(grammar, weight)| weight * grammar.score(history, word))
}

fn run(gesture: &Gesture, params: &Params) -> Outcome {
    let started = Instant::now();
    let context = |word: &str| {
        let table = match (&gesture.affinities, &gesture.previous) {
            (Some(affinities), Some(previous)) => affinities.score(previous, word),
            _ => 0.0,
        };
        table + if gesture.cell.language == "fr" { grammar_score(&gesture.history, word) } else { 0.0 }
    };
    let ranked = decode_in_context(
        &gesture.layout,
        &gesture.lexicon,
        &gesture.samples,
        params,
        3,
        context,
    );
    let micros = u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX);
    let ranked = ranked.unwrap_or_default();
    let keys = fold(&gesture.word);
    Outcome {
        in_lexicon: gesture.lexicon.contains(&gesture.word),
        strict1: ranked.first().is_some_and(|c| c.surface.to_lowercase() == gesture.word),
        strict3: ranked.iter().any(|c| c.surface.to_lowercase() == gesture.word),
        folded1: ranked.first().is_some_and(|c| fold(c.surface) == keys),
        micros,
    }
}

/// The mean of each cell's top-1, so that no layout outweighs another.
fn balanced_top1(cells: &BTreeMap<Cell, Vec<&Gesture>>, params: &Params) -> f64 {
    let scores: Vec<f64> = cells
        .values()
        .map(|gestures| {
            let hits = gestures
                .par_iter()
                .filter(|gesture| run(gesture, params).strict1)
                .count();
            hits as f64 / gestures.len().max(1) as f64
        })
        .collect();
    scores.iter().sum::<f64>() / scores.len().max(1) as f64
}

fn tune(gestures: &[Gesture], start: Params) -> Params {
    let mut cells: BTreeMap<Cell, Vec<&Gesture>> = BTreeMap::new();
    for gesture in gestures.iter().filter(|gesture| gesture.tuning) {
        let cell = cells.entry(gesture.cell.clone()).or_default();
        if cell.len() < TUNING_GESTURES_PER_CELL {
            cell.push(gesture);
        }
    }
    cells.retain(|_, gestures| gestures.len() >= 50);

    let mut best = start;
    let mut best_score = balanced_top1(&cells, &best);
    let grids: [(&str, &[f32], Setter); 4] = [
        ("alignment", &[0.0, 0.25, 0.5, 1.0, 2.0, 4.0, 8.0, 16.0], |p, v| p.alignment_weight = v),
        ("lambda", &[0.0, 0.001, 0.002, 0.004, 0.007, 0.01, 0.02, 0.04, 0.08], |p, v| p.lambda = v),
        ("sigma_shape", &[0.05, 0.08, 0.11, 0.15, 0.2, 0.3, 0.5], |p, v| p.sigma_shape = v),
        ("sigma_location", &[0.3, 0.4, 0.5, 0.7, 1.0, 1.5, 2.5], |p, v| p.sigma_location = v),
    ];
    for round in 0..3 {
        for (name, values, set) in &grids {
            for &value in *values {
                let mut trial = best;
                set(&mut trial, value);
                let score = balanced_top1(&cells, &trial);
                if score > best_score {
                    best_score = score;
                    best = trial;
                }
            }
            eprintln!("round {round} {name}: {:.2}% {best:?}", best_score * 100.0);
        }
    }
    best
}

/// Replays real gestures of `words` as if typed after `previous`, and shows
/// what wins, to explain a confusion rather than average it away.
fn run_probe(gestures: &[Gesture], params: &Params, previous: &str, words: &str, root: &std::path::Path) {
    let table = std::fs::read(root.join("ngrams/affinity.bin")).unwrap_or_default();
    let table = Affinities::from_bytes(&table);
    for word in words.split(',') {
        let members: Vec<&Gesture> = gestures.iter().filter(|g| g.word == word).collect();
        let mut wins: BTreeMap<String, usize> = BTreeMap::new();
        for gesture in &members {
            let history = vec![previous.to_owned()];
            let context = |candidate: &str| table.score(previous, candidate) + grammar_score(&history, candidate);
            let ranked = decode_in_context(&gesture.layout, &gesture.lexicon, &gesture.samples, params, 3, context)
                .unwrap_or_default();
            if let Some(best) = ranked.first() {
                *wins.entry(best.surface.to_owned()).or_default() += 1;
            }
        }
        let mut sorted: Vec<_> = wins.into_iter().collect();
        sorted.sort_by_key(|entry| std::cmp::Reverse(entry.1));
        sorted.truncate(5);
        println!("{word} ({} gestures) after {previous}: {sorted:?}", members.len());
    }
    for word in words.split(',') {
        println!("{previous} -> {word}: affinity {:.2}", table.score(previous, word));
    }
}

/// The keyboard's own assets, as the phone loads them.
struct Assets {
    lexicon_text: String,
    language: Language,
}

fn deployed(root: &Path, neural: Option<Arc<synthswipe::Neural>>) -> Result<Assets, Box<dyn Error + Send + Sync>> {
    let assets = root.join(ASSETS);
    let lexicon_text = std::fs::read_to_string(assets.join("lexicon.csv"))?;
    let lexicon = Arc::new(synthswipe::Lexicon::from_csv(&lexicon_text));
    let affinities = Some(Arc::new(Affinities::from_bytes(&std::fs::read(assets.join("affinity.bin"))?)));
    let grammar = Some(Arc::new(read_grammar(&assets)?));
    Ok(Assets { lexicon_text, language: Language { lexicon, affinities, grammar, neural } })
}

/// The rules, then the forms, read as one text as on the phone.
fn read_grammar(assets: &Path) -> Result<Grammar, Box<dyn Error + Send + Sync>> {
    let rules = std::fs::read_to_string(assets.join("grammar.tsv"))?;
    let forms = std::fs::read_to_string(assets.join("forms.tsv"))?;
    Ok(Grammar::parse(&format!("{rules}{forms}")))
}

/// Common words by first and last key: what a wrong word could look like to the
/// gesture check, against the truly meant one.
fn rival_words(lexicon_text: &str) -> HashMap<(char, char), Vec<String>> {
    let mut rivals: HashMap<(char, char), Vec<String>> = HashMap::new();
    for line in lexicon_text.lines() {
        let mut fields = line.rsplitn(3, ',');
        let (Some(_), Some(weight), Some(word)) = (fields.next(), fields.next(), fields.next()) else { continue };
        let keys = fold(word);
        if let (Some(&first), Some(&last), Ok(weight)) = (keys.first(), keys.last(), weight.parse::<f32>())
            && weight >= COMMON_WEIGHT
            && keys.len() >= MIN_RIVAL_KEYS
        {
            rivals.entry((first, last)).or_default().push(word.to_lowercase());
        }
    }
    rivals
}

/// Counts of the replay. Only numbers: this may be shown beyond the volume.
#[derive(Debug, Default)]
struct Replay {
    total: usize,
    right: usize,
    network_right: usize,
    corrections: usize,
    corrections_fit: usize,
    rivals: usize,
    rivals_fit: usize,
    recorded: usize,
    differed: usize,
    written_right: usize,
    other_right: usize,
    by_network: usize,
    by_geometry: usize,
}

impl Replay {
    fn check_correction(&mut self, gesture: &Traced, truth: &str, layout: &Layout, rivals: &HashMap<(char, char), Vec<String>>, params: &Params) {
        self.corrections += 1;
        self.corrections_fit += usize::from(fits_gesture(layout, truth, &gesture.samples, params));
        let keys = fold(truth);
        if let (Some(first), Some(last)) = (keys.first(), keys.last()) {
            let same_ends = rivals.get(&(*first, *last)).map_or(&[][..], Vec::as_slice);
            for rival in same_ends.iter().filter(|rival| **rival != truth).take(RIVALS_PER_CORRECTION) {
                self.rivals += 1;
                self.rivals_fit += usize::from(fits_gesture(layout, rival, &gesture.samples, params));
            }
        }
    }

    fn count_other(&mut self, gesture: &Traced, truth: &str) {
        match gesture.reader {
            Some(Reader::Network) => self.by_network += 1,
            Some(Reader::Geometry) => self.by_geometry += 1,
            None => {}
        }
        let (word, other) = (gesture.word.to_lowercase(), gesture.other.to_lowercase());
        if !other.is_empty() {
            self.recorded += 1;
            if other != word {
                self.differed += 1;
                self.written_right += usize::from(word == truth);
                self.other_right += usize::from(other == truth);
            }
        }
    }
}

/// Replays the owner's gestures through the decoder that ships, with its lexicon,
/// context, grammar and language, in one code path with the keyboard. Only memory
/// stays out: it never leaves the phone. The truth is the word the owner kept.
fn run_personal(text: &str, params: &Params, neural: Option<Arc<synthswipe::Neural>>, root: &Path, show_words: bool) -> Result<(), Box<dyn Error + Send + Sync>> {
    let assets = deployed(root, neural)?;
    let geometry = Language { neural: None, ..assets.language.clone() };
    let rivals = rival_words(&assets.lexicon_text);
    let mut replay = Replay::default();
    let mut layout: Option<(Arc<Vec<TracedKey>>, Arc<Layout>)> = None;
    let mut misses: BTreeMap<String, usize> = BTreeMap::new();
    let gestures = parse(text);
    let rephrased = gestures.iter().filter(|gesture| gesture.fate == Fate::Rephrased).count();
    for gesture in &gestures {
        let Some(truth) = gesture.truth.as_deref().map(str::to_lowercase) else { continue };
        if !layout.as_ref().is_some_and(|(keys, _)| Arc::ptr_eq(keys, &gesture.keys)) {
            layout = layout_of(&gesture.keys).ok().map(|read| (Arc::clone(&gesture.keys), Arc::new(read)));
        }
        let Some((_, keyboard)) = &layout else { continue };
        replay.total += 1;
        if truth != gesture.word.to_lowercase() {
            replay.check_correction(gesture, &truth, keyboard, &rivals, params);
        }
        replay.count_other(gesture, &truth);
        let read = |language: &Language| {
            Decoding::replaying(language, Arc::clone(keyboard), *params, &gesture.history).rank(&gesture.samples).into_iter().next().unwrap_or_default()
        };
        let got = read(&geometry);
        if got.to_lowercase() == truth {
            replay.right += 1;
        } else {
            *misses.entry(format!("{truth}>{got}")).or_default() += 1;
        }
        if assets.language.neural.is_some() {
            replay.network_right += usize::from(read(&assets.language).to_lowercase() == truth);
        }
    }
    let Replay { total, right, network_right, corrections, corrections_fit, rivals, rivals_fit, recorded, differed, written_right, other_right, by_network, by_geometry } = replay;
    println!("corrections that fit {corrections_fit}/{corrections}; rivals with the same ends that fit {rivals_fit}/{rivals}");
    println!("written by the network {by_network}, by the geometry {by_geometry}; the other reader recorded on {recorded}, and differed on {differed}: the written word was right {written_right}, the other {other_right}");
    println!("changes of mind, left out of everything here: {rephrased}");
    let (pairs, tallies) = revisions::replay(&gestures, &assets.language, params);
    println!("next word, over {pairs} pairs of gestures:");
    for (margin, revisions::Tally { help, hurt, nothing }) in tallies {
        println!("  margin {margin:.2}: helps {help}, hurts {hurt}, changes nothing {nothing}");
    }
    if show_words {
        let mut worst: Vec<_> = misses.into_iter().collect();
        worst.sort_by_key(|entry| std::cmp::Reverse(entry.1));
        worst.truncate(SHOWN_MISSES);
        println!("still wrong: {worst:?}");
    }
    if assets.language.neural.is_some() {
        println!("{network_right}/{total} with the network");
    }
    println!("{right}/{total} of your gestures read right by the shipped decoder");
    Ok(())
}

fn percent(hits: usize, total: usize) -> f64 {
    100.0 * hits as f64 / total.max(1) as f64
}

fn margin(hits: usize, total: usize) -> f64 {
    let rate = hits as f64 / total.max(1) as f64;
    196.0 * (rate * (1.0 - rate) / total.max(1) as f64).sqrt()
}

fn report(gestures: &[Gesture], params: &Params) {
    let mut cells: BTreeMap<Cell, Vec<&Gesture>> = BTreeMap::new();
    for gesture in gestures.iter().filter(|gesture| !gesture.tuning) {
        cells.entry(gesture.cell.clone()).or_default().push(gesture);
    }
    println!(
        "| Set | Slice | n | In lexicon | Top-1 | ±95% | Top-3 | Top-1 folded | Closed top-1 | Closed top-3 | p95 ms |"
    );
    println!("|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|");
    for (cell, members) in &cells {
        let set = format!("{} {} {}", cell.source, cell.language, cell.layout);
        row(&set, "all", members, params);
        if members.len() < MIN_GESTURES_TO_SLICE {
            continue;
        }
        for (slice, belongs) in SLICES {
            let subset: Vec<&Gesture> = members.iter().copied().filter(|g| belongs(&g.word)).collect();
            if !subset.is_empty() {
                row(&set, slice, &subset, params);
            }
        }
    }
}

fn row(set: &str, slice: &str, members: &[&Gesture], params: &Params) {
    let outcomes: Vec<Outcome> = members.par_iter().map(|g| run(g, params)).collect();
    let total = outcomes.len();
    let count = |test: fn(&Outcome) -> bool| outcomes.iter().filter(|o| test(o)).count();
    let closed: Vec<_> = members.iter().filter_map(|g| g.closed).collect();
    let closed_rate = |test: fn(&load::ClosedLibrary) -> bool| {
        if closed.is_empty() {
            "n/a".to_owned()
        } else {
            format!(
                "{:.2}",
                percent(closed.iter().filter(|c| test(c)).count(), closed.len())
            )
        }
    };
    let mut timings: Vec<u64> = outcomes.iter().map(|o| o.micros).collect();
    timings.sort_unstable();
    let p95 = timings.get(total * 95 / 100).copied().unwrap_or(0);
    let top1 = count(|o| o.strict1);
    println!(
        "| {set} | {slice} | {total} | {:.1}% | {:.2} | {:.2} | {:.2} | {:.2} | {} | {} | {:.1} |",
        percent(count(|o| o.in_lexicon), total),
        percent(top1, total),
        margin(top1, total),
        percent(count(|o| o.strict3), total),
        percent(count(|o| o.folded1), total),
        closed_rate(|c| c.top1),
        closed_rate(|c| c.top3),
        p95 as f64 / 1000.0,
    );
}

fn has_apostrophe(word: &str) -> bool {
    word.contains(['\'', '’'])
}

fn has_accent(word: &str) -> bool {
    word.chars().any(|c| c.is_alphabetic() && !c.is_ascii())
}

fn letters(word: &str) -> usize {
    word.chars().filter(|c| c.is_alphabetic()).count()
}

type Slice = (&'static str, fn(&str) -> bool);

const SLICES: [Slice; 5] = [
    ("accent", has_accent),
    ("apostrophe", has_apostrophe),
    ("1-4 letters", |word| letters(word) <= 4),
    ("5-8 letters", |word| (5..=8).contains(&letters(word))),
    ("9+ letters", |word| letters(word) >= 9),
];

#[allow(clippy::too_many_lines)]
fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let mut root = PathBuf::from("data/raw");
    let mut should_tune = false;
    let mut skip_futo = false;
    let mut personal = false;
    let mut show_words = false;
    let mut export_directory = None;
    let mut neural_file: Option<PathBuf> = None;
    let mut spoken = false;
    let mut units = load::Units::default();
    let mut probe: Option<(String, String)> = None;
    let mut min_weight = 0.0_f32;
    let mut per_cell = usize::MAX;
    let mut params = Params::default();
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--tune" => should_tune = true,
            "--export" => export_directory = Some(PathBuf::from(arguments.next().ok_or("--export needs a directory")?)),
            "--neural" => neural_file = Some(PathBuf::from(arguments.next().ok_or("--neural needs a file")?)),
            "--no-futo" => skip_futo = true,
            "--intent" => params.intent_weight = arguments.next().ok_or("--intent needs a value")?.parse()?,
            "--personal" => personal = true,
            "--show-words" => show_words = true,
            "--turns" => params.turn_weight = arguments.next().ok_or("--turns needs a value")?.parse()?,
            "--start" => params.start_weight = arguments.next().ok_or("--start needs a value")?.parse()?,
            "--grammar" => {
                let weight: f32 = arguments.next().ok_or("--grammar needs a value")?.parse()?;
                let grammar = read_grammar(Path::new("android/app/src/main/assets"))?;
                GRAMMAR.set((grammar, weight)).map_err(|_| "grammar set twice")?;
            }
            "--spoken" => spoken = true,
            "--key-units" => units.key_units = true,
            "--offset" => units.offset = arguments.next().ok_or("--offset needs a value")?.parse()?,
            "--probe" => probe = Some((
                arguments.next().ok_or("--probe needs a previous word")?,
                arguments.next().ok_or("--probe needs words")?,
            )),
            "--marks" => params.mark_weight = arguments.next().ok_or("--marks needs a value")?.parse()?,
            "--context" => {
                params.context_weight = arguments.next().ok_or("--context needs a value")?.parse()?;
            }
            "--min-weight" => min_weight = arguments.next().ok_or("--min-weight needs a value")?.parse()?,
            "--per-cell" => per_cell = arguments.next().ok_or("--per-cell needs a value")?.parse()?,
            "--lambda" => params.lambda = arguments.next().ok_or("--lambda needs a value")?.parse()?,
            "--pass-radius" => {
                params.pass_radius = arguments.next().ok_or("--pass-radius needs a value")?.parse()?;
            }
            _ => root = PathBuf::from(argument),
        }
    }
    if personal {
        let text = std::fs::read_to_string(root.join("personal/traces.tsv"))?;
        #[cfg(feature = "onnx")]
        let model = neural_file.as_ref().map(std::fs::read).transpose()?.and_then(|bytes| synthswipe_onnx::load(&bytes)).map(Arc::new);
        #[cfg(not(feature = "onnx"))]
        let model = neural_file.as_ref().and(None);
        return run_personal(&text, &params, model, &root, show_words);
    }
    let heliboard = root.join("heliboard-gesture-data");
    let futo = root.join("swipe.futo.org");
    let counts = if spoken {
        let mut merged: HashMap<String, u32> = HashMap::new();
        for name in ["ngrams/fr.txt", "ngrams/en.txt"] {
            for line in std::fs::read_to_string(root.join(name))?.lines() {
                let mut fields = line.split(' ');
                if let (Some(word), Some(count), None) = (fields.next(), fields.next(), fields.next()) {
                    let count: u32 = count.parse().unwrap_or(0);
                    let best = merged.entry(word.to_owned()).or_default();
                    *best = (*best).max(count);
                }
            }
        }
        Some(merged)
    } else {
        None
    };
    let lexicons = Lexicons::new(root.join("heliboard-word-list-csvs/compressed/unsorted"), min_weight, counts, units);

    let started = Instant::now();
    let french = lexicons.get(&load::main_dictionary(&heliboard, "fr")?)?;
    let english = lexicons.get(&load::main_dictionary(&heliboard, "en-US")?)?;
    eprintln!(
        "lexicons: fr {} words, en {} words, built in {} ms",
        french.len(),
        english.len(),
        started.elapsed().as_millis()
    );

    if let Some(directory) = &export_directory {
        std::fs::create_dir_all(directory)?;
        let layouts_directory = futo.join("swipe-5/layouts");
        let filter = FutoFilter {
            layouts_directory: &layouts_directory,
            french: french.clone(),
            english: english.clone(),
            french_affinities: None,
            english_affinities: None,
            tuning_only: false,
            max_rows: usize::MAX,
            units,
        };
        let sources = [
            ("futo-train", "train.jsonl"),
            ("futo-dev", "dev.jsonl"),
            ("futo-test", "test.jsonl"),
            ("futo-2", "swipe-2/swipe2.jsonl"),
            ("futo-3", "swipe-3/swipe3.jsonl"),
            ("futo-4", "swipe-4/swipe4.jsonl"),
            ("futo-5", "swipe-5/swipe5.jsonl"),
        ];
        for (name, file) in sources {
            let mut writer = export::Writer::create(directory, name)?;
            load::futo_each(&futo.join(file), &filter, |gesture, session| writer.write(&gesture, session))?;
            eprintln!("{name}: {} gestures", writer.finish()?);
        }
        let mut writer = export::Writer::create(directory, "heliboard-fr")?;
        for gesture in load::heliboard(&heliboard, "fr", "fr", usize::MAX, &lexicons)? {
            let group = if gesture.tuning { "tune" } else { "test" };
            writer.write(&gesture, group)?;
        }
        eprintln!("heliboard-fr: {} gestures", writer.finish()?);
        return Ok(());
    }
    let mut gestures = load::heliboard(&heliboard, "fr", "fr", usize::MAX, &lexicons)?;
    gestures.extend(load::heliboard(&heliboard, "en-US", "en", ENGLISH_FILES, &lexicons)?);
    if !skip_futo {
        let model = |name: &str| -> Result<Option<Arc<Affinities>>, Box<dyn Error + Send + Sync>> {
            if params.context_weight == 0.0 {
                return Ok(None);
            }
            Ok(Some(Arc::new(Affinities::from_bytes(&std::fs::read(root.join("ngrams").join(name))?))))
        };
        let (french_affinities, english_affinities) = (model("affinity.bin")?, model("affinity.bin")?);
        let layouts_directory = futo.join("swipe-5/layouts");
        let filter = |tuning_only, max_rows| FutoFilter {
            layouts_directory: &layouts_directory,
            french: french.clone(),
            english: english.clone(),
            french_affinities: french_affinities.clone(),
            english_affinities: english_affinities.clone(),
            tuning_only,
            max_rows,
            units,
        };
        gestures.extend(load::futo(&futo.join("swipe-5/swipe5.jsonl"), &filter(false, usize::MAX))?);
        gestures.extend(load::futo(&futo.join("test.jsonl"), &filter(false, FUTO_ENGLISH_TEST_ROWS))?);
        gestures.extend(load::futo(&futo.join("dev.jsonl"), &filter(true, FUTO_ENGLISH_TUNING_ROWS))?);
    }
    let mut seen: BTreeMap<Cell, usize> = BTreeMap::new();
    gestures.retain(|gesture| {
        let count = seen.entry(gesture.cell.clone()).or_default();
        *count += 1;
        gesture.tuning || *count <= per_cell
    });
    eprintln!("gestures: {}", gestures.len());

    if let Some((previous, words)) = probe {
        run_probe(&gestures, &params, &previous, &words, &root);
        return Ok(());
    }
    if should_tune {
        params = tune(&gestures, params);
    }
    println!("Parameters: `{params:?}`\n");
    report(&gestures, &params);
    if let Ok(status) = std::fs::read_to_string("/proc/self/status") {
        for line in status.lines().filter(|line| line.starts_with("VmHWM")) {
            eprintln!("{line}");
        }
    }
    Ok(())
}
