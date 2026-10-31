#![no_main]

use std::sync::{Arc, OnceLock};

use libfuzzer_sys::fuzz_target;
use synthboard_core::{Catalog, Command, Engine, Event, Field, NextWords};
use synthswipe::{Affinities, Grammar, Lexicon};

const EVENT_BYTES: usize = 12;

struct Shared {
    lexicon: Arc<Lexicon>,
    affinities: Arc<Affinities>,
    grammar: Arc<Grammar>,
    next_words: Arc<NextWords>,
    catalog: Arc<Catalog>,
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
            ("i'll", 120.0, 2),
            ("👍", 100.0, 0),
        ])),
        affinities: Arc::new(Affinities::default()),
        grammar: Arc::new(Grammar::parse("suis\t1\nsous\t0\n")),
        next_words: Arc::new(NextWords::parse("<s>\tje the\nje\tsuis\n")),
        catalog: Arc::new(Catalog::parse("Food\t🍕\t\tpizza|cheese\nPeople\t👍\t👍🏽\tpouce|thumbs up\n")),
    })
}

fn text(seed: u8) -> String {
    const SAMPLES: [&str; 8] = ["", "a", "é", "👍🏽", "bonjour ", "c'est. ", "\n", "e\u{301}"];
    SAMPLES[usize::from(seed) % SAMPLES.len()].to_owned()
}

fn event(chunk: &[u8]) -> Event {
    let x = f32::from(u16::from_le_bytes([chunk[1], chunk[2]])) / 16.0 - 100.0;
    let y = f32::from(u16::from_le_bytes([chunk[3], chunk[4]])) / 16.0 - 100.0;
    let t = u64::from(u32::from_le_bytes([chunk[5], chunk[6], chunk[7], chunk[8]]));
    let pointer = i32::from(chunk[9] % 3);
    let number = u32::from(chunk[10]) * 4;
    match chunk[0] % 19 {
        0 => Event::Resize { width: x.max(1.0) * 4.0, density: f32::from(chunk[9] % 4 + 1) },
        1 => Event::Shown {
            field: Field { input_type: u32::from(chunk[9]) | (u32::from(chunk[10]) << 12), ime_options: u32::from(chunk[11]) << 20 },
            before: text(chunk[9]),
            cursor: number,
        },
        2 => Event::Hidden,
        3 | 4 => Event::Down { pointer, x, y, t },
        5 | 6 | 7 => Event::Move { pointer, x, y, t },
        8 | 9 => Event::Up { pointer, x, y, t },
        10 => Event::Cancel,
        11 => Event::Deadline { t },
        12 => Event::Selection { start: number, end: u32::from(chunk[11]) * 4 },
        13 => Event::Context { before: text(chunk[9]) },
        14 => Event::Text { text: text(chunk[9]) },
        15 => Event::DeleteOne,
        16 => Event::OpenEmojiSearch,
        17 => Event::CloseEmojiPanel,
        _ => Event::Memory { text: text(chunk[9]) },
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
    engine.step(Event::Resize { width: 1080.0, density: 2.625 });
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
    let settled = engine.step(Event::Cancel);
    assert_eq!(settled.deadline, None, "a deadline outlived every finger");
});
