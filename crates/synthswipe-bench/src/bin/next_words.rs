//! Lists the likeliest next words for each common word, from pair counts.
//!
//! Usage: `next_words OUTPUT MIN_COUNT PRIMARY_COUNTS SECONDARY_COUNTS`. Each
//! output line is `word TAB next next next`. Two slots go to the primary
//! language and one to the secondary, so a French sentence start still offers
//! an English word and the reverse.

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt::Write as _;

const SLOTS: usize = 3;

fn read(path: &str, min_count: u32) -> Result<BTreeMap<String, Vec<(u32, String)>>, Box<dyn Error>> {
    let mut next: BTreeMap<String, Vec<(u32, String)>> = BTreeMap::new();
    for line in std::fs::read_to_string(path)?.lines() {
        let mut fields = line.split(' ');
        let (Some(first), Some(second), Some(count), None) =
            (fields.next(), fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let count: u32 = count.parse().unwrap_or(0);
        if count >= min_count && second.chars().next().is_some_and(char::is_alphabetic) {
            next.entry(first.to_owned()).or_default().push((count, second.to_owned()));
        }
    }
    for list in next.values_mut() {
        list.sort_unstable_by(|a, b| b.cmp(a));
        list.truncate(SLOTS);
    }
    Ok(next)
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut arguments = std::env::args().skip(1);
    let output = arguments.next().ok_or("missing OUTPUT")?;
    let min_count: u32 = arguments.next().ok_or("missing MIN_COUNT")?.parse()?;
    let primary = read(&arguments.next().ok_or("missing PRIMARY")?, min_count)?;
    let secondary = read(&arguments.next().ok_or("missing SECONDARY")?, min_count)?;

    let mut text = String::new();
    let words: std::collections::BTreeSet<&String> = primary.keys().chain(secondary.keys()).collect();
    for word in words {
        let (first, second) = (primary.get(word), secondary.get(word));
        let mut chosen: Vec<&str> = Vec::new();
        let limit = if second.is_some() { SLOTS - 1 } else { SLOTS };
        for (_, next) in first.into_iter().flatten().take(limit) {
            chosen.push(next);
        }
        for (_, next) in second.into_iter().flatten() {
            if chosen.len() < SLOTS && !chosen.contains(&next.as_str()) {
                chosen.push(next);
            }
        }
        writeln!(text, "{word}\t{}", chosen.join(" "))?;
    }
    std::fs::write(output, &text)?;
    println!("{} bytes", text.len());
    Ok(())
}
