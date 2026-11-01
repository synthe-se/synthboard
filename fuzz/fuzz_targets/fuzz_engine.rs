#![no_main]

use std::sync::{Arc, OnceLock};

use libfuzzer_sys::fuzz_target;
use synthboard_core::{Catalog, Command, DrawnKey, Engine, Event, Field, Icon, NextWords, Step};
use synthswipe::{Affinities, Bigrams, Encoder, Grammar, Lexicon, MODEL_HEAD_BYTES, MODEL_MAGIC, Neural, fold, split_model};

const EVENT_BYTES: usize = 22;
const RULES: &str = include_str!("../../android/app/src/main/assets/grammar.tsv");
/// What makes a word revised by the next: a verb that wants the other reading.
const COUNTS: &str = "peur 1000\npeut 1000\naller 1000\nrien 100000\npeut aller 900\n";
/// Steps of a scripted swipe between two keys, and the milliseconds of each.
const SWIPE_STEPS: u32 = 8;
const SWIPE_STEP_MS: u64 = 12;
const SWIPE_PAUSE_MS: u64 = 700;
/// The first byte chooses the script: these bits swipe a pair, then backspace after it.
const SWIPES_PAIR: u8 = 1;
const TAKES_IT_BACK: u8 = 2;

/// Answers with values that follow the gesture, so that the blend gets real work.
#[derive(Debug)]
struct Fake;

impl Encoder for Fake {
    fn encode(&self, input: &[f32], output: &mut [f32]) -> bool {
        let seed = input.first().copied().unwrap_or(0.0);
        output.iter_mut().enumerate().for_each(|(index, value)| *value = seed * (index % 7) as f32);
        true
    }
}

struct Shared {
    lexicon: Arc<Lexicon>,
    affinities: Arc<Affinities>,
    grammar: Arc<Grammar>,
    next_words: Arc<NextWords>,
    catalog: Arc<Catalog>,
    neural: Arc<Neural>,
}

fn shared() -> &'static Shared {
    static SHARED: OnceLock<Shared> = OnceLock::new();
    SHARED.get_or_init(|| Shared {
        lexicon: Arc::new(Lexicon::with_languages([
            ("bonjour", 150.0, 1),
            ("c'est", 160.0, 1),
            ("été", 140.0, 1),
            ("où", 150.0, 1),
            ("ou", 170.0, 1),
            ("merge", 110.0, 2),
            ("peur", 170.0, 0),
            ("peut", 165.0, 1),
            ("aller", 150.0, 1),
            ("i'll", 120.0, 2),
            ("👍", 100.0, 0),
        ])),
        affinities: {
            let mut pairs = Vec::new();
            Bigrams::parse(COUNTS).compile_into(1, 0.0, &mut pairs);
            Arc::new(Affinities::from_bytes(&Affinities::to_bytes(pairs)))
        },
        grammar: Arc::new(Grammar::parse(&format!("{RULES}suis\t1\nsous\t0\nbonjour\t0\naller\t64\n"))),
        next_words: Arc::new(NextWords::parse("<s>\tje the\nje\tsuis\n")),
        catalog: Arc::new(Catalog::parse("Food\t🍕\t\tpizza|cheese\nPeople\t👍\t👍🏽\tpouce|thumbs up\n")),
        neural: {
            let mut file = MODEL_MAGIC.to_vec();
            file.resize(file.len() + MODEL_HEAD_BYTES, 0);
            file.push(1);
            let (head, _) = split_model(&file).expect("a well-formed model");
            Arc::new(Neural::new(head, Box::new(Fake)))
        },
    })
}

fn text(seed: u8) -> String {
    const SAMPLES: [&str; 8] = ["", "a", "é", "👍🏽", "bonjour ", "c'est. ", "\n", "e\u{301}"];
    SAMPLES[usize::from(seed) % SAMPLES.len()].to_owned()
}

fn float(chunk: &[u8]) -> f32 {
    f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]])
}

fn event(chunk: &[u8]) -> Event {
    let x = f32::from(u16::from_le_bytes([chunk[1], chunk[2]])) / 16.0 - 100.0;
    let y = f32::from(u16::from_le_bytes([chunk[3], chunk[4]])) / 16.0 - 100.0;
    let t = u64::from_le_bytes([chunk[5], chunk[6], chunk[7], chunk[8], chunk[9], chunk[10], chunk[11], chunk[12]]);
    let pointer = i32::from(chunk[13] % 3);
    let number = u32::from(chunk[14]) * 4;
    let wild = float(&chunk[17..]);
    let flag = chunk[14] % 2 == 0;
    match chunk[0] % 31 {
        0 => Event::Resize { width: x.max(1.0) * 4.0, screen_height: y.max(1.0) * 4.0, density: f32::from(chunk[13] % 4 + 1) },
        1 => Event::Shown {
            field: Field { input_type: u32::from(chunk[13]) | (u32::from(chunk[14]) << 12), ime_options: u32::from(chunk[15]) << 20 },
            before: text(chunk[13]),
            after: text(chunk[14]),
            cursor: number,
        },
        2 => Event::Hidden,
        3 | 4 => Event::Down { pointer, x, y, t },
        5 | 6 | 7 => Event::Move { pointer, x, y, t },
        8 | 9 => Event::Up { pointer, x, y, t },
        10 => Event::Cancel,
        11 => Event::Deadline { t },
        12 => Event::Selection { start: number, end: u32::from(chunk[15]) * 4 },
        13 => Event::Context { before: text(chunk[13]), after: text(chunk[14]) },
        14 => Event::Text { text: text(chunk[13]) },
        15 => Event::DeleteOne,
        16 => Event::OpenEmojiSearch,
        17 => Event::CloseEmojiPanel,
        18 => Event::Hover { x, y },
        19 => Event::Session { unix: t },
        20 => Event::Recents { items: vec![text(chunk[13])] },
        21 => Event::Neural { deciding: flag },
        22 => Event::Alphabet { qwerty: flag },
        23 => Event::Screenshots { allowed: flag },
        24 => Event::HeightScale { scale: wild },
        25 => Event::ForgetAll,
        26 => Event::LanguageReady,
        27 => Event::Traces { on: flag },
        28 => Event::HoverEnd { x, y, t },
        29 => Event::ToneChosen { variant: number, variants: u32::from(chunk[15]) },
        _ => Event::Move { pointer, x: wild, y: float(&chunk[18..]), t },
    }
}

fn center(key: &DrawnKey) -> (f32, f32) {
    (key.x + key.width / 2.0, key.y + key.height / 2.0)
}

fn drawn_keys(step: &Step) -> Vec<DrawnKey> {
    step.commands
        .iter()
        .find_map(|command| match command {
            Command::Keys { keys, .. } => Some(keys.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

/// Draws a word across its keys, and gives what lifting the finger did.
fn swipe(engine: &mut Engine, keys: &[DrawnKey], word: &str, clock: &mut u64) -> Step {
    let points: Vec<(f32, f32)> =
        fold(word).iter().filter_map(|letter| keys.iter().find(|key| key.label == letter.to_string())).map(center).collect();
    let Some(&(mut x, mut y)) = points.first() else {
        return Step::default();
    };
    *clock += SWIPE_PAUSE_MS;
    engine.step(Event::Down { pointer: 0, x, y, t: *clock });
    for &(to_x, to_y) in points.iter().skip(1) {
        for step in 1..=SWIPE_STEPS {
            let ratio = step as f32 / SWIPE_STEPS as f32;
            *clock += SWIPE_STEP_MS;
            engine.step(Event::Move { pointer: 0, x: x + (to_x - x) * ratio, y: y + (to_y - y) * ratio, t: *clock });
        }
        (x, y) = (to_x, to_y);
    }
    engine.step(Event::Up { pointer: 0, x, y, t: *clock })
}

/// Tries the engine right after a revision, with every event still to come.
fn revise(engine: &mut Engine, keys: &[DrawnKey], takes_it_back: bool) {
    let mut clock = 0;
    swipe(engine, keys, "peur", &mut clock);
    let lift = swipe(engine, keys, "aller", &mut clock);
    assert!(lift.commands.iter().any(|command| matches!(command, Command::DeleteBefore { .. })), "the script no longer revises");
    if let Some(key) = keys.iter().find(|key| key.icon == Icon::Delete && takes_it_back) {
        let (x, y) = center(key);
        engine.step(Event::Down { pointer: 0, x, y, t: clock + SWIPE_PAUSE_MS });
        engine.step(Event::Up { pointer: 0, x, y, t: clock + SWIPE_PAUSE_MS + SWIPE_STEP_MS });
    }
}

fuzz_target!(|data: &[u8]| {
    let data_shared = shared();
    let mut engine = Engine::new();
    engine.set_lexicon(Arc::clone(&data_shared.lexicon));
    engine.set_language(
        Arc::clone(&data_shared.affinities),
        Arc::clone(&data_shared.grammar),
        Arc::clone(&data_shared.next_words),
    );
    engine.set_catalog(Arc::clone(&data_shared.catalog));
    engine.set_neural(Arc::clone(&data_shared.neural));
    engine.set_trace_key([9; 32]);
    engine.set_memory_key([7; 32]);
    engine.step(Event::Resize { width: 1080.0, screen_height: 2424.0, density: 2.625 });
    let shown = engine.step(Event::Shown { field: Field { input_type: 1, ime_options: 0 }, before: String::new(), after: String::new(), cursor: 0 });
    engine.load_memory("bonjour\t2\n<s>\tbonjour\t2\n^GitHub\n!merge\n");
    let script = data.first().copied().unwrap_or(0);
    if script & SWIPES_PAIR != 0 {
        revise(&mut engine, &drawn_keys(&shown), script & TAKES_IT_BACK != 0);
    }
    for chunk in data.chunks_exact(EVENT_BYTES) {
        let current = event(chunk);
        if let Some((decoding, path)) = engine.preview_job() {
            let _ = decoding.preview(&path);
        }
        let step = engine.step(current);
        for command in &step.commands {
            if let Command::Commit { text } = command {
                assert!(!text.contains('\0'));
            }
        }
    }
    let _ = engine.recovered();
    let _ = engine.take_unsaved();
    let settled = engine.step(Event::Cancel);
    assert_eq!(settled.deadline, None, "a deadline outlived every finger");
});
