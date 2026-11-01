//! Binding between the Android shell and the Rust core. No logic lives here:
//! it only stops panics at the boundary, which BoltFFI does not do by itself.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex, MutexGuard, Once, PoisonError};

use boltffi::export;
use synthboard_core::{Catalog, EmojiGroup, Engine, Event, Field, NextWords, Step};
use synthswipe::{Affinities, Grammar, Lexicon};

boltffi::scaffolding!();

/// The keyboard, as seen from Kotlin.
#[export]
pub struct Keyboard {
    engine: Mutex<Engine>,
}

/// A panic message could quote what was typed, so none is ever printed.
fn silence_panics() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| std::panic::set_hook(Box::new(|_| {})));
}

/// Runs `work`, turning a panic into the empty result so that the keyboard
/// process survives. The caller decides how to recover.
fn guarded<T: Default>(work: impl FnOnce() -> T) -> (T, bool) {
    catch_unwind(AssertUnwindSafe(work)).map_or_else(|_| (T::default(), true), |value| (value, false))
}

fn parse_lexicon(bytes: &[u8]) -> Lexicon {
    let text = String::from_utf8_lossy(bytes);
    Lexicon::with_languages(text.lines().filter_map(|line| {
        let (rest, languages) = line.rsplit_once(',')?;
        let (word, weight) = rest.rsplit_once(',')?;
        Some((word, weight.parse::<f32>().ok()?, languages.parse::<u8>().ok()?))
    }))
}

#[export]
impl Keyboard {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        silence_panics();
        Self { engine: Mutex::new(Engine::new()) }
    }

    /// Handles one event. After a panic the engine restarts empty, so taps
    /// keep working and the shell reopens it on the next `Shown` (X-9).
    pub fn step(&self, event: Event) -> Step {
        let mut engine = self.lock();
        let (step, panicked) = guarded(|| engine.step(event));
        if panicked {
            let lexicon = engine.lexicon();
            *engine = Engine::new();
            if let Some(lexicon) = lexicon {
                engine.set_lexicon(lexicon);
            }
        }
        step
    }

    /// Whether the shell may read this field's text.
    pub fn may_read(&self, field: Field) -> bool {
        field.allows_reading()
    }

    /// Allows traces, sealed for the owner of this public key, given as hex.
    /// Anything but a valid key leaves traces off.
    pub fn set_trace_key(&self, hex: String) {
        if let Some(key) = synthboard_seal::from_hex(hex.trim()).and_then(|bytes| bytes.try_into().ok()) {
            self.lock().set_trace_key(key);
        }
    }

    /// Reads saved memory away from the engine's lock, then gives it to the engine.
    pub fn load_memory(&self, text: String) {
        let read = self.lock().memory_reader();
        let (memory, panicked) = guarded(|| Some(read(&text)));
        if let (Some(memory), false) = (memory, panicked) {
            self.lock().adopt_memory(memory);
        }
    }

    /// The word the gesture under way would give, or nothing. The engine is
    /// locked only to copy the gesture, so taps never wait for this decode.
    pub fn preview(&self) -> String {
        let job = self.lock().preview_job();
        let Some((decoding, path)) = job else {
            return String::new();
        };
        guarded(|| decoding.preview(&path)).0
    }

    /// Parses the language data and gives it to the engine.
    pub fn load_language(&self, affinities: Vec<u8>, forms: Vec<u8>, next_words: Vec<u8>) {
        let (parsed, _) = guarded(|| {
            Some((
                Arc::new(Affinities::from_bytes(&affinities)),
                Arc::new(Grammar::parse(&String::from_utf8_lossy(&forms))),
                Arc::new(NextWords::parse(&String::from_utf8_lossy(&next_words))),
            ))
        });
        if let Some((affinities, grammar, next_words)) = parsed {
            self.lock().set_language(affinities, grammar, next_words);
        }
    }

    /// Parses the emoji asset, keeps it for search and returns it for display.
    pub fn load_emoji(&self, bytes: Vec<u8>) -> Vec<EmojiGroup> {
        let parsed = guarded(|| Some(Arc::new(Catalog::parse(&String::from_utf8_lossy(&bytes)))));
        let Some(catalog) = parsed.0 else {
            return Vec::new();
        };
        self.lock().set_catalog(Arc::clone(&catalog));
        catalog.groups.clone()
    }

    /// Parses `word,weight` lines and gives the lexicon to the engine.
    pub fn load_lexicon(&self, bytes: Vec<u8>) {
        let (lexicon, panicked) = guarded(|| Some(Arc::new(parse_lexicon(&bytes))));
        if let (Some(lexicon), false) = (lexicon, panicked) {
            self.lock().set_lexicon(lexicon);
        }
    }
}

impl Keyboard {
    fn lock(&self) -> MutexGuard<'_, Engine> {
        self.engine.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_panic_stops_at_the_boundary() {
        let (value, panicked) = guarded::<u32>(|| std::panic::panic_any("boom"));
        assert_eq!((value, panicked), (0, true));
    }

    #[test]
    fn normal_work_passes_through() {
        assert_eq!(guarded(|| 7_u32), (7, false));
    }

    #[test]
    fn the_lexicon_format_skips_bad_lines() {
        let lexicon = parse_lexicon(b"bonjour,120,1\nnot a line\nbon,abc,1\nmerge,80,2\n");
        assert_eq!(lexicon.len(), 2);
        assert!(lexicon.contains("bonjour"));
    }
}
