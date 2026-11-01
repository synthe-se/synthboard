//! Binding between the Android shell and the Rust core. No logic lives here: it
//! decodes each call, stops panics at the boundary and encodes the reply. The
//! only `unsafe` in the project is in `jni`, which moves bytes, and `wipe`, which clears them.

#[cfg(not(fuzzing))]
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex, MutexGuard, Once, PoisonError};

use synthboard_core::codec::{Wire, decode};
use synthboard_core::{Catalog, EmojiGroup, Engine, Event, Field, Memory, NextWords, Step, wire_enum};
use synthboard_seal::KEY_BYTES;
use synthswipe::{Affinities, Grammar, Lexicon};
use zeroize::Zeroizing;

#[allow(unsafe_code)]
mod jni;
#[allow(unsafe_code)]
mod wipe;

/// A larger buffer came from loading assets, which happens once. The shell lets
/// go of its own at this same size.
pub const KEPT_BYTES: usize = 64 * 1024;

wire_enum! {
    /// One request from the shell.
    #[derive(Debug, Clone, PartialEq)]
    pub enum Call {
        Step { event: Event },
        MayRead { field: Field },
        SetTraceKey { hex: String },
        LoadLanguage { affinities: Vec<u8>, forms: Vec<u8>, next_words: Vec<u8> },
        LoadEmoji { bytes: Vec<u8> },
        LoadLexicon { bytes: Vec<u8> },
        LoadNeural { bytes: Vec<u8> },
        NewMemoryKeys,
        SetMemoryKey { key: Vec<u8> },
        LoadMemory { sealed: Option<Vec<u8>>, secret: Vec<u8> },
        SealMemory,
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

/// A panic message could quote what was typed, so none is ever printed. The
/// fuzzer finds its crashes through the panic hook, so it keeps its own.
fn silence_panics() {
    static ONCE: Once = Once::new();
    if cfg!(not(fuzzing)) {
        ONCE.call_once(|| std::panic::set_hook(Box::new(|_| {})));
    }
}

/// Runs `work`, turning a panic into the empty result so that the keyboard
/// process survives. The caller decides how to recover.
#[cfg(not(fuzzing))]
fn guarded<T: Default>(work: impl FnOnce() -> T) -> (T, bool) {
    catch_unwind(AssertUnwindSafe(work)).map_or_else(|_| (T::default(), true), |value| (value, false))
}

/// A panic is what the fuzzer looks for, so here it stays one.
#[cfg(fuzzing)]
fn guarded<T: Default>(work: impl FnOnce() -> T) -> (T, bool) {
    (work(), false)
}

/// Opens sealed memory and wipes the secret key given for it.
fn opened(sealed: &[u8], secret: Vec<u8>) -> Option<Zeroizing<Vec<u8>>> {
    let secret = Zeroizing::new(secret);
    let key = Zeroizing::new(<[u8; KEY_BYTES]>::try_from(secret.as_slice()).ok()?);
    synthboard_seal::open(&key, sealed).ok().map(Zeroizing::new)
}

/// Makes the key pair memory is sealed to: 32 secret bytes, then 32 public.
fn new_memory_keys() -> Vec<u8> {
    synthboard_seal::generate_keys().map_or_else(|_| Vec::new(), |(secret, public)| [secret, public].concat())
}

/// Seals a copy of memory to its key.
fn sealed_memory((memory, key): &(Arc<Memory>, [u8; KEY_BYTES])) -> Option<Vec<u8>> {
    let text = Zeroizing::new(memory.to_text().into_bytes());
    synthboard_seal::seal(key, &text).ok()
}

/// The plain text of saved memory, for an export the user asked for.
fn open_memory(sealed: &[u8], secret: Vec<u8>) -> Vec<u8> {
    opened(sealed, secret).map_or_else(Vec::new, |text| text.to_vec())
}

fn parse_lexicon(bytes: &[u8]) -> Lexicon {
    Lexicon::from_csv(&String::from_utf8_lossy(bytes))
}

impl Keyboard {
    #[allow(clippy::new_without_default)]
    #[must_use]
    pub fn new() -> Self {
        silence_panics();
        Self { engine: Mutex::new(Engine::new()) }
    }

    fn answer(&self, call: Call) -> Reply {
        match call {
            Call::Step { event } => Reply::Stepped { step: self.step(event) },
            Call::MayRead { field } => Reply::Flag { on: field.allows_reading() },
            Call::SetTraceKey { hex } => {
                self.set_trace_key(&hex);
                Reply::Done
            }
            Call::LoadLanguage { affinities, forms, next_words } => {
                self.load_language(&affinities, &forms, &next_words);
                Reply::Done
            }
            Call::LoadEmoji { bytes } => Reply::Emoji { groups: self.load_emoji(&bytes) },
            Call::LoadLexicon { bytes } => {
                self.load_lexicon(&bytes);
                Reply::Done
            }
            Call::LoadNeural { bytes } => {
                self.load_neural(&bytes);
                Reply::Done
            }
            Call::NewMemoryKeys => Reply::Bytes { bytes: new_memory_keys() },
            Call::SetMemoryKey { key } => {
                self.set_memory_key(key);
                Reply::Done
            }
            Call::LoadMemory { sealed, secret } => Reply::Flag { on: self.load_memory(sealed.as_deref(), secret) },
            Call::SealMemory => Reply::Bytes { bytes: self.seal_memory() },
            Call::OpenMemory { sealed, secret } => Reply::Bytes { bytes: open_memory(&sealed, secret) },
        }
    }

    /// Handles one event. After a panic the engine restarts empty, so taps
    /// keep working and the shell reopens it on the next `Shown` (X-9).
    fn step(&self, event: Event) -> Step {
        let mut engine = self.lock();
        let (step, panicked) = guarded(|| engine.step(event));
        if panicked {
            *engine = engine.recovered();
        }
        step
    }

    /// Allows traces, sealed for the owner of this public key, given as hex.
    /// Anything but a valid key leaves traces off.
    fn set_trace_key(&self, hex: &str) {
        if let Some(key) = synthboard_seal::from_hex(hex.trim()).and_then(|bytes| bytes.try_into().ok()) {
            self.lock().set_trace_key(key);
        }
    }

    /// Sets the public key that memory is sealed to. Anything else changes nothing.
    fn set_memory_key(&self, public: Vec<u8>) {
        if let Ok(key) = public.try_into() {
            self.lock().set_memory_key(key);
        }
    }

    /// Opens saved memory, which only exists in the clear on this side of the
    /// boundary, and gives it to the engine. `None` means there is none yet.
    /// Returns false if it cannot be opened: the shell must not leave it there,
    /// or nothing would ever be saved again. The engine's lock is not held
    /// meanwhile, so a long memory delays no tap.
    fn load_memory(&self, sealed: Option<&[u8]>, secret: Vec<u8>) -> bool {
        let read = self.lock().memory_reader();
        let (memory, panicked) = guarded(|| {
            let text = match sealed {
                Some(sealed) => opened(sealed, secret)?,
                None => Zeroizing::new(Vec::new()),
            };
            Some(read(&String::from_utf8_lossy(&text)))
        });
        match (memory, panicked) {
            (Some(memory), false) => {
                self.lock().adopt_memory(memory);
                true
            }
            _ => false,
        }
    }

    /// Seals memory the engine wants saved, outside its lock. Nothing to do
    /// gives nothing: a later save may already have taken it.
    fn seal_memory(&self) -> Vec<u8> {
        let job = self.lock().take_unsaved();
        job.as_ref().and_then(sealed_memory).unwrap_or_default()
    }

    /// The word the gesture under way would give, or nothing. The engine is
    /// locked only to copy the gesture, so taps never wait for this decode.
    fn preview(&self) -> String {
        guarded(|| {
            let job = self.lock().preview_job();
            job.map_or_else(String::new, |(decoding, path)| decoding.preview(&path))
        })
        .0
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

    /// Gives the model to the engine. A file that does not load leaves the geometry
    /// alone in charge. A build without the runtime ignores it.
    #[cfg_attr(not(feature = "onnx"), allow(clippy::unused_self))]
    fn load_neural(&self, bytes: &[u8]) {
        #[cfg(feature = "onnx")]
        {
            let (neural, panicked) = guarded(|| synthswipe_onnx::load(bytes).map(Arc::new));
            if let (Some(neural), false) = (neural, panicked) {
                self.lock().set_neural(neural);
            }
        }
        #[cfg(not(feature = "onnx"))]
        let _ = bytes;
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
    #[allow(clippy::panic)]
    fn a_panic_stops_at_the_boundary() {
        let (value, panicked) = guarded::<u32>(|| std::panic::panic_any("boom"));
        assert_eq!((value, panicked), (0, true));
    }

    #[test]
    fn no_saved_memory_opens_as_nothing_and_a_wrong_key_as_a_refusal() {
        assert!(opened(&[], vec![0; KEY_BYTES]).is_none());
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
        assert_eq!(answer(&keyboard, &Call::SetTraceKey { hex: String::new() }), Some(Reply::Done));
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
    fn memory_comes_back_from_what_was_sealed() {
        let (secret, public) = synthboard_seal::generate_keys().unwrap_or_default();
        let memory = Arc::new(Memory::parse("bonjour\t3\n", |_| true));
        let sealed = sealed_memory(&(memory, public)).unwrap_or_default();
        assert_eq!(open_memory(&sealed, secret.to_vec()), b"bonjour\t3\n");
    }

    #[test]
    fn a_step_answers_and_memory_says_whether_it_opened() {
        let keyboard = Keyboard::new();
        let shown = Event::Shown { field: Field::default(), before: String::new(), after: String::new(), cursor: 0 };
        let Some(Reply::Stepped { step }) = answer(&keyboard, &Call::Step { event: shown }) else {
            unreachable!("a step always answers");
        };
        assert_ne!(step.commands, Vec::new());
        let (secret, _) = synthboard_seal::generate_keys().unwrap_or_default();
        let load = |sealed| Call::LoadMemory { sealed, secret: secret.to_vec() };
        assert_eq!(answer(&keyboard, &load(None)), Some(Reply::Flag { on: true }));
        assert_eq!(answer(&keyboard, &load(Some(vec![1, 2]))), Some(Reply::Flag { on: false }));
    }

    #[test]
    fn no_gesture_previews_nothing() {
        assert_eq!(Keyboard::new().preview(), "");
    }

    #[test]
    fn nothing_to_save_seals_nothing() {
        assert_eq!(answer(&Keyboard::new(), &Call::SealMemory), Some(Reply::Bytes { bytes: Vec::new() }));
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
