//! The one reader of the keyboard's trace, for every tool on the Mac.
//!
//! A plain trace holds one record per line, its fields separated by tabs:
//!
//! - `S unix`: a session began, in Unix seconds.
//! - `K keys`: the layout from here on, space-separated `label:x:y:width:height`,
//!   centres and sizes in pixels.
//! - `G word candidates points history other reader`: a gesture. `word` is what it
//!   wrote, `candidates` the ranking joined by commas, `points` space-separated
//!   `x,y,ms`, `history` the sentence before it joined by spaces, `other` what the
//!   reader that did not decide would have written, or nothing, and `reader` who
//!   decided: `g` the geometry, `n` the network. Early traces stop before `history`,
//!   `other` or `reader`.
//! - `C word`: the bar replaced the last gesture's word with this one.
//! - `D`: backspace reached the last gesture's word.
//! - `F word`: what the gesture deleted last really meant. On a retry this comes
//!   after the second gesture, `G(A) D G(B) F(B)`, and is about the first.
//!
//! Anyone with the public key can forge a record, so a line that does not parse
//! is skipped, and `is_sound` keeps it out of the archive.

use std::sync::Arc;

use synthswipe::{Key, Layout, LayoutError, Sample, is_key_label};

const SEPARATOR: char = '\t';
const LIST_SEPARATOR: char = ' ';
/// The fields of a key after its label: x, y, width and height.
const KEY_FIELDS: usize = 4;

/// One key of a traced layout, in pixels.
#[derive(Debug, Clone, PartialEq)]
pub struct TracedKey {
    pub label: String,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// What became of a gesture's word.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Fate {
    Kept,
    Picked,
    Deleted,
}

/// Who chose the word a gesture wrote.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reader {
    Geometry,
    Network,
}

/// One gesture and what happened to it.
#[derive(Debug, Clone, PartialEq)]
pub struct Traced {
    /// The start of its session, in Unix seconds, if one was recorded.
    pub session: Option<u64>,
    pub word: String,
    pub candidates: Vec<String>,
    /// The word really meant, if that is known.
    pub truth: Option<String>,
    pub fate: Fate,
    pub samples: Vec<Sample>,
    pub history: Vec<String>,
    pub other: String,
    pub reader: Option<Reader>,
    pub keys: Arc<Vec<TracedKey>>,
}

fn parse_key(text: &str) -> Option<TracedKey> {
    let mut fields = text.rsplitn(KEY_FIELDS + 1, ':');
    let mut number = || fields.next()?.parse::<f32>().ok().filter(|value| value.is_finite());
    let (height, width, y, x) = (number()?, number()?, number()?, number()?);
    let label = fields.next()?.to_owned();
    (width > 0.0 && height > 0.0).then_some(TracedKey { label, x, y, width, height })
}

fn parse_keys(text: &str) -> Option<Vec<TracedKey>> {
    text.split(LIST_SEPARATOR).filter(|key| !key.is_empty()).map(parse_key).collect()
}

fn parse_sample(text: &str) -> Option<Sample> {
    let mut fields = text.split(',');
    let sample = Sample {
        x: fields.next()?.parse().ok()?,
        y: fields.next()?.parse().ok()?,
        t_ms: fields.next()?.parse().ok()?,
    };
    fields.next().is_none().then_some(sample)
}

fn parse_samples(text: &str) -> Option<Vec<Sample>> {
    text.split(LIST_SEPARATOR).filter(|point| !point.is_empty()).map(parse_sample).collect()
}

fn parse_reader(text: &str) -> Option<Reader> {
    match text {
        "g" => Some(Reader::Geometry),
        "n" => Some(Reader::Network),
        _ => None,
    }
}

/// The decoder's layout from a traced one: the keys a gesture can cross.
///
/// # Errors
/// Returns why the layout cannot be used.
pub fn layout_of(keys: &[TracedKey]) -> Result<Layout, LayoutError> {
    Layout::new(keys.iter().filter_map(|key| {
        let label = key.label.chars().next().filter(|&label| is_key_label(label))?;
        Some(Key { label, x: key.x, y: key.y })
    }))
}

/// Reads a trace, skipping what does not parse.
#[must_use]
pub fn parse(text: &str) -> Vec<Traced> {
    let mut found: Vec<Traced> = Vec::new();
    let mut session = None;
    let mut keys = Arc::new(Vec::new());
    for line in text.lines() {
        let fields: Vec<&str> = line.split(SEPARATOR).collect();
        match fields.as_slice() {
            ["S", unix] => session = unix.parse().ok().or(session),
            ["K", listed] => {
                if let Some(read) = parse_keys(listed).filter(|read| !read.is_empty()) {
                    keys = Arc::new(read);
                }
            }
            ["G", word, candidates, points, rest @ ..] => {
                let Some(samples) = parse_samples(points) else {
                    continue;
                };
                found.push(Traced {
                    session,
                    word: (*word).to_owned(),
                    candidates: candidates.split(',').filter(|word| !word.is_empty()).map(str::to_owned).collect(),
                    truth: Some((*word).to_owned()),
                    fate: Fate::Kept,
                    samples,
                    history: rest.first().map_or_else(Vec::new, |line| {
                        line.split(LIST_SEPARATOR).filter(|word| !word.is_empty()).map(str::to_owned).collect()
                    }),
                    other: rest.get(1).copied().unwrap_or_default().to_owned(),
                    reader: rest.get(2).copied().and_then(parse_reader),
                    keys: Arc::clone(&keys),
                });
            }
            ["C", chosen] => {
                if let Some(gesture) = found.last_mut() {
                    gesture.truth = Some((*chosen).to_owned());
                    gesture.fate = Fate::Picked;
                }
            }
            ["D"] => {
                if let Some(gesture) = found.last_mut() {
                    gesture.truth = None;
                    gesture.fate = Fate::Deleted;
                }
            }
            ["F", meant] => {
                if let Some(gesture) = found.iter_mut().rev().find(|gesture| gesture.fate == Fate::Deleted) {
                    gesture.truth.get_or_insert_with(|| (*meant).to_owned());
                }
            }
            _ => {}
        }
    }
    found
}

/// Whether a line can enter the archive: anyone with the public key can forge one.
#[must_use]
pub fn is_sound(line: &str) -> bool {
    if line.chars().any(|letter| letter != SEPARATOR && letter.is_control()) {
        return false;
    }
    let fields: Vec<&str> = line.split(SEPARATOR).collect();
    match fields.as_slice() {
        ["S", unix] => unix.parse::<u64>().is_ok(),
        ["K", keys] => parse_keys(keys).is_some(),
        ["G", _, _, points, ..] => parse_samples(points).is_some(),
        _ => true,
    }
}

fn is_correction(line: &str) -> bool {
    line == "D" || line.starts_with("C\t") || line.starts_with("F\t")
}

/// Splits a trace into blocks: a gesture line with the corrections after it, or one
/// other line by itself.
#[must_use]
pub fn blocks(text: &str) -> Vec<Vec<&str>> {
    let mut found: Vec<Vec<&str>> = Vec::new();
    let mut open = false;
    for line in text.lines() {
        if open
            && is_correction(line)
            && let Some(block) = found.last_mut()
        {
            block.push(line);
        } else {
            open = line.starts_with("G\t");
            found.push(vec![line]);
        }
    }
    found
}

/// Adds received traces to an archive, keeping each gesture once. The phone's file
/// is cumulative and may arrive several times, so a gesture seen before is dropped,
/// unless the new copy knows more of what happened next. Gives the merged text
/// and how many gestures were new.
#[must_use]
pub fn merge(archive: &str, received: &[String]) -> (String, usize) {
    let mut kept: Vec<Vec<&str>> = blocks(archive);
    let mut position: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for (index, block) in kept.iter().enumerate() {
        if let Some(head) = block.first().filter(|head| head.starts_with("G\t")) {
            position.insert(head, index);
        }
    }
    let mut added = 0;
    for block in received.iter().flat_map(|text| blocks(text)) {
        let Some(&head) = block.first() else {
            continue;
        };
        if !head.starts_with("G\t") {
            if kept.last() != Some(&block) {
                kept.push(block);
            }
        } else if let Some(&index) = position.get(head) {
            if let Some(known) = kept.get_mut(index).filter(|known| block.len() > known.len()) {
                *known = block;
            }
        } else {
            position.insert(head, kept.len());
            kept.push(block);
            added += 1;
        }
    }
    let mut merged = String::new();
    for line in kept.iter().flatten().filter(|line| is_sound(line)) {
        merged.push_str(line);
        merged.push('\n');
    }
    (merged, added)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    const G: &str = "G\tbon\tbon,bien\t1,2,0 3,4,10\tje suis\tbien\tn";

    #[test]
    fn a_gesture_gives_every_field() {
        let read = parse(&format!("S\t86400\nK\ta:1:2:3:4 :5:6:7:8\n{G}\n"));
        let gesture = &read[0];
        assert_eq!(gesture.session, Some(86_400));
        assert_eq!(gesture.word, "bon");
        assert_eq!(gesture.candidates, ["bon", "bien"]);
        assert_eq!(gesture.truth.as_deref(), Some("bon"));
        assert_eq!(gesture.fate, Fate::Kept);
        assert_eq!(gesture.samples[1], Sample { x: 3.0, y: 4.0, t_ms: 10 });
        assert_eq!(gesture.history, ["je", "suis"]);
        assert_eq!(gesture.other, "bien");
        assert_eq!(gesture.reader, Some(Reader::Network));
        assert_eq!(gesture.keys.len(), 2);
        assert_eq!(gesture.keys[1].label, "");
    }

    #[test]
    fn the_reader_is_read_and_early_lines_have_none() {
        let read = parse("G\ta\ta\t1,2,0\th\t\tg\nG\ta\ta\t1,2,0\nG\ta\ta\t1,2,0\th\t\tz\n");
        assert_eq!(read[0].reader, Some(Reader::Geometry));
        assert_eq!(read[1].reader, None);
        assert_eq!(read[1].other, "");
        assert_eq!(read[2].reader, None);
    }

    #[test]
    fn corrections_reach_their_gesture() {
        let read = parse(&format!("{G}\nC\tbien\n{G}\nD\n{G}\nD\nF\tbien\n"));
        assert_eq!((read[0].truth.as_deref(), read[0].fate), (Some("bien"), Fate::Picked));
        assert_eq!((read[1].truth.as_deref(), read[1].fate), (None, Fate::Deleted));
        assert_eq!((read[2].truth.as_deref(), read[2].fate), (Some("bien"), Fate::Deleted));
    }

    #[test]
    fn the_word_meant_reaches_the_gesture_rejected_before_a_retry() {
        let read = parse("G\ta\ta\t1,2,0\nD\nG\tb\tb\t1,2,0\nF\tb\n");
        assert_eq!((read[0].truth.as_deref(), read[0].fate), (Some("b"), Fate::Deleted));
        assert_eq!((read[1].truth.as_deref(), read[1].fate), (Some("b"), Fate::Kept));
    }

    #[test]
    fn a_word_meant_answers_only_the_last_deletion() {
        let read = parse("G\ta\ta\t1,2,0\nD\nG\tb\tb\t1,2,0\nD\nG\tc\tc\t1,2,0\nF\tc\n");
        assert_eq!(read[0].truth, None);
        assert_eq!(read[1].truth.as_deref(), Some("c"));
    }

    #[test]
    fn bad_lines_are_skipped() {
        let read = parse("nonsense\nG\ta\ta\tx,y,z\nS\tnot\nK\tbroken\nG\ta\ta\t1,2,0\nC\n");
        assert_eq!(read.len(), 1);
        assert_eq!(read[0].fate, Fate::Kept);
    }

    #[test]
    fn a_layout_keeps_its_letters() {
        let keys = parse_keys("a:0:0:9:9 b:10:0:9:9 ⇧:20:0:9:9 ':30:0:9:9").unwrap();
        let layout = layout_of(&keys).unwrap();
        assert!(layout.center('a').is_some());
        assert!(layout.center('\'').is_some());
        assert!(layout.center('⇧').is_none());
    }

    #[test]
    fn a_label_may_be_a_colon() {
        assert_eq!(parse_key("::1:2:3:4").unwrap().label, ":");
    }

    #[test]
    fn only_sound_lines_pass() {
        assert!(is_sound(G));
        assert!(is_sound("S\t1"));
        assert!(is_sound("K\ta:1:2:3:4"));
        assert!(is_sound("D"));
        assert!(!is_sound("S\tnever"));
        assert!(!is_sound("K\ta:1:2:0:4"));
        assert!(!is_sound("K\ta:1:2:nan:4"));
        assert!(!is_sound("G\ta\ta\t1,2"));
        assert!(!is_sound("C\ta\u{1b}[2J"));
    }

    #[test]
    fn blocks_hold_a_gesture_and_its_corrections() {
        let text = format!("S\t1\n{G}\nC\ta\nD\nS\t2\nD\n");
        let found = blocks(&text);
        assert_eq!(found.len(), 4);
        assert_eq!(found[1].len(), 3);
        assert_eq!(found[3], ["D"]);
    }

    #[test]
    fn a_gesture_merges_once_and_keeps_the_longer_story() {
        let archive = format!("{G}\n");
        let (merged, added) = merge(&archive, &[format!("{G}\nC\tbien\n"), "G\tx\tx\t1,2,0\n".to_owned()]);
        assert_eq!(added, 1);
        assert_eq!(merged, format!("{G}\nC\tbien\nG\tx\tx\t1,2,0\n"));
    }

    #[test]
    fn a_session_line_is_not_doubled_and_a_forged_line_stays_out() {
        let (merged, _) = merge("S\t1\n", &["S\t1\nS\tforged\n".to_owned()]);
        assert_eq!(merged, "S\t1\n");
    }
}
