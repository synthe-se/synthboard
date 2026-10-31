//! Counts words and word pairs from plain text on stdin.
//!
//! Usage: `ngrams OUTPUT MAX_TOKENS MIN_PAIR_COUNT`. Stops reading once
//! `MAX_TOKENS` is reached, and keeps only aggregate counts, never sentences.

use std::collections::HashMap;
use std::error::Error;
use std::fs::File;
use std::io::{BufRead, BufWriter, Write, stdin};

use synthswipe::{SENTENCE_START, tokens};

const MIN_WORD_COUNT: u32 = 5;

fn main() -> Result<(), Box<dyn Error>> {
    let mut arguments = std::env::args().skip(1);
    let output = arguments.next().ok_or("missing OUTPUT")?;
    let max_tokens: u64 = arguments.next().ok_or("missing MAX_TOKENS")?.parse()?;
    let min_pair: u32 = arguments.next().ok_or("missing MIN_PAIR_COUNT")?.parse()?;

    let mut ids: HashMap<String, u32> = HashMap::new();
    let mut words: Vec<String> = Vec::new();
    let mut unigrams: Vec<u32> = Vec::new();
    let mut pairs: HashMap<u64, u32> = HashMap::new();
    let mut seen: u64 = 0;

    let mut id_of = |word: &str, words: &mut Vec<String>, unigrams: &mut Vec<u32>| -> u32 {
        if let Some(&id) = ids.get(word) {
            return id;
        }
        let id = u32::try_from(words.len()).unwrap_or(u32::MAX);
        ids.insert(word.to_owned(), id);
        words.push(word.to_owned());
        unigrams.push(0);
        id
    };

    let start = id_of(SENTENCE_START, &mut words, &mut unigrams);
    for line in stdin().lock().lines() {
        let mut previous = start;
        if let Some(slot) = unigrams.get_mut(start as usize) {
            *slot += 1;
        }
        for word in tokens(&line?) {
            let id = id_of(&word, &mut words, &mut unigrams);
            if let Some(slot) = unigrams.get_mut(id as usize) {
                *slot += 1;
            }
            *pairs.entry(u64::from(previous) << 32 | u64::from(id)).or_default() += 1;
            previous = id;
            seen += 1;
        }
        if seen >= max_tokens {
            break;
        }
    }

    let mut out = BufWriter::new(File::create(output)?);
    let kept = |id: u32| unigrams.get(id as usize).is_some_and(|&count| count >= MIN_WORD_COUNT);
    for (id, word) in (0_u32..).zip(&words) {
        if kept(id) {
            writeln!(out, "{word} {}", unigrams.get(id as usize).copied().unwrap_or(0))?;
        }
    }
    for (key, count) in &pairs {
        let (first, second) = (u32::try_from(key >> 32)?, u32::try_from(key & u64::from(u32::MAX))?);
        if *count >= min_pair && kept(first) && kept(second)
            && let (Some(a), Some(b)) = (words.get(first as usize), words.get(second as usize)) {
                writeln!(out, "{a} {b} {count}")?;
            }
    }
    eprintln!("{seen} tokens, {} words, {} pairs seen", words.len(), pairs.len());
    Ok(())
}
