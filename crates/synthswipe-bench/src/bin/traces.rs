//! The owner's traces, once opened. Plain traces only ever exist on the volume.
//!
//! `traces ingest ARCHIVE RECEIVED...` adds received files to the archive.
//! `traces report ARCHIVE [--show-words]` says how swipe typing went: the share of
//! gestures kept without a correction, which is the measure that counts, and how
//! far touches land from key centres. Words the owner typed appear only with
//! `--show-words`, for the owner's own terminal.

#![allow(clippy::cast_precision_loss)]

use std::collections::{BTreeMap, HashMap};
use std::error::Error;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use synthswipe_bench::traces::{Fate, Traced, merge, parse};

const OWNER_ONLY: u32 = 0o600;
/// Fewer touches than this say nothing about a key.
const MIN_KEY_SAMPLES: usize = 3;
const SHOWN_CONFUSIONS: usize = 15;
const SECONDS_PER_DAY: u64 = 86_400;
const SHOW_WORDS: &str = "--show-words";
const UNKNOWN_DAY: &str = "unknown";

/// A date from days since 1970, in UTC. Howard Hinnant's `civil_from_days`, whose
/// numbers are the days and years of the Gregorian 400-year cycle, counted from March.
fn date_of(days: u64) -> String {
    const DAYS_BEFORE_EPOCH: u64 = 719_468;
    const ERA_DAYS: u64 = 146_097;
    const ERA_YEARS: u64 = 400;
    const YEAR_DAYS: u64 = 365;
    const FOUR_YEARS_DAYS: u64 = 1_460;
    const CENTURY_DAYS: u64 = 36_524;
    const LAST_ERA_DAY: u64 = 146_096;
    const YEARS_PER_CENTURY: u64 = 100;
    const YEARS_PER_LEAP: u64 = 4;
    const MONTHS_FIVE: u64 = 5;
    const MONTH_FACTOR: u64 = 153;
    const MONTH_OFFSET: u64 = 2;
    const MARCH_BASED_JANUARY: u64 = 10;
    const MONTHS_PER_YEAR: u64 = 12;
    const MARCH: u64 = 3;
    const FEBRUARY: u64 = 2;
    let shifted = days + DAYS_BEFORE_EPOCH;
    let era = shifted / ERA_DAYS;
    let day_of_era = shifted % ERA_DAYS;
    let year_of_era = (day_of_era - day_of_era / FOUR_YEARS_DAYS + day_of_era / CENTURY_DAYS - day_of_era / LAST_ERA_DAY) / YEAR_DAYS;
    let day_of_year = day_of_era - (YEAR_DAYS * year_of_era + year_of_era / YEARS_PER_LEAP - year_of_era / YEARS_PER_CENTURY);
    let shifted_month = (MONTHS_FIVE * day_of_year + MONTH_OFFSET) / MONTH_FACTOR;
    let day = day_of_year - (MONTH_FACTOR * shifted_month + MONTH_OFFSET) / MONTHS_FIVE + 1;
    let month = if shifted_month < MARCH_BASED_JANUARY { shifted_month + MARCH } else { shifted_month + MARCH - MONTHS_PER_YEAR };
    let year = year_of_era + era * ERA_YEARS + u64::from(month <= FEBRUARY);
    format!("{year:04}-{month:02}-{day:02}")
}

fn day_of(gesture: &Traced) -> String {
    gesture.session.map_or_else(|| UNKNOWN_DAY.to_owned(), |unix| date_of(unix / SECONDS_PER_DAY))
}

/// The share of a day's gestures kept as written, in percent. A change of mind says
/// nothing of the reading, so it counts neither way.
fn kept_share(fates: &HashMap<Fate, usize>) -> f64 {
    let count = |fate| fates.get(&fate).copied().unwrap_or(0);
    let read: usize = fates.values().sum::<usize>() - count(Fate::Rephrased);
    100.0 * count(Fate::Kept) as f64 / read.max(1) as f64
}

fn print_days(gestures: &[Traced]) {
    println!("\n{:<12}{:>9}{:>8}{:>8}{:>9}{:>11}", "Day", "Gestures", "Kept", "Picked", "Deleted", "Rephrased");
    let mut days: BTreeMap<String, HashMap<Fate, usize>> = BTreeMap::new();
    for gesture in gestures {
        *days.entry(day_of(gesture)).or_default().entry(gesture.fate).or_default() += 1;
    }
    for (day, fates) in days {
        let count = |fate| fates.get(&fate).copied().unwrap_or(0);
        let total: usize = fates.values().sum();
        let kept = kept_share(&fates);
        println!("{day:<12}{total:>9}{kept:>7.0}%{:>8}{:>9}{:>11}", count(Fate::Picked), count(Fate::Deleted), count(Fate::Rephrased));
    }
}

/// Lists what the owner meant and what was written: their own words.
fn print_confusions(gestures: &[Traced]) {
    let mut confusions: HashMap<String, usize> = HashMap::new();
    for gesture in gestures.iter().filter(|gesture| matches!(gesture.fate, Fate::Picked | Fate::Deleted)) {
        let meant = gesture.truth.as_deref().unwrap_or("?");
        *confusions.entry(format!("{meant} <- {}", gesture.word)).or_default() += 1;
    }
    let mut sorted: Vec<(String, usize)> = confusions.into_iter().collect();
    sorted.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    if !sorted.is_empty() {
        println!("\nMost frequent confusions (meant <- written):");
    }
    for (pair, count) in sorted.iter().take(SHOWN_CONFUSIONS) {
        println!("  {count:>3}  {pair}");
    }
}

/// The touch-down and lift-off show aim best, since each is one known key.
fn print_aim(gestures: &[Traced]) {
    let mut offsets: BTreeMap<char, Vec<(f32, f32)>> = BTreeMap::new();
    for gesture in gestures {
        let letters: Vec<char> =
            gesture.truth.as_deref().unwrap_or("").to_lowercase().chars().filter(|&letter| letter != '\'').collect();
        let ends = [letters.first().zip(gesture.samples.first()), letters.last().zip(gesture.samples.last())];
        for (letter, sample) in ends.into_iter().flatten() {
            if let Some(key) = gesture.keys.iter().find(|key| key.label.to_lowercase().chars().eq([*letter])) {
                offsets.entry(*letter).or_default().push(((sample.x - key.x) / key.width, (sample.y - key.y) / key.height));
            }
        }
    }
    offsets.retain(|_, values| values.len() >= MIN_KEY_SAMPLES);
    if !offsets.is_empty() {
        println!("\nAim per key, in key sizes (+x right, +y down):");
    }
    for (letter, values) in offsets {
        let count = values.len() as f32;
        let dx = values.iter().map(|value| value.0).sum::<f32>() / count;
        let dy = values.iter().map(|value| value.1).sum::<f32>() / count;
        println!("  {letter}  {dx:+.2} {dy:+.2}  ({} touches)", values.len());
    }
}

fn report(archive: &str, show_words: bool) -> Result<(), Box<dyn Error>> {
    let gestures = parse(&fs::read_to_string(archive)?);
    if gestures.is_empty() {
        println!("No gestures yet.");
        return Ok(());
    }
    print_days(&gestures);
    if show_words {
        print_confusions(&gestures);
    }
    print_aim(&gestures);
    Ok(())
}

/// Replaces `path` whole, readable by its owner alone.
fn write_private(path: &Path, text: &str) -> Result<(), Box<dyn Error>> {
    let temporary: PathBuf = path.with_extension("tmp");
    let mut out = OpenOptions::new().write(true).create(true).truncate(true).mode(OWNER_ONLY).open(&temporary)?;
    out.write_all(text.as_bytes())?;
    out.sync_all()?;
    fs::rename(temporary, path)?;
    Ok(())
}

fn ingest(archive: &str, received: &[&str]) -> Result<(), Box<dyn Error>> {
    let path = Path::new(archive);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let known = if path.exists() { fs::read_to_string(path)? } else { String::new() };
    let texts = received.iter().map(fs::read_to_string).collect::<Result<Vec<String>, _>>()?;
    let (merged, added) = merge(&known, &texts);
    write_private(path, &merged)?;
    println!("{added} new gestures.");
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let names: Vec<&str> = arguments.iter().map(String::as_str).collect();
    match names.as_slice() {
        ["ingest", archive, received @ ..] => ingest(archive, received),
        ["report", archive] => report(archive, false),
        ["report", archive, SHOW_WORDS] => report(archive, true),
        _ => Err("usage: traces ingest ARCHIVE RECEIVED... | report ARCHIVE [--show-words]".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_change_of_mind_counts_neither_way() {
        let fates = HashMap::from([(Fate::Kept, 3), (Fate::Deleted, 1), (Fate::Rephrased, 6)]);
        assert!((kept_share(&fates) - 75.0).abs() < f64::EPSILON);
    }

    #[test]
    fn a_day_of_nothing_but_changes_of_mind_has_no_share() {
        assert!(kept_share(&HashMap::from([(Fate::Rephrased, 2)])).abs() < f64::EPSILON);
    }

    #[test]
    fn dates_come_out_of_days() {
        assert_eq!(date_of(0), "1970-01-01");
        assert_eq!(date_of(59), "1970-03-01");
        assert_eq!(date_of(11_016), "2000-02-29");
        assert_eq!(date_of(20_758), "2026-11-01");
    }
}
