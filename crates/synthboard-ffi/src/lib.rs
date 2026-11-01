//! Binding between the Android shell and the Rust core. No logic lives here: it
//! decodes each call, stops panics at the boundary and encodes the reply. The
//! only `unsafe` in the project is in `jni`, and all it does is move bytes.

#![deny(unsafe_code)]

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex, MutexGuard, Once, PoisonError};

use synthboard_core::codec::{Wire, decode};
use synthboard_core::{Catalog, EmojiGroup, Engine, Event, Field, NextWords, Step, wire_enum};
use synthswipe::{Affinities, Grammar, Lexicon};
use zeroize::Zeroizing;

#[allow(unsafe_code)]
mod jni;

wire_enum! {
    /// One request from the shell.
    #[derive(Debug, Clone, PartialEq)]
    pub enum Call {
        Step { event: Event },
        MayRead { field: Field },
        SetTraceKey { hex: String },
        Preview,
        LoadLanguage { affinities: Vec<u8>, forms: Vec<u8>, next_words: Vec<u8> },
        LoadEmoji { bytes: Vec<u8> },
        LoadLexicon { bytes: Vec<u8> },
        NewMemoryKeys,
        SetMemoryKey { key: Vec<u8> },
        LoadMemory { sealed: Vec<u8>, secret: Vec<u8> },
        OpenMemory { sealed: Vec<u8>, secret: Vec<u8> },
    }
}

wire_enum! {
    /// The answer to a call.
    #[derive(Debug, PartialEq)]
    pub enum Reply {
        Done,
        Stepped { step: Step },
        Flag { on: bool },
        Text { text: String },
        Bytes { bytes: Vec<u8> },
        Emoji { groups: Vec<EmojiGroup> },
    }
}

/// The keyboard. A process has one, and everything in it is behind its lock.
#[derive(Debug)]
pub struct Keyboard {
    engine: Mutex<Engine>,
}

/// Answers one encoded call into `reply`. A call that does not decode gets no
/// answer: only a mismatch between the two sides can cause it, and nothing
/// should act on it.
pub fn handle(keyboard: &Keyboard, input: &[u8], reply: &mut Vec<u8>) {
    let (answer, _) = guarded(|| decode::<Call>(input).map(|call| keyboard.answer(call)));
    if let Some(answer) = answer {
        answer.put(reply);
    }
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

/// Opens sealed memory and wipes the secret key given for it.
fn opened(sealed: &[u8], secret: Vec<u8>) -> Option<Zeroizing<Vec<u8>>> {
    let secret = Zeroizing::new(secret);
    if sealed.is_empty() {
        return Some(Zeroizing::new(Vec::new()));
    }
    let key = Zeroizing::new(<[u8; 32]>::try_from(secret.as_slice()).ok()?);
    synthboard_seal::open(&key, sealed).ok().map(Zeroizing::new)
}

fn parse_lexicon(bytes: &[u8]) -> Lexicon {
    let text = String::from_utf8_lossy(bytes);
    Lexicon::with_languages(text.lines().filter_map(|line| {
        let (rest, languages) = line.rsplit_once(',')?;
        let (word, weight) = rest.rsplit_once(',')?;
        Some((word, weight.parse::<f32>().ok()?, languages.parse::<u8>().ok()?))
    }))
}

impl Keyboard {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        silence_panics();
        Self { engine: Mutex::new(Engine::new()) }
    }

    fn answer(&self, call: Call) -> Reply {
        match call {
            Call::Step { event } => Reply::Stepped { step: self.step(event) },
            Call::MayRead { field } => Reply::Flag { on: self.may_read(field) },
            Call::SetTraceKey { hex } => {
                self.set_trace_key(&hex);
                Reply::Done
            }
            Call::Preview => Reply::Text { text: self.preview() },
            Call::LoadLanguage { affinities, forms, next_words } => {
                self.load_language(&affinities, &forms, &next_words);
                Reply::Done
            }
            Call::LoadEmoji { bytes } => Reply::Emoji { groups: self.load_emoji(&bytes) },
            Call::LoadLexicon { bytes } => {
                self.load_lexicon(&bytes);
                Reply::Done
            }
            Call::NewMemoryKeys => Reply::Bytes { bytes: self.new_memory_keys() },
            Call::SetMemoryKey { key } => {
                self.set_memory_key(key);
                Reply::Done
            }
            Call::LoadMemory { sealed, secret } => {
                self.load_memory(&sealed, secret);
                Reply::Done
            }
            Call::OpenMemory { sealed, secret } => Reply::Bytes { bytes: self.open_memory(&sealed, secret) },
        }
    }

    /// Handles one event. After a panic the engine restarts empty, so taps
    /// keep working and the shell reopens it on the next `Shown` (X-9).
    fn step(&self, event: Event) -> Step {
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
    fn may_read(&self, field: Field) -> bool {
        field.allows_reading()
    }

    /// Allows traces, sealed for the owner of this public key, given as hex.
    /// Anything but a valid key leaves traces off.
    fn set_trace_key(&self, hex: &str) {
        if let Some(key) = synthboard_seal::from_hex(hex.trim()).and_then(|bytes| bytes.try_into().ok()) {
            self.lock().set_trace_key(key);
        }
    }

    /// Makes the key pair memory is sealed to: 32 secret bytes, then 32 public.
    fn new_memory_keys(&self) -> Vec<u8> {
        synthboard_seal::generate_keys().map_or_else(|_| Vec::new(), |(secret, public)| [secret, public].concat())
    }

    /// Sets the public key that memory is sealed to. Anything else changes nothing.
    fn set_memory_key(&self, public: Vec<u8>) {
        if let Ok(key) = public.try_into() {
            self.lock().set_memory_key(key);
        }
    }

    /// Opens saved memory, which only exists in the clear on this side of the
    /// boundary, and gives it to the engine. No sealed bytes means no memory yet.
    /// The engine's lock is not held meanwhile, so a long memory delays no tap.
    fn load_memory(&self, sealed: &[u8], secret: Vec<u8>) {
        let read = self.lock().memory_reader();
        let (memory, panicked) = guarded(|| {
            let text = opened(sealed, secret)?;
            Some(read(&String::from_utf8_lossy(&text)))
        });
        if let (Some(memory), false) = (memory, panicked) {
            self.lock().adopt_memory(memory);
        }
    }

    /// The plain text of saved memory, for an export the user asked for.
    fn open_memory(&self, sealed: &[u8], secret: Vec<u8>) -> Vec<u8> {
        opened(sealed, secret).map_or_else(Vec::new, |text| text.to_vec())
    }

    /// The word the gesture under way would give, or nothing. The engine is
    /// locked only to copy the gesture, so taps never wait for this decode.
    fn preview(&self) -> String {
        let job = self.lock().preview_job();
        let Some((decoding, path)) = job else {
            return String::new();
        };
        guarded(|| decoding.preview(&path)).0
    }

    /// Parses the language data and gives it to the engine.
    fn load_language(&self, affinities: &[u8], forms: &[u8], next_words: &[u8]) {
        let (parsed, _) = guarded(|| {
            Some((
                Arc::new(Affinities::from_bytes(affinities)),
                Arc::new(Grammar::parse(&String::from_utf8_lossy(forms))),
                Arc::new(NextWords::parse(&String::from_utf8_lossy(next_words))),
            ))
        });
        if let Some((affinities, grammar, next_words)) = parsed {
            self.lock().set_language(affinities, grammar, next_words);
        }
    }

    /// Parses the emoji asset, keeps it for search and returns it for display.
    fn load_emoji(&self, bytes: &[u8]) -> Vec<EmojiGroup> {
        let parsed = guarded(|| Some(Arc::new(Catalog::parse(&String::from_utf8_lossy(bytes)))));
        let Some(catalog) = parsed.0 else {
            return Vec::new();
        };
        self.lock().set_catalog(Arc::clone(&catalog));
        catalog.groups.clone()
    }

    /// Parses `word,weight` lines and gives the lexicon to the engine.
    fn load_lexicon(&self, bytes: &[u8]) {
        let (lexicon, panicked) = guarded(|| Some(Arc::new(parse_lexicon(bytes))));
        if let (Some(lexicon), false) = (lexicon, panicked) {
            self.lock().set_lexicon(lexicon);
        }
    }

    fn lock(&self) -> MutexGuard<'_, Engine> {
        self.engine.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use synthboard_core::codec::encode;

    use super::*;

    #[test]
    fn a_panic_stops_at_the_boundary() {
        let (value, panicked) = guarded::<u32>(|| std::panic::panic_any("boom"));
        assert_eq!((value, panicked), (0, true));
    }

    #[test]
    fn no_saved_memory_opens_as_nothing_and_a_wrong_key_as_a_refusal() {
        assert_eq!(opened(&[], Vec::new()).as_deref(), Some(&Vec::new()));
        let (_, public) = synthboard_seal::generate_keys().unwrap_or_default();
        let (other, _) = synthboard_seal::generate_keys().unwrap_or_default();
        let sealed = synthboard_seal::seal(&public, b"mot").unwrap_or_default();
        assert!(opened(&sealed, other.to_vec()).is_none());
        assert!(opened(&sealed, vec![1, 2]).is_none());
    }

    fn answer(keyboard: &Keyboard, call: &Call) -> Option<Reply> {
        let mut reply = Vec::new();
        handle(keyboard, &encode(call), &mut reply);
        decode(&reply)
    }

    #[test]
    fn a_call_comes_back_as_its_reply() {
        let keyboard = Keyboard::new();
        let field = Field { input_type: 0x81, ime_options: 0 };
        assert_eq!(answer(&keyboard, &Call::MayRead { field }), Some(Reply::Flag { on: false }));
        assert_eq!(answer(&keyboard, &Call::Preview), Some(Reply::Text { text: String::new() }));
    }

    #[test]
    fn memory_keys_open_what_their_public_half_sealed() {
        let keyboard = Keyboard::new();
        let Some(Reply::Bytes { bytes }) = answer(&keyboard, &Call::NewMemoryKeys) else {
            unreachable!("keys are always made");
        };
        let (secret, public) = bytes.split_at(32);
        let key: [u8; 32] = public.try_into().unwrap_or_default();
        let sealed = synthboard_seal::seal(&key, b"johnduff").unwrap_or_default();
        let call = Call::OpenMemory { sealed, secret: secret.to_vec() };
        assert_eq!(answer(&keyboard, &call), Some(Reply::Bytes { bytes: b"johnduff".to_vec() }));
    }

    #[test]
    fn assets_reach_the_engine() {
        let keyboard = Keyboard::new();
        answer(&keyboard, &Call::LoadLexicon { bytes: b"bonjour,120,1\n".to_vec() });
        assert!(keyboard.lock().lexicon().is_some());
        let bytes = "Smileys\t🙂\t\tsourire\n".as_bytes().to_vec();
        let Some(Reply::Emoji { groups }) = answer(&keyboard, &Call::LoadEmoji { bytes }) else {
            unreachable!("the catalogue always answers");
        };
        assert_eq!(groups.len(), 1);
    }

    #[test]
    fn a_call_that_does_not_decode_gets_no_answer() {
        let mut reply = Vec::new();
        handle(&Keyboard::new(), &[255, 1, 2], &mut reply);
        assert_eq!(reply, Vec::<u8>::new());
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
