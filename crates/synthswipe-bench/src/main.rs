//! Benchmark harness. It uses only the public `synthswipe` API, so it stays
//! the first real consumer of the library.
//!
//! Usage: `synthswipe-bench [data/raw]`. Writes a Markdown report to stdout and
//! progress to stderr.

#![allow(clippy::cast_precision_loss)]

mod load;

use std::collections::{BTreeMap, HashMap};
use std::error::Error;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use rayon::prelude::*;
use synthswipe::{Affinities, Grammar, Params, decode_in_context, fold};

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
        strict1: ranked.first().is_some_and(|c| c.surface == gesture.word),
        strict3: ranked.iter().any(|c| c.surface == gesture.word),
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

/// Replays the user's own gestures. The truth is the word they kept: the one
/// they picked from the bar, or the decoded word if they did not undo it.
fn run_personal(text: &str, params: &Params, gestures: &[Gesture]) {
    let Some(lexicon) = gestures.iter().find(|g| g.cell.language == "fr").map(|g| g.lexicon.clone()) else {
        return;
    };
    let mut layout = None;
    let lines: Vec<&str> = text.lines().collect();
    let (mut total, mut hits) = (0, 0);
    let mut misses: BTreeMap<String, usize> = BTreeMap::new();
    for (index, line) in lines.iter().enumerate() {
        let fields: Vec<&str> = line.split('\t').collect();
        match fields.as_slice() {
            ["K", keys] => {
                let parsed = keys.split(' ').filter_map(|key| {
                    let mut parts = key.split(':');
                    let label = parts.next()?.chars().next()?;
                    let x = parts.next()?.parse().ok()?;
                    let y = parts.next()?.parse().ok()?;
                    (label.is_alphabetic() || label == '\'').then_some(synthswipe::Key { label, x, y })
                });
                if let Ok(valid) = synthswipe::Layout::new(parsed) {
                    layout = Some(valid);
                }
            }
            ["G", word, _, points, ..] => {
                let later = lines.iter().skip(index + 1).take_while(|line| !line.starts_with('G'));
                let mut rejected = false;
                let mut truth = word.to_lowercase();
                for line in later {
                    match line.split('\t').collect::<Vec<_>>().as_slice() {
                        ["C", chosen] => truth = (*chosen).to_lowercase(),
                        ["D"] => rejected = true,
                        ["F", meant] => {
                            truth = (*meant).to_lowercase();
                            rejected = false;
                        }
                        _ => {}
                    }
                }
                let samples: Vec<synthswipe::Sample> = points
                    .split(' ')
                    .filter_map(|point| {
                        let mut parts = point.split(',');
                        Some(synthswipe::Sample {
                            x: parts.next()?.parse().ok()?,
                            y: parts.next()?.parse().ok()?,
                            t_ms: parts.next()?.parse().ok()?,
                        })
                    })
                    .collect();
                let Some(layout) = &layout else { continue };
                let ranked = decode_in_context(layout, &lexicon, &samples, params, 3, |_| 0.0).unwrap_or_default();
                if rejected {
                    continue;
                }
                total += 1;
                if ranked.first().is_some_and(|c| c.surface == truth) {
                    hits += 1;
                } else {
                    let got = ranked.first().map_or("", |c| c.surface);
                    *misses.entry(format!("{truth}>{got}")).or_default() += 1;
                }
            }
            _ => {}
        }
    }
    let mut worst: Vec<_> = misses.into_iter().collect();
    worst.sort_by_key(|entry| std::cmp::Reverse(entry.1));
    worst.truncate(10);
    println!("{hits}/{total} of your gestures read right, without context; still wrong: {worst:?}");
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
            "--no-futo" => skip_futo = true,
            "--intent" => params.intent_weight = arguments.next().ok_or("--intent needs a value")?.parse()?,
            "--personal" => personal = true,
            "--turns" => params.turn_weight = arguments.next().ok_or("--turns needs a value")?.parse()?,
            "--start" => params.start_weight = arguments.next().ok_or("--start needs a value")?.parse()?,
            "--grammar" => {
                let weight: f32 = arguments.next().ok_or("--grammar needs a value")?.parse()?;
                let text = std::fs::read_to_string("android/app/src/main/assets/forms.tsv")?;
                GRAMMAR.set((Grammar::parse(&text), weight)).map_err(|_| "grammar set twice")?;
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

    if personal {
        let text = std::fs::read_to_string(root.join("personal/traces.tsv"))?;
        run_personal(&text, &params, &gestures);
        return Ok(());
    }
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
