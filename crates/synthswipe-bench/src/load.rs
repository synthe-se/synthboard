//! Reads every dataset into one `Gesture` shape.

use std::collections::{BTreeMap, HashMap};
use std::error::Error;
use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rayon::prelude::*;
use ruzstd::decoding::StreamingDecoder;
use serde::Deserialize;
use synthswipe::{Affinities, Key, Layout, Lexicon, SENTENCE_START, Sample, spoken_weight, tokens};

type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;

/// One row of the layout × language matrix.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Cell {
    pub source: &'static str,
    pub language: &'static str,
    pub layout: String,
}

#[derive(Debug, Clone, Copy)]
pub struct ClosedLibrary {
    pub top1: bool,
    pub top3: bool,
}

#[derive(Debug)]
pub struct Gesture {
    pub cell: Cell,
    pub tuning: bool,
    pub word: String,
    pub samples: Vec<Sample>,
    pub layout: Layout,
    pub lexicon: Arc<Lexicon>,
    pub closed: Option<ClosedLibrary>,
    /// The sentence's word before this one, when the data keeps the sentence.
    pub previous: Option<String>,
    /// The last words of the sentence before this one.
    pub history: Vec<String>,
    pub affinities: Option<Arc<Affinities>>,
}

fn is_tuning(id: &str) -> bool {
    let mut hasher = DefaultHasher::new();
    id.hash(&mut hasher);
    hasher.finish().is_multiple_of(5)
}

fn zstd_bytes(path: &Path) -> Result<Vec<u8>> {
    let mut decoder = StreamingDecoder::new(BufReader::new(fs::File::open(path)?))?;
    let mut bytes = Vec::new();
    decoder.read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// A listed word never heard in the subtitles keeps part of its list weight.
const UNSPOKEN_RATIO: f32 = 0.6;

/// Word lists, loaded once each by dictionary hash.
/// How gestures and keys are prepared before decoding.
#[derive(Debug, Clone, Copy, Default)]
pub struct Units {
    /// Measure in key sizes, so that a tall key does not inflate vertical error.
    pub key_units: bool,
    /// Moves every sample upward by this fraction of a key height.
    pub offset: f32,
}

pub struct Lexicons {
    directory: PathBuf,
    min_weight: f32,
    spoken: Option<HashMap<String, u32>>,
    pub units: Units,
    loaded: Mutex<HashMap<String, Arc<Lexicon>>>,
}

impl Lexicons {
    pub fn new(directory: PathBuf, min_weight: f32, spoken: Option<HashMap<String, u32>>, units: Units) -> Self {
        Self {
            directory,
            min_weight,
            spoken,
            units,
            loaded: Mutex::new(HashMap::new()),
        }
    }

    pub fn get(&self, hash: &str) -> Result<Arc<Lexicon>> {
        let mut loaded = self.loaded.lock().map_err(|_| "lexicon cache poisoned")?;
        if let Some(lexicon) = loaded.get(hash) {
            return Ok(Arc::clone(lexicon));
        }
        let path = self.directory.join(format!("{hash}.lex.csv.zst"));
        let text = String::from_utf8(zstd_bytes(&path)?)?;
        let words = text.lines().skip(1).filter_map(|line| {
            let (word, frequency) = line.rsplit_once(',')?;
            let weight = frequency.parse::<f32>().ok()?;
            let word = word.trim_matches('"');
            let chosen = match &self.spoken {
                Some(counts) => counts
                    .get(&word.to_lowercase())
                    .map_or(weight * UNSPOKEN_RATIO, |&count| spoken_weight(count)),
                None => weight,
            };
            (weight >= self.min_weight).then_some((word, chosen))
        });
        let lexicon = Arc::new(Lexicon::new(words));
        loaded.insert(hash.to_owned(), Arc::clone(&lexicon));
        Ok(lexicon)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct HeliRow {
    target_word: Option<String>,
    dictionaries: Vec<HeliDictionary>,
    #[serde(default)]
    suggestions: Vec<HeliSuggestion>,
    gesture: Vec<HeliPoint>,
    keyboard_info: HeliKeyboard,
    uuid: String,
}

#[derive(Deserialize)]
struct HeliDictionary {
    hash: String,
    #[serde(rename = "type")]
    kind: String,
}

#[derive(Deserialize)]
struct HeliSuggestion {
    word: String,
}

#[derive(Deserialize)]
struct HeliPoint {
    id: u32,
    x: f32,
    y: f32,
    millis: u32,
}

#[derive(Deserialize)]
struct HeliKeyboard {
    keys: Vec<HeliKey>,
}

#[derive(Deserialize)]
struct HeliKey {
    left: f32,
    width: f32,
    top: f32,
    height: f32,
    value: String,
}

fn layout_name(letters: &str) -> String {
    match letters.get(..6) {
        Some(prefix) if prefix.eq_ignore_ascii_case("azerty") => "azerty".to_owned(),
        Some(prefix) if prefix.eq_ignore_ascii_case("qwerty") => "qwerty".to_owned(),
        Some(prefix) if prefix.eq_ignore_ascii_case("qwertz") => "qwertz".to_owned(),
        _ => "other".to_owned(),
    }
}

fn heli_gesture(row: HeliRow, language: &'static str, lexicons: &Lexicons) -> Option<Gesture> {
    let word = row.target_word?.to_lowercase();
    let first_id = row.gesture.first()?.id;
    if row.gesture.iter().any(|point| point.id != first_id) {
        return None;
    }
    let hash = &row.dictionaries.iter().find(|d| d.kind == "main")?.hash;
    let lexicon = lexicons.get(hash).ok()?;

    let letters: Vec<(char, &HeliKey)> = row
        .keyboard_info
        .keys
        .iter()
        .filter_map(|key| {
            let mut value = key.value.chars();
            let label = value.next()?;
            (value.next().is_none() && (label.is_alphabetic() || label == '\'')).then_some((label, key))
        })
        .collect();
    let name: String = letters.iter().map(|&(label, _)| label).collect();
    let units = lexicons.units;
    let first_key = letters.first()?.1;
    let scale = if units.key_units { first_key.width / first_key.height } else { 1.0 };
    let lift = units.offset * first_key.height;
    let layout = Layout::new(letters.iter().map(|&(label, key)| Key {
        label,
        x: key.left + key.width / 2.0,
        y: (key.top + key.height / 2.0) * scale,
    }))
    .ok()?;

    let suggested = row.suggestions.iter().map(|s| s.word.to_lowercase());
    let top3: Vec<String> = suggested.take(3).collect();
    Some(Gesture {
        cell: Cell {
            source: "heliboard",
            language,
            layout: layout_name(&name),
        },
        tuning: is_tuning(&row.uuid),
        closed: Some(ClosedLibrary {
            top1: top3.first() == Some(&word),
            top3: top3.contains(&word),
        }),
        word,
        samples: row
            .gesture
            .iter()
            .map(|point| Sample {
                x: point.x,
                y: (point.y + lift) * scale,
                t_ms: point.millis,
            })
            .collect(),
        layout,
        lexicon,
        previous: None,
        history: Vec::new(),
        affinities: None,
    })
}

fn gesture_files(root: &Path, language_directory: &str) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for batch in fs::read_dir(root.join("compressed"))? {
        let directory = batch?.path().join(language_directory);
        if directory.is_dir() {
            for file in fs::read_dir(directory)? {
                files.push(file?.path());
            }
        }
    }
    files.sort();
    Ok(files)
}

/// Loads up to `max_files` of a `HeliBoard` language.
pub fn heliboard(
    root: &Path,
    directory: &str,
    language: &'static str,
    max_files: usize,
    lexicons: &Lexicons,
) -> Result<Vec<Gesture>> {
    let files = gesture_files(root, directory)?;
    let chunks: Vec<Vec<Gesture>> = files
        .par_iter()
        .take(max_files)
        .map(|path| -> Result<Vec<Gesture>> {
            let rows: Vec<HeliRow> = serde_json::from_slice(&zstd_bytes(path)?)?;
            Ok(rows
                .into_iter()
                .filter_map(|row| heli_gesture(row, language, lexicons))
                .collect())
        })
        .collect::<Result<_>>()?;
    Ok(chunks.into_iter().flatten().collect())
}

/// The dictionary hash most gestures of a language were collected with.
pub fn main_dictionary(root: &Path, directory: &str) -> Result<String> {
    #[derive(Deserialize)]
    struct Row {
        dictionaries: Vec<HeliDictionary>,
    }
    let path = gesture_files(root, directory)?
        .into_iter()
        .next()
        .ok_or("no gesture file")?;
    let rows: Vec<Row> = serde_json::from_slice(&zstd_bytes(&path)?)?;
    let mut counts: BTreeMap<String, u32> = BTreeMap::new();
    for row in rows {
        for dictionary in row.dictionaries.into_iter().filter(|d| d.kind == "main") {
            *counts.entry(dictionary.hash).or_default() += 1;
        }
    }
    counts
        .into_iter()
        .max_by_key(|&(_, count)| count)
        .map(|(hash, _)| hash)
        .ok_or_else(|| "no main dictionary".into())
}

#[derive(Deserialize)]
struct FutoRow {
    session: String,
    word: String,
    canvas_width: f32,
    canvas_height: f32,
    data: serde_json::Value,
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    layout: Option<String>,
    #[serde(default)]
    dual_finger: Option<u8>,
    #[serde(default)]
    sentence: Option<String>,
    #[serde(default)]
    word_idx: Option<usize>,
}

#[derive(Deserialize)]
struct FutoPoint {
    x: f32,
    y: f32,
    t: f64,
}

#[derive(Deserialize)]
struct FutoLayout {
    keys: Vec<FutoKey>,
}

#[derive(Deserialize)]
struct FutoKey {
    letter: char,
    cx: f32,
    cy: f32,
    rx: f32,
    ry: f32,
}

/// The FUTO rows to keep.
pub struct FutoFilter<'a> {
    pub layouts_directory: &'a Path,
    pub french: Arc<Lexicon>,
    pub english: Arc<Lexicon>,
    pub french_affinities: Option<Arc<Affinities>>,
    pub english_affinities: Option<Arc<Affinities>>,
    pub tuning_only: bool,
    pub max_rows: usize,
    pub units: Units,
}

fn futo_gesture(
    row: FutoRow,
    filter: &FutoFilter<'_>,
    layouts: &mut HashMap<String, Vec<FutoKey>>,
) -> Result<Option<Gesture>> {
    if row.dual_finger.unwrap_or(0) != 0 {
        return Ok(None);
    }
    let (language, lexicon, affinities) = match row.language.as_deref().unwrap_or("en") {
        "fr" => ("fr", &filter.french, &filter.french_affinities),
        "en" => ("en", &filter.english, &filter.english_affinities),
        _ => return Ok(None),
    };
    let name = row.layout.as_deref().unwrap_or("qwerty");
    if !matches!(name, "azerty" | "qwerty" | "qwertz") {
        return Ok(None);
    }
    if !layouts.contains_key(name) {
        let path = filter.layouts_directory.join(format!("{name}.json"));
        let parsed: FutoLayout = serde_json::from_slice(&fs::read(path)?)?;
        layouts.insert(name.to_owned(), parsed.keys);
    }
    let keys = layouts.get(name).ok_or("layout missing")?;
    let sample_key = keys.iter().find(|key| key.letter.is_alphabetic()).ok_or("no keys")?;
    let key_width = 2.0 * sample_key.rx * row.canvas_width;
    let key_height = 2.0 * sample_key.ry * row.canvas_height;
    let scale = if filter.units.key_units { key_width / key_height } else { 1.0 };
    let lift = filter.units.offset * key_height;
    let layout = Layout::new(
        keys.iter()
            .filter(|key| key.letter.is_alphabetic() || key.letter == '\'')
            .map(|key| Key {
                label: key.letter,
                x: key.cx * row.canvas_width,
                y: key.cy * row.canvas_height * scale,
            }),
    )?;
    let points: Vec<FutoPoint> = serde_json::from_value(row.data)?;
    let start = points.first().map_or(0.0, |point| point.t);
    let tuning = filter.tuning_only || is_tuning(&row.session);
    let previous = previous_word(row.sentence.as_deref(), row.word_idx);
    let history = history_before(row.sentence.as_deref(), row.word_idx);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let samples = points
        .iter()
        .map(|point| Sample {
            x: point.x * row.canvas_width,
            y: (point.y * row.canvas_height + lift) * scale,
            t_ms: (point.t - start) as u32,
        })
        .collect();
    Ok(Some(Gesture {
        cell: Cell {
            source: "futo",
            language,
            layout: name.to_owned(),
        },
        tuning,
        word: row.word.to_lowercase(),
        samples,
        layout,
        lexicon: Arc::clone(lexicon),
        closed: None,
        previous,
        history,
        affinities: affinities.clone(),
    }))
}

/// The tokens before the word at `index`, counting words by spaces.
fn history_before(sentence: Option<&str>, index: Option<usize>) -> Vec<String> {
    let (Some(sentence), Some(index)) = (sentence, index) else {
        return Vec::new();
    };
    let before: Vec<&str> = sentence.split(' ').take(index).collect();
    tokens(&before.join(" "))
}

/// The last token before the word at `index`, counting words by spaces.
fn previous_word(sentence: Option<&str>, index: Option<usize>) -> Option<String> {
    let index = index?;
    if index == 0 {
        return Some(SENTENCE_START.to_owned());
    }
    let word = sentence?.split(' ').nth(index - 1)?;
    tokens(word).pop()
}

/// Loads a FUTO JSONL file. Rows without language or layout are English QWERTY.
pub fn futo(path: &Path, filter: &FutoFilter<'_>) -> Result<Vec<Gesture>> {
    let mut gestures = Vec::new();
    futo_each(path, filter, |gesture, _| {
        gestures.push(gesture);
        Ok(())
    })?;
    Ok(gestures)
}

/// Hands each gesture of a FUTO file to `visit` with its session, without
/// keeping them: the training files are too large to hold.
pub fn futo_each(
    path: &Path,
    filter: &FutoFilter<'_>,
    mut visit: impl FnMut(Gesture, &str) -> Result<()>,
) -> Result<()> {
    let mut layouts = HashMap::new();
    let reader = BufReader::new(fs::File::open(path)?);
    for line in reader.lines().take(filter.max_rows) {
        let row: FutoRow = serde_json::from_str(&line?)?;
        let session = row.session.clone();
        if let Some(gesture) = futo_gesture(row, filter, &mut layouts)? {
            visit(gesture, &session)?;
        }
    }
    Ok(())
}
