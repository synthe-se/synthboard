//! The keyboard state machine. `step` is its only entry point, every timestamp
//! arrives with an event, and the one deadline is returned rather than armed.

use std::sync::Arc;

use boltffi::data;
use synthswipe::{Affinities, Grammar, Layout, Lexicon, Params, signature, SENTENCE_START, Sample, correct, decode_in_context, tokens};
use unicode_segmentation::UnicodeSegmentation;
use zeroize::Zeroize;

use crate::emoji::Catalog;
use crate::memory::Memory;
use crate::predict::NextWords;
use crate::board::{Action, Alphabet, BAR_DP, Board, Layer, Purpose, Screen, popup_of};
use crate::wire::{Command, DrawnKey, Haptic, Icon};

/// Android `InputType` and `EditorInfo` constants used for field rules.
mod android {
    pub const CLASS_MASK: u32 = 0xf;
    pub const CLASS_TEXT: u32 = 1;
    pub const CLASS_NUMBER: u32 = 2;
    pub const CLASS_PHONE: u32 = 3;
    pub const CLASS_DATETIME: u32 = 4;
    pub const VARIATION_MASK: u32 = 0xff0;
    pub const PASSWORD: u32 = 0x80;
    pub const VISIBLE_PASSWORD: u32 = 0x90;
    pub const WEB_PASSWORD: u32 = 0xe0;
    pub const URI: u32 = 0x10;
    pub const EMAIL: u32 = 0x20;
    pub const WEB_EMAIL: u32 = 0xd0;
    pub const CAP_CHARACTERS: u32 = 0x1000;
    pub const CAP_WORDS: u32 = 0x2000;
    pub const CAP_SENTENCES: u32 = 0x4000;
    pub const NO_SUGGESTIONS: u32 = 0x80000;
    pub const NO_PERSONALIZED_LEARNING: u32 = 0x100_0000;
}

const SLOP_KEYS: f32 = 0.5;
const FAST_TAP_MS: u64 = 500;
const FAST_TAP_SPAN_MS: f32 = 450.0;
const FAST_TAP_SLOP_KEYS: f32 = 2.0;
const LONG_PRESS_MS: u64 = 300;
const MENU_PRESS_MS: u64 = 450;
const DELETE_DELAY_MS: u64 = 400;
const DELETE_REPEAT_MS: u64 = 50;
/// Letters a held backspace removes before it goes word by word.
const DELETE_LETTERS: u32 = 2;
const DELETE_WORD_REPEAT_MS: u64 = 120;
const DOUBLE_TAP_MS: u64 = 300;
const DOUBLE_SPACE_MS: u64 = 1100;
const CURSOR_STEP_DP: f32 = 10.0;
const FAST_CURSOR_DP: f32 = 24.0;
const DELETE_SWIPE_DP: f32 = 16.0;
const DELETE_WORD_DP: f32 = 20.0;
const POPUP_ITEM_DP: f32 = 44.0;
const POPUP_LIFT_DP: f32 = 58.0;
const POPUP_BAND_DP: f32 = 63.36;
const SUGGESTIONS: usize = 4;
const MAX_RECENTS: usize = 32;
const SEARCH_CELLS: usize = 8;
const CONTEXT_CHARS: usize = 48;
const HISTORY_TOKENS: usize = 6;
/// A word the user keeps choosing outranks a more common one after two uses:
/// with `context_weight`, each use counts for about a nat at first.
const FAMILIAR_WEIGHT: f32 = 6.0;
/// How close to a personal trace, from 0 to 1, a gesture must be to lead.
const TRACE_LEADS: f32 = 0.35;
/// Matching traces that make a word lead however loose they are: the user has
/// chosen it twice for this gesture, and that decides.
const TRACE_VOTES: u32 = 2;
const FRENCH: u8 = 1;
const ENGLISH: u8 = 2;
/// Evidence kept from one word to the next.
const LANGUAGE_MEMORY: f32 = 0.8;
const LANGUAGE_LIMIT: f32 = 3.0;
/// Where a session starts: French by a little, the user's main language.
const LANGUAGE_START: f32 = 0.7;
/// Context points per unit of language evidence. With `context_weight`, the
/// full limit tilts a word by about 0.7 nat: enough to separate `il` from
/// `i'll`, never enough to exclude `merge` from a French sentence.
const LANGUAGE_TILT: f32 = 1.55;
/// Weight of subject and verb agreement. With `context_weight`, a verb that
/// agrees gains 1.5 nats and a word that cannot follow loses as much, enough to
/// beat a wrong frequency but not a clearly different gesture.
const GRAMMAR_WEIGHT: f32 = 10.0;
/// How far below the bar a release still confirms forgetting, in bar heights.
const FORGET_REACH: f32 = 1.5;
/// A habit counts this many times more than corpus evidence: it is the user's
/// own, and `context_weight` scales both.
const HABIT_WEIGHT: f32 = 6.0;
/// The most spatial cost an automatic correction may have: about one
/// neighbouring key, so that a word is never rewritten on weak evidence.
const AUTOCORRECT_COST: f32 = 3.0;

/// Rules derived from the text field.
#[data]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Field {
    pub input_type: u32,
    pub ime_options: u32,
}

impl Field {
    fn base_layer(self) -> Layer {
        match self.input_type & android::CLASS_MASK {
            android::CLASS_NUMBER | android::CLASS_DATETIME => Layer::Numbers,
            android::CLASS_PHONE => Layer::Phone,
            _ => Layer::Alpha,
        }
    }

    fn purpose(self) -> Purpose {
        if self.is_password() {
            return Purpose::Password;
        }
        if !self.is_text() {
            return Purpose::Text;
        }
        match self.variation() {
            android::URI => Purpose::Url,
            android::EMAIL | android::WEB_EMAIL => Purpose::Email,
            _ => Purpose::Text,
        }
    }

    fn is_text(self) -> bool {
        self.input_type & android::CLASS_MASK == android::CLASS_TEXT
    }

    fn variation(self) -> u32 {
        self.input_type & android::VARIATION_MASK
    }

    fn is_password(self) -> bool {
        self.is_text()
            && matches!(
                self.variation(),
                android::PASSWORD | android::VISIBLE_PASSWORD | android::WEB_PASSWORD
            )
    }

    fn is_address(self) -> bool {
        self.is_text()
            && matches!(self.variation(), android::URI | android::EMAIL | android::WEB_EMAIL)
    }

    fn allows_gestures(self) -> bool {
        self.is_text() && !self.is_password()
    }

    fn allows_auto_spacing(self) -> bool {
        self.is_text() && !self.is_password() && !self.is_address()
    }

    fn allows_suggestions(self) -> bool {
        self.allows_gestures() && self.input_type & android::NO_SUGGESTIONS == 0
    }

    /// Whether anything typed here may be remembered, in the learned words as
    /// in the traces. Addresses hold no language worth learning.
    #[must_use]
    pub fn allows_learning(self) -> bool {
        self.allows_suggestions()
            && !self.is_address()
            && self.ime_options & android::NO_PERSONALIZED_LEARNING == 0
    }

    /// Whether the keyboard may read this field's text at all.
    #[must_use]
    pub fn allows_reading(self) -> bool {
        !self.is_password()
    }
}

/// Something that happened. Every time is in milliseconds on one clock.
#[data]
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// The keyboard window has a new size.
    Resize { width: f32, screen_height: f32, density: f32 },
    /// The keyboard height setting changed, 1.0 being the usual height.
    HeightScale { scale: f32 },
    /// Erases everything learned, from the settings screen.
    ForgetAll,
    /// The keyboard opened on a field.
    Shown { field: Field, before: String, after: String, cursor: u32 },
    /// The keyboard closed.
    Hidden,
    Down { pointer: i32, x: f32, y: f32, t: u64 },
    Move { pointer: i32, x: f32, y: f32, t: u64 },
    Up { pointer: i32, x: f32, y: f32, t: u64 },
    /// The system took the touch away.
    Cancel,
    /// The deadline returned by the last step has passed.
    Deadline { t: u64 },
    /// The field's selection changed.
    Selection { start: u32, end: u32 },
    /// A screen reader's finger moved over the keyboard.
    Hover { x: f32, y: f32 },
    /// Answer to `Command::RequestContext`.
    Context { before: String, after: String },
    /// Text chosen outside the keys, such as an emoji.
    Text { text: String },
    /// Removes one grapheme, from the emoji panel.
    DeleteOne,
    /// Provides the recent emoji saved earlier.
    Recents { items: Vec<String> },
    /// Provides what was learned earlier.
    Memory { text: String },
    /// The language data finished loading, so the bar can fill.
    LanguageReady,
    /// A keyboard session began, at this Unix time in seconds.
    Session { unix: u64 },
    /// The emoji panel's search row was tapped.
    OpenEmojiSearch,
    /// The emoji panel was closed from inside.
    CloseEmojiPanel,
}

/// The result of one event.
#[data]
#[derive(Debug, Default, PartialEq)]
pub struct Step {
    pub commands: Vec<Command>,
    /// When to send `Event::Deadline`. `None` whenever no finger is down.
    pub deadline: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Shift {
    #[default]
    Off,
    Auto,
    OneShot,
    Locked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Spacing {
    #[default]
    None,
    /// A space owed before the next word, not yet written (P-1).
    Phantom,
    /// A real space written by the space bar or a double-space candidate.
    Weak,
}

/// The word being tapped, with where each letter was touched.
#[derive(Debug, Clone, Default)]
struct Typing {
    text: String,
    taps: Vec<[f32; 2]>,
    /// What the bar offers, in slot order: centre, left, right.
    offered: Vec<String>,
}

/// A correction that one backspace can still undo (B-9).
#[derive(Debug, Clone)]
struct Corrected {
    typed: String,
    written: String,
}

/// The word just committed, which stays replaceable (B-5).
#[derive(Debug, Clone)]
struct LastWord {
    text: String,
    alternatives: Vec<String>,
    spacing_after: Spacing,
    /// The gesture that wrote this word, if a swipe did.
    gesture: Vec<Sample>,
}

#[derive(Debug)]
enum Touch {
    Idle,
    Bar { x: f32 },
    SearchRow { x: f32 },
    /// The "forget this word" bubble is open over the bar.
    Forget { word: String },
    Key { key: usize, down: (f32, f32), slop: f32, path: Vec<Sample>, start: u64 },
    Gesture { path: Vec<Sample>, start: u64 },
    Popup { items: Vec<String>, selected: Option<usize>, x: f32, y: f32, path: Vec<Sample>, start: u64, down_y: f32 },
    Cursor { anchor: f32 },
    Repeat { count: u32 },
    DeleteSwipe { anchor: f32, words: u32, base_cursor: u32 },
    ShiftDrag { prior: Shift },
}

/// The whole keyboard.
#[derive(Debug)]
pub struct Engine {
    lexicon: Option<Arc<Lexicon>>,
    affinities: Option<Arc<Affinities>>,
    grammar: Option<Arc<Grammar>>,
    /// The owner's public key. Without one, no trace can exist.
    trace_key: Option<[u8; 32]>,
    layout_traced: bool,
    next_words: Option<Arc<NextWords>>,
    predictions: Vec<String>,
    /// The gesture of a word just deleted: the next word is what it meant.
    failed_gesture: Option<Vec<Sample>>,
    memory: Arc<Memory>,
    /// The session's language: above 0 French, below 0 English. Reset each
    /// time the keyboard opens or the field empties, and never stored.
    leaning: f32,
    /// The last word and the word before it, learned once the next one starts.
    pending: Option<(String, String)>,
    memory_changed: bool,
    typing: Option<Typing>,
    corrected: Option<Corrected>,
    /// Words the user restored after a correction: left alone this session.
    protected: Vec<String>,
    tap_point: [f32; 2],
    catalog: Option<Arc<Catalog>>,
    recents: Vec<String>,
    search: Option<String>,
    params: Params,
    alphabet: Alphabet,
    layer: Layer,
    height_scale: f32,
    board: Option<Board>,
    width: f32,
    screen_height: f32,
    density: f32,
    field: Field,
    shift: Shift,
    spacing: Spacing,
    before: String,
    /// The text after the cursor, to step over it by whole characters.
    after: String,
    hovered: Option<usize>,
    cursor: u32,
    expected_selection: Option<u32>,
    selection: (u32, u32),
    last_word: Option<LastWord>,
    touch: Touch,
    pointer: Option<i32>,
    deadline: Option<u64>,
    pressed: i32,
    last_up: Option<u64>,
    last_shift_tap: Option<u64>,
    last_space_tap: Option<u64>,
    out: Vec<Command>,
}

fn utf16_len(text: &str) -> u32 {
    u32::try_from(text.encode_utf16().count()).unwrap_or(u32::MAX)
}

/// How far a finger must travel to start a gesture, in key widths. Right after
/// a tap the finger is still typing, so a swipe needs more room (G-3). `since`
/// is the time since the last tap, if there was one.
fn gesture_slop(since: Option<u64>) -> f32 {
    match since {
        Some(elapsed) if elapsed < FAST_TAP_MS => {
            let ratio = (elapsed as f32 / FAST_TAP_SPAN_MS).min(1.0);
            FAST_TAP_SLOP_KEYS + (SLOP_KEYS - FAST_TAP_SLOP_KEYS) * ratio
        }
        _ => SLOP_KEYS,
    }
}

/// Writes `word` in the same case as `model`.
fn case_like(word: &str, model: &str) -> String {
    let capitalised = model.chars().next().is_some_and(char::is_uppercase);
    let all_upper = model.chars().count() > 1 && model.chars().all(|c| !c.is_lowercase());
    if all_upper {
        word.to_uppercase()
    } else if capitalised {
        title_case(word)
    } else {
        word.to_owned()
    }
}

fn sample_at(x: f32, y: f32, t: u64, start: u64) -> Sample {
    let t_ms = u32::try_from(t.saturating_sub(start)).unwrap_or(u32::MAX);
    Sample { x, y, t_ms }
}

fn title_case(word: &str) -> String {
    let mut letters = word.chars();
    letters.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(letters).collect()
    })
}

fn opens_sentence(before: &str) -> bool {
    let trimmed = before.trim_end_matches([' ', '\u{a0}', '(', '[', '«', '"', '“']);
    if trimmed.is_empty() || trimmed.ends_with('\n') {
        return true;
    }
    if !trimmed.ends_with(['.', '!', '?', '…']) || trimmed.len() == before.len() {
        return false;
    }
    let last_word = trimmed.rsplit(char::is_whitespace).next().unwrap_or("");
    let lower = last_word.to_lowercase();
    !matches!(lower.as_str(), "m." | "mme." | "dr." | "etc." | "e.g." | "i.e.")
        && !last_word.chars().rev().nth(1).is_some_and(|c| c.is_ascii_digit())
}

/// English first-person forms take a capital wherever they stand.
fn is_english_i(word: &str) -> bool {
    matches!(word, "i" | "i'm" | "i'll" | "i've" | "i'd" | "i’m" | "i’ll" | "i’ve" | "i’d")
}

fn cased(shift: Shift, word: &str) -> String {
    if shift == Shift::Off && is_english_i(word) {
        return title_case(word);
    }
    match shift {
        Shift::Locked => word.to_uppercase(),
        Shift::Auto | Shift::OneShot => title_case(word),
        Shift::Off => word.to_owned(),
    }
}

/// A gesture decoder detached from the engine. The final word and the live
/// preview both go through `rank`, so they can never disagree.
#[derive(Debug, Clone)]
pub struct Decoding {
    lexicon: Arc<Lexicon>,
    affinities: Option<Arc<Affinities>>,
    grammar: Option<Arc<Grammar>>,
    memory: Arc<Memory>,
    layout: Layout,
    params: Params,
    previous: String,
    history: Vec<String>,
    leaning: f32,
    shift: Shift,
}

impl Decoding {
    /// Everything the sentence says about a word: corpus affinity, agreement,
    /// the user's habit and the session's language. Swipes and taps share it.
    fn score(&self, previous: &str, history: &[String], word: &str) -> f32 {
        let table = self.affinities.as_ref().map_or(0.0, |table| table.score(previous, word));
        let agreement = self.grammar.as_ref().map_or(0.0, |grammar| GRAMMAR_WEIGHT * grammar.score(history, word));
        let habit = HABIT_WEIGHT * self.memory.habit(previous, word)
            + FAMILIAR_WEIGHT * self.memory.familiarity(word);
        table + agreement + habit + language_tilt(self.lexicon.languages(word), self.leaning)
    }

    fn context(&self, word: &str) -> f32 {
        self.score(&self.previous, &self.history, word)
    }

    fn rank(&self, path: &[Sample]) -> Vec<String> {
        let context = |word: &str| self.context(word);
        let extra = self.memory.extra_words();
        let mut ranked =
            decode_in_context(&self.layout, &self.lexicon, path, &self.params, SUGGESTIONS, context).unwrap_or_default();
        if let Some(extra) = &extra {
            ranked.extend(decode_in_context(&self.layout, extra, path, &self.params, SUGGESTIONS, context).unwrap_or_default());
            ranked.sort_by(|a, b| b.score.total_cmp(&a.score).then_with(|| a.surface.cmp(b.surface)));
            ranked.truncate(SUGGESTIONS);
        }
        let mut words: Vec<String> = ranked
            .iter()
            .map(|candidate| candidate.surface.to_owned())
            .filter(|word| !self.memory.is_blocked(word))
            .collect();
        // A personal trace is a light vote: a close one takes the lead, and
        // a loose one only secures a place among the alternatives.
        let gesture = signature(path, self.layout.pitch());
        for (word, strength, count) in self.memory.trace_matches(&gesture).into_iter().rev() {
            words.retain(|known| *known != word);
            let leads = strength >= TRACE_LEADS || count >= TRACE_VOTES;
            words.insert(usize::from(!leads).min(words.len()), word);
        }
        words.truncate(SUGGESTIONS);
        words
    }

    /// The word this gesture would give so far, in the case it would be written.
    #[must_use]
    pub fn preview(&self, path: &[Sample]) -> String {
        self.rank(path).first().map_or_else(String::new, |word| cased(self.shift, word))
    }
}

/// How the session's language tilts a word: only a word of one language moves.
fn language_tilt(languages: u8, leaning: f32) -> f32 {
    match languages {
        FRENCH => LANGUAGE_TILT * leaning,
        ENGLISH => -LANGUAGE_TILT * leaning,
        _ => 0.0,
    }
}

fn icon_of(action: &Action) -> Icon {
    match action {
        Action::Shift => Icon::Shift,
        Action::Delete => Icon::Delete,
        Action::Enter => Icon::Enter,
        Action::Emoji => Icon::Emoji,
        _ => Icon::None,
    }
}

fn attaches_to_word(text: &str) -> bool {
    matches!(text, "," | "." | ")" | "]" | "}")
}

impl Engine {
    #[must_use]
    pub fn new() -> Self {
        Self {
            lexicon: None,
            affinities: None,
            grammar: None,
            trace_key: None,
            layout_traced: false,
            next_words: None,
            predictions: Vec::new(),
            failed_gesture: None,
            memory: Arc::new(Memory::default()),
            leaning: LANGUAGE_START,
            pending: None,
            memory_changed: false,
            typing: None,
            corrected: None,
            protected: Vec::new(),
            tap_point: [0.0, 0.0],
            catalog: None,
            recents: Vec::new(),
            search: None,
            params: Params::default(),
            alphabet: Alphabet::default(),
            layer: Layer::default(),
            height_scale: 1.0,
            board: None,
            width: 0.0,
            screen_height: 0.0,
            density: 1.0,
            field: Field::default(),
            shift: Shift::default(),
            spacing: Spacing::default(),
            before: String::new(),
            after: String::new(),
            hovered: None,
            cursor: 0,
            expected_selection: None,
            selection: (0, 0),
            last_word: None,
            touch: Touch::Idle,
            pointer: None,
            deadline: None,
            pressed: -1,
            last_up: None,
            last_shift_tap: None,
            last_space_tap: None,
            out: Vec::new(),
        }
    }

    /// Provides the lexicon. Taps work before it arrives (X-9).
    pub fn set_lexicon(&mut self, lexicon: Arc<Lexicon>) {
        self.lexicon = Some(lexicon);
    }

    /// Provides the context table and the next-word list.
    pub fn set_language(&mut self, affinities: Arc<Affinities>, grammar: Arc<Grammar>, next_words: Arc<NextWords>) {
        self.affinities = Some(affinities);
        self.grammar = Some(grammar);
        self.next_words = Some(next_words);
    }

    /// Allows traces, sealed for the owner of this public key.
    pub fn set_trace_key(&mut self, key: [u8; 32]) {
        self.trace_key = Some(key);
    }

    /// Provides the emoji catalogue.
    pub fn set_catalog(&mut self, catalog: Arc<Catalog>) {
        self.catalog = Some(catalog);
    }

    /// The lexicon in use, kept across a restart of the engine.
    #[must_use]
    pub fn lexicon(&self) -> Option<Arc<Lexicon>> {
        self.lexicon.clone()
    }

    /// Handles one event.
    pub fn step(&mut self, event: Event) -> Step {
        match event {
            Event::Resize { width, screen_height, density } => {
                self.layout_traced = false;
                self.width = width;
                self.screen_height = screen_height;
                self.density = density;
                self.rebuild();
            }
            Event::Hover { x, y } => self.hover(x, y),
            Event::HeightScale { scale } => {
                if (scale - self.height_scale).abs() > f32::EPSILON {
                    self.height_scale = scale;
                    self.rebuild();
                }
            }
            Event::ForgetAll => {
                self.memory = Arc::new(Memory::default());
                self.pending = None;
                self.recents.clear();
                self.protected.clear();
                self.clear_suggestions();
            }
            Event::Shown { field, before, after, cursor } => {
                self.after = if field.allows_reading() { after } else { String::new() };
                self.shown(field, before, cursor);
            }
            Event::Hidden => self.hidden(),
            Event::Down { pointer, x, y, t } => self.down(pointer, x, y, t),
            Event::Move { pointer, x, y, t } => {
                if self.pointer == Some(pointer) {
                    self.moved(x, y, t);
                }
            }
            Event::Up { pointer, x, y, t } => {
                if self.pointer == Some(pointer) {
                    self.lifted(x, y, t);
                }
            }
            Event::Cancel => self.cancel(),
            Event::Deadline { t } => self.deadline_passed(t),
            Event::Selection { start, end } => self.selection_changed(start, end),
            Event::Context { before, after } => {
                if self.field.allows_reading() {
                    self.before = before;
                    self.after = after;
                }
                self.refresh_shift();
            }
            Event::Text { text } => self.pick(&text),
            Event::DeleteOne => self.delete_one(),
            Event::Recents { items } => self.recents = items,
            Event::Session { unix } => self.trace(format!("S\t{unix}")),
            Event::LanguageReady => {
                if self.typing.is_none() && self.last_word.is_none() {
                    self.clear_suggestions();
                }
            }
            Event::Memory { text } => {
                self.memory = Arc::new(Memory::parse(&text, self.known()));
            }
            Event::OpenEmojiSearch => self.open_search(),
            Event::CloseEmojiPanel => self.search = None,
        }
        self.after_step()
    }

    fn after_step(&mut self) -> Step {
        if matches!(self.touch, Touch::Idle) {
            self.deadline = None;
        }
        Step {
            commands: std::mem::take(&mut self.out),
            deadline: self.deadline,
        }
    }

    fn rebuild(&mut self) {
        if self.width <= 0.0 {
            return;
        }
        let screen = Screen { width: self.width, height: self.screen_height, density: self.density };
        let board = Board::new(self.alphabet, self.layer, self.field.purpose(), screen, self.height_scale);
        let shifted = self.shift != Shift::Off;
        let keys = board
            .keys
            .iter()
            .map(|key| DrawnKey {
                x: key.x,
                y: key.y,
                width: key.width,
                height: key.height,
                label: match icon_of(&key.action) {
                    Icon::None if shifted => key.label.to_uppercase(),
                    Icon::None => key.label.clone(),
                    _ => String::new(),
                },
                hint: key.hint.clone(),
                icon: icon_of(&key.action),
                is_pill: matches!(key.action, Action::Enter | Action::Layer(_)),
                is_function: !matches!(key.action, Action::Text(_) | Action::Space),
            })
            .collect();
        self.out.push(Command::Keys {
            height: board.height,
            bar_height: self.bar_height(),
            keys,
        });
        self.board = Some(board);
        self.send_look();
    }

    fn send_look(&mut self) {
        let shift = match self.shift {
            Shift::Off => 0,
            Shift::Auto | Shift::OneShot => 1,
            Shift::Locked => 2,
        };
        self.out.push(Command::Look { pressed: self.pressed, shift });
    }

    fn shown(&mut self, field: Field, before: String, cursor: u32) {
        self.field = field;
        self.leaning = LANGUAGE_START;
        self.before = if field.allows_reading() { before } else { String::new() };
        self.cursor = cursor;
        self.selection = (cursor, cursor);
        self.expected_selection = None;
        self.end_word();
        self.layer = field.base_layer();
        self.touch = Touch::Idle;
        self.pointer = None;
        self.shift = Shift::Off;
        self.refresh_shift();
        self.rebuild();
        self.clear_suggestions();
    }

    fn hidden(&mut self) {
        self.cancel();
        self.finish_typing_literal();
        self.confirm_pending();
        self.save_memory();
        self.forget_text();
        if self.search.take().is_some() {
            self.out.push(Command::EmojiSearch { on: false, query: String::new(), results: Vec::new() });
        }
        self.out.push(Command::EmojiPanel { on: false, recents: Vec::new() });
    }

    /// Forgets the word in progress and any space owed after it.
    fn end_word(&mut self) {
        self.spacing = Spacing::None;
        self.last_word = None;
        self.typing = None;
        self.corrected = None;
    }

    /// Wipes everything this session read or typed, once the keyboard is closed.
    fn forget_text(&mut self) {
        self.before.zeroize();
        self.after.zeroize();
        self.protected.zeroize();
        if let Some(query) = self.search.as_mut() {
            query.zeroize();
        }
        self.search = None;
        self.predictions.clear();
        self.corrected = None;
        self.failed_gesture = None;
        self.typing = None;
        self.last_word = None;
    }

    fn cancel(&mut self) {
        if matches!(self.touch, Touch::Gesture { .. }) {
            self.out.push(Command::Trail { on: false });
        }
        if matches!(self.touch, Touch::Popup { .. }) {
            self.hide_popup();
        }
        self.touch = Touch::Idle;
        self.pointer = None;
        self.press(-1);
    }

    fn press(&mut self, key: i32) {
        if self.pressed != key {
            self.pressed = key;
            self.send_look();
        }
    }

    fn hide_popup(&mut self) {
        self.out.push(Command::Popup {
            x: 0.0,
            y: 0.0,
            item_width: 0.0,
            selected: -1,
            items: Vec::new(),
        });
    }

    /// The word before the cursor, or the sentence start.
    fn previous_token(&self) -> String {
        if opens_sentence(&self.before) {
            return SENTENCE_START.to_owned();
        }
        let tail: String = {
            let mut letters: Vec<char> = self.before.chars().rev().take(CONTEXT_CHARS).collect();
            letters.reverse();
            letters.into_iter().collect()
        };
        tokens(&tail).pop().unwrap_or_else(|| SENTENCE_START.to_owned())
    }

    /// Asks whether the lexicon has a word, independently of the engine.
    fn known(&self) -> impl Fn(&str) -> bool + use<> {
        let lexicon = self.lexicon.clone();
        move |word| lexicon.as_ref().is_some_and(|words| words.contains(word))
    }

    /// Learns the waiting word, now that it was kept.
    fn confirm_pending(&mut self) {
        let Some((previous, word)) = self.pending.take() else {
            return;
        };
        let known = self.known();
        Arc::make_mut(&mut self.memory).learn(&previous, &word, known);
        self.memory_changed = true;
    }

    /// Files a gesture as the user's own trace for the word it really meant.
    fn teach(&mut self, word: &str, gesture: &[Sample]) {
        let Some(pitch) = self.board.as_ref().and_then(|b| b.layout.as_ref()).map(Layout::pitch) else {
            return;
        };
        if self.field.allows_learning() && gesture.len() >= 2 {
            Arc::make_mut(&mut self.memory).add_trace(word, signature(gesture, pitch));
            self.memory_changed = true;
            // A chat app keeps the keyboard open for hours, so waiting for it
            // to close would lose what the user just taught.
            self.save_memory();
        }
    }

    fn update_leaning(&mut self, word: &str) {
        let languages = self.lexicon.as_ref().map_or(0, |lexicon| lexicon.languages(word));
        let evidence = match languages {
            FRENCH => 1.0,
            ENGLISH => -1.0,
            _ => return,
        };
        self.leaning = (self.leaning * LANGUAGE_MEMORY + evidence).clamp(-LANGUAGE_LIMIT, LANGUAGE_LIMIT);
    }

    /// Queues a finished word for learning, and learns the one before it.
    fn remember(&mut self, previous: String, word: &str) {
        self.update_leaning(word);
        if let Some(failed) = self.failed_gesture.take() {
            self.trace(format!("F\t{word}"));
            self.teach(word, &failed);
        }
        self.confirm_pending();
        if self.field.allows_learning() {
            self.pending = Some((previous, word.to_lowercase()));
        }
    }

    fn save_memory(&mut self) {
        if std::mem::take(&mut self.memory_changed) {
            self.out.push(Command::SaveMemory { text: self.memory.to_text() });
        }
    }

    /// Fills the bar with what comes next, or empties it inside a word.
    fn clear_suggestions(&mut self) {
        self.refresh_shift();
        let between_words = self.spacing == Spacing::Phantom
            || self.before.is_empty()
            || self.before.ends_with(|c: char| c.is_whitespace() || matches!(c, '\'' | '’'));
        self.predictions = match (&self.next_words, between_words && self.field.allows_suggestions()) {
            (Some(next_words), true) => {
                let previous = self.previous_token();
                let mut words = self.memory.next_after(&previous);
                for word in next_words.after(&previous) {
                    if !words.contains(&word) {
                        words.push(word);
                    }
                }
                words.retain(|word| !self.memory.is_blocked(word));
                words.truncate(3);
                words
            }
            _ => Vec::new(),
        };
        let cased: Vec<String> = self.predictions.iter().map(|word| self.apply_case(word)).collect();
        self.predictions = cased;
        let shown = self.predictions.clone();
        self.show_alternatives(&shown);
    }

    fn refresh_shift(&mut self) {
        if self.shift == Shift::Locked {
            return;
        }
        let flags = self.field.input_type;
        let auto = self.field.is_text()
            && !self.field.is_password()
            && !self.field.is_address()
            && (flags & android::CAP_CHARACTERS != 0
                || (flags & android::CAP_SENTENCES != 0 && opens_sentence(&self.before))
                || (flags & android::CAP_WORDS != 0
                    && self.before.chars().next_back().is_none_or(char::is_whitespace)));
        let next = if auto { Shift::Auto } else { Shift::Off };
        if self.shift != next && self.shift != Shift::OneShot {
            self.set_shift(next);
        }
    }

    fn set_shift(&mut self, shift: Shift) {
        let relabel = (self.shift == Shift::Off) != (shift == Shift::Off);
        self.shift = shift;
        if relabel {
            self.rebuild();
        } else {
            self.send_look();
        }
    }

    fn consume_shift(&mut self) {
        if matches!(self.shift, Shift::Auto | Shift::OneShot) {
            self.set_shift(Shift::Off);
        }
    }

    fn key(&self, index: usize) -> Option<&crate::board::Key> {
        self.board.as_ref().and_then(|board| board.keys.get(index))
    }

    fn bar_height(&self) -> f32 {
        BAR_DP * self.density
    }

    fn down(&mut self, pointer: i32, x: f32, y: f32, t: u64) {
        if self.pointer.is_some() {
            if !matches!(self.touch, Touch::Key { .. }) {
                return;
            }
            // Rollover: a second finger completes the first tap (G-4).
            if let Touch::Key { key, down, .. } = self.touch {
                self.tap(key, down, t);
            }
            self.touch = Touch::Idle;
        }
        self.pointer = Some(pointer);
        if y < 0.0 {
            self.touch = Touch::SearchRow { x };
            return;
        }
        if y < self.bar_height() {
            self.touch = Touch::Bar { x };
            if self.slot_word(x).is_some() {
                self.deadline = Some(t + LONG_PRESS_MS);
            }
            return;
        }
        let Some(key) = self.board.as_ref().and_then(|board| board.key_at(x, y)) else {
            return;
        };
        self.out.push(Command::Vibrate { kind: Haptic::Tap });
        self.press(i32::try_from(key).unwrap_or(-1));
        let action = self.key(key).map(|k| k.action.clone());
        self.deadline = match action {
            Some(Action::Delete) => Some(t + DELETE_DELAY_MS),
            Some(Action::Space | Action::Shift) => Some(t + MENU_PRESS_MS),
            Some(Action::Text(_)) => Some(t + LONG_PRESS_MS),
            _ => None,
        };
        let width = self.board.as_ref().map_or(1.0, |board| board.key_width);
        let slop = gesture_slop(self.last_up.map(|last| t.saturating_sub(last))) * width;
        self.touch = Touch::Key {
            key,
            down: (x, y),
            slop,
            path: vec![Sample { x, y, t_ms: 0 }],
            start: t,
        };
        if action == Some(Action::Shift) {
            let prior = self.shift;
            if self.last_shift_tap.is_some_and(|last| t.saturating_sub(last) < DOUBLE_TAP_MS) {
                self.set_shift(Shift::Locked);
                self.last_shift_tap = None;
                self.touch = Touch::ShiftDrag { prior: Shift::Locked };
                self.deadline = None;
            } else {
                self.touch = Touch::ShiftDrag { prior };
            }
        }
    }

    fn moved(&mut self, x: f32, y: f32, t: u64) {
        let density = self.density;
        match &mut self.touch {
            Touch::Key { path, start, .. } | Touch::Popup { path, start, .. } | Touch::Gesture { path, start } => {
                path.push(sample_at(x, y, t, *start));
            }
            Touch::Cursor { anchor } => {
                let step = CURSOR_STEP_DP * density;
                let steps = ((x - *anchor) / step).trunc() as i32;
                if steps != 0 {
                    *anchor += steps as f32 * step;
                    self.move_cursor(steps);
                }
            }
            Touch::DeleteSwipe { anchor, words, base_cursor } => {
                let travel = ((*anchor - x) - DELETE_SWIPE_DP * density).max(0.0);
                let wanted = (travel / (DELETE_WORD_DP * density)) as u32 + 1;
                if wanted != *words {
                    *words = wanted;
                    let base = *base_cursor;
                    self.select_words(wanted, base);
                }
            }
            Touch::ShiftDrag { .. }
            | Touch::Idle
            | Touch::Bar { .. }
            | Touch::SearchRow { .. }
            | Touch::Forget { .. }
            | Touch::Repeat { .. } => {}
        }
        match self.touch {
            Touch::Key { .. } => self.moved_on_key(x, y, t),
            Touch::Popup { .. } => self.moved_in_popup(x, y),
            _ => {}
        }
    }

    fn can_gesture(&self) -> bool {
        self.layer == Layer::Alpha && self.field.allows_gestures()
    }

    /// Decides what a finger still on its first key has become.
    fn moved_on_key(&mut self, x: f32, y: f32, t: u64) {
        let Touch::Key { key, down, slop, start, .. } = self.touch else {
            return;
        };
        let (dx, dy) = (x - down.0, y - down.1);
        let fast = self.last_up.is_some_and(|last| t.saturating_sub(last) < FAST_TAP_MS);
        match self.key(key).map(|k| k.action.clone()) {
            Some(Action::Space) => {
                let threshold = if fast { FAST_CURSOR_DP } else { CURSOR_STEP_DP } * self.density;
                if dx.abs() >= threshold {
                    self.touch = Touch::Cursor { anchor: down.0 };
                    self.deadline = None;
                    self.press(-1);
                    self.end_word();
                }
            }
            Some(Action::Delete) => {
                if -dx >= DELETE_SWIPE_DP * self.density {
                    self.touch = Touch::DeleteSwipe { anchor: down.0, words: 0, base_cursor: self.cursor };
                    self.deadline = None;
                }
            }
            Some(Action::Text(text))
                if self.can_gesture() && text.chars().all(char::is_alphabetic) && dx.hypot(dy) >= slop =>
            {
                if let Touch::Key { path, .. } = std::mem::replace(&mut self.touch, Touch::Idle) {
                    self.begin_gesture(path, start);
                }
            }
            _ => {}
        }
    }

    /// Follows the finger along an open popup, or rescues a slow start (A-9).
    fn moved_in_popup(&mut self, x: f32, y: f32) {
        let can_gesture = self.can_gesture();
        let slop = SLOP_KEYS * self.board.as_ref().map_or(1.0, |board| board.key_width);
        let item = POPUP_ITEM_DP * self.density;
        let band = POPUP_BAND_DP * self.density;
        let Touch::Popup { items, selected, x: popup_x, y: popup_y, path, down_y, .. } = &mut self.touch else {
            return;
        };
        let count = items.len() as f32;
        let inside = x >= *popup_x - band && x <= *popup_x + item * count + band && y <= *down_y + band;
        // A digit-only bubble has nothing to slide to, so any real travel means
        // the finger was starting a gesture.
        let travel = path.first().map_or(0.0, |first| (x - first.x).hypot(y - first.y));
        let left_lone_item = items.len() == 1 && travel >= slop;
        let hovered = (inside && !left_lone_item)
            .then(|| (((x - *popup_x) / item).floor().max(0.0) as usize).min(items.len().saturating_sub(1)));
        if hovered.is_none() && !selected.is_some_and(|index| index != 0) && can_gesture {
            if let Touch::Popup { path, start, .. } = std::mem::replace(&mut self.touch, Touch::Idle) {
                self.hide_popup();
                self.begin_gesture(path, start);
            }
            return;
        }
        if *selected != hovered {
            *selected = hovered;
            let command = Command::Popup {
                x: *popup_x,
                y: *popup_y,
                item_width: item,
                selected: hovered.map_or(-1, |index| i32::try_from(index).unwrap_or(-1)),
                items: items.clone(),
            };
            self.out.push(command);
        }
    }

    fn begin_gesture(&mut self, path: Vec<Sample>, start: u64) {
        self.deadline = None;
        self.press(-1);
        self.out.push(Command::Vibrate { kind: Haptic::GestureStart });
        self.out.push(Command::Trail { on: true });
        self.touch = Touch::Gesture { path, start };
    }

    fn lifted(&mut self, x: f32, y: f32, t: u64) {
        self.pointer = None;
        self.press(-1);
        match std::mem::replace(&mut self.touch, Touch::Idle) {
            Touch::Key { key, down, .. } => {
                self.tap(key, down, t);
                self.last_up = Some(t);
            }
            Touch::Gesture { path, .. } => {
                self.out.push(Command::Trail { on: false });
                self.finish_gesture(&path);
            }
            Touch::Popup { items, selected, .. } => {
                self.hide_popup();
                if let Some(text) = selected.and_then(|index| items.into_iter().nth(index)) {
                    self.type_text(&text);
                }
            }
            Touch::Bar { x } => self.tap_suggestion(x),
            Touch::SearchRow { x } => self.tap_search_row(x),
            Touch::Forget { word } => {
                self.hide_popup();
                if y < self.bar_height() * FORGET_REACH {
                    self.forget(&word);
                }
            }
            Touch::DeleteSwipe { words, .. } => {
                if words > 0 {
                    self.out.push(Command::DeleteSelection);
                    self.out.push(Command::RequestContext);
                }
            }
            Touch::ShiftDrag { prior } => self.release_shift(prior, x, y, t),
            Touch::Cursor { .. } | Touch::Repeat { .. } | Touch::Idle => {
                self.refresh_shift();
            }
        }
    }

    fn release_shift(&mut self, prior: Shift, x: f32, y: f32, t: u64) {
        let target = self
            .board
            .as_ref()
            .and_then(|board| board.key_at(x, y))
            .filter(|&index| self.key(index).is_some_and(|key| matches!(key.action, Action::Text(_))));
        if let Some(index) = target {
            // Slide from shift: one capital, then the prior state (C-3).
            if let Some(Action::Text(text)) = self.key(index).map(|key| key.action.clone()) {
                let capital = title_case(&text);
                self.insert_text(&capital);
                self.set_shift(prior);
            }
            return;
        }
        if prior == Shift::Locked && self.shift == Shift::Locked && self.last_shift_tap.is_none() {
            // The double tap that locked it has just lifted.
            self.last_shift_tap = Some(0);
            return;
        }
        self.last_shift_tap = Some(t);
        self.set_shift(match self.shift {
            Shift::Off => Shift::OneShot,
            Shift::OneShot | Shift::Auto | Shift::Locked => Shift::Off,
        });
    }

    fn deadline_passed(&mut self, t: u64) {
        if self.deadline.is_none_or(|due| t < due) {
            return;
        }
        self.deadline = None;
        match &self.touch {
            Touch::Key { key, .. } => {
                let key = *key;
                match self.key(key).map(|k| k.action.clone()) {
                    Some(Action::Delete) => {
                        self.pending = None;
                        self.delete_one();
                        self.out.push(Command::Vibrate { kind: Haptic::Step });
                        self.touch = Touch::Repeat { count: 1 };
                        self.deadline = Some(t + DELETE_REPEAT_MS);
                    }
                    Some(Action::Space) => {
                        self.out.push(Command::Vibrate { kind: Haptic::LongPress });
                        self.out.push(Command::Picker);
                        self.cancel();
                    }
                    Some(Action::Text(text)) => self.open_popup(key, &text),
                    _ => {}
                }
            }
            Touch::Bar { x } => {
                let x = *x;
                if let Some(word) = self.slot_word(x) {
                    let width = self.width / 2.0;
                    self.out.push(Command::Vibrate { kind: Haptic::LongPress });
                    self.out.push(Command::ForgetBubble {
                        x: (x - width / 2.0).clamp(0.0, self.width - width),
                        width,
                        word: word.clone(),
                    });
                    self.touch = Touch::Forget { word };
                }
            }
            Touch::Repeat { count } => {
                let count = *count;
                if count < DELETE_LETTERS {
                    self.delete_one();
                    self.deadline = Some(t + DELETE_REPEAT_MS);
                } else {
                    self.delete_word();
                    self.deadline = Some(t + DELETE_WORD_REPEAT_MS);
                }
                self.out.push(Command::Vibrate { kind: Haptic::Step });
                self.touch = Touch::Repeat { count: count + 1 };
            }
            Touch::ShiftDrag { .. } => {
                self.out.push(Command::Vibrate { kind: Haptic::LongPress });
                self.set_shift(Shift::Locked);
                self.last_shift_tap = None;
                self.touch = Touch::ShiftDrag { prior: Shift::Locked };
            }
            _ => {}
        }
    }

    fn open_popup(&mut self, key: usize, text: &str) {
        let upper = self.shift != Shift::Off;
        let hint = self.key(key).map(|k| k.hint.clone()).filter(|hint| !hint.is_empty());
        let items: Vec<String> = popup_of(text)
            .into_iter()
            .map(|item| if upper { item.to_uppercase() } else { item })
            .chain(hint)
            .collect();
        if items.is_empty() {
            return;
        }
        let Touch::Key { path, start, down, .. } = std::mem::replace(&mut self.touch, Touch::Idle) else {
            return;
        };
        let item = POPUP_ITEM_DP * self.density;
        let (center_x, top) = self.key(key).map_or((down.0, down.1), |k| (k.center().0, k.y));
        let span = item * items.len() as f32;
        let x = (center_x - item / 2.0).min(self.width - span).max(0.0);
        let y = (top - POPUP_LIFT_DP * self.density).max(0.0);
        self.out.push(Command::Vibrate { kind: Haptic::LongPress });
        self.out.push(Command::Popup { x, y, item_width: item, selected: 0, items: items.clone() });
        self.touch = Touch::Popup { items, selected: Some(0), x, y, path, start, down_y: down.1 };
    }

    fn tap(&mut self, key: usize, down: (f32, f32), t: u64) {
        self.tap_point = [down.0, down.1];
        let Some(action) = self.key(key).map(|k| k.action.clone()) else {
            return;
        };
        match action {
            Action::Text(text) => self.type_text(&text),
            Action::Space => self.tap_space(t),
            Action::Delete => self.delete_tap(),
            Action::Enter if self.search.is_some() => self.close_search(),
            Action::Enter => {
                self.end_word();
                self.clear_suggestions();
                self.out.push(Command::Enter);
            }
            Action::Layer(layer) => {
                self.layer = layer;
                self.rebuild();
            }
            Action::Emoji => self.open_panel(),
            Action::Shift => {}
        }
    }

    fn open_panel(&mut self) {
        self.close_search();
        self.out.push(Command::EmojiPanel { on: true, recents: self.recents.clone() });
    }

    fn open_search(&mut self) {
        self.search = Some(String::new());
        self.out.push(Command::EmojiPanel { on: false, recents: Vec::new() });
        self.layer = Layer::Alpha;
        self.rebuild();
        self.send_search();
    }

    /// The right end of the row clears the query, then leaves the search.
    fn tap_search_row(&mut self, x: f32) {
        if x < self.width - self.bar_height() {
            return;
        }
        match self.search.as_mut() {
            Some(query) if !query.is_empty() => {
                query.clear();
                self.send_search();
            }
            _ => self.open_panel(),
        }
    }

    fn close_search(&mut self) {
        if self.search.take().is_some() {
            self.out.push(Command::EmojiSearch { on: false, query: String::new(), results: Vec::new() });
        }
    }

    fn search_results(&self) -> Vec<String> {
        let query = self.search.as_deref().unwrap_or("");
        if query.trim().is_empty() {
            return self.recents.clone();
        }
        self.catalog.as_ref().map(|catalog| catalog.search(query)).unwrap_or_default()
    }

    fn send_search(&mut self) {
        let results = self.search_results();
        let query = self.search.clone().unwrap_or_default();
        self.out.push(Command::EmojiSearch { on: true, query, results });
    }

    /// Adds to the search query when searching, and returns whether it did.
    fn search_input(&mut self, text: &str) -> bool {
        let Some(query) = self.search.as_mut() else {
            return false;
        };
        query.push_str(&text.to_lowercase());
        self.send_search();
        true
    }

    /// Inserts a picked emoji and remembers it.
    fn pick(&mut self, emoji: &str) {
        self.insert_raw(emoji);
        if self.field.allows_learning() {
            self.recents.retain(|item| item != emoji);
            self.recents.insert(0, emoji.to_owned());
            self.recents.truncate(MAX_RECENTS);
            self.out.push(Command::SaveRecents { items: self.recents.clone() });
        }
    }

    fn type_text(&mut self, text: &str) {
        if self.search_input(text) {
            return;
        }
        let cased = match self.shift {
            Shift::Off => text.to_owned(),
            _ => text.to_uppercase(),
        };
        self.last_space_tap = None;
        if attaches_to_word(text) {
            self.finish_typing();
            match self.spacing {
                Spacing::Weak if self.field.allows_auto_spacing() && self.before.ends_with(' ') => {
                    // Punctuation after a typed space takes its place (P-5).
                    self.delete_units(1);
                    self.insert_text(&cased);
                    self.spacing = Spacing::Phantom;
                }
                Spacing::Phantom => self.insert_text(&cased),
                _ => {
                    self.insert_text(&cased);
                    self.spacing = Spacing::None;
                }
            }
            self.refresh_shift();
            return;
        }
        self.settle_phantom(text);
        self.insert_text(&cased);
        self.spacing = Spacing::None;
        self.last_word = None;
        self.corrected = None;
        if text.chars().all(char::is_alphabetic) {
            let tap = self.tap_point;
            let typing = self.typing.get_or_insert_with(Typing::default);
            typing.text.push_str(&cased);
            typing.taps.push(tap);
        } else {
            self.typing = None;
        }
        self.clear_suggestions();
        self.offer_corrections();
        self.consume_shift();
        self.refresh_shift();
    }

    fn is_valid_word(&self, word: &str) -> bool {
        let extra = self.memory.extra_words();
        self.lexicon.as_ref().is_some_and(|lexicon| lexicon.contains(word))
            || extra.is_some_and(|words| words.contains(word))
    }

    /// The token before the word that ends the mirror.
    fn token_before(&self, word: &str) -> String {
        let cut = self.before.len().saturating_sub(word.len());
        let earlier = self.before.get(..cut).unwrap_or("");
        if opens_sentence(earlier) {
            return SENTENCE_START.to_owned();
        }
        tokens(earlier).pop().unwrap_or_else(|| SENTENCE_START.to_owned())
    }

    /// The last tokens of the sentence, before `word` if it ends the mirror.
    fn history(&self, word: &str) -> Vec<String> {
        let cut = self.before.len().saturating_sub(word.len());
        let earlier = self.before.get(..cut).unwrap_or("");
        let sentence = earlier.rsplit(['.', '!', '?', '\n']).next().unwrap_or("");
        let mut words = tokens(sentence);
        let extra = words.len().saturating_sub(HISTORY_TOKENS);
        words.drain(..extra);
        words
    }

    /// Records a line, sealed before it leaves the engine, so that plain text
    /// never reaches the Android side. A field that forbids learning records nothing.
    fn trace(&mut self, mut line: String) {
        if self.field.allows_learning()
            && let Some(record) = self.sealed(&line)
        {
            self.out.push(Command::Trace { record });
        }
        line.zeroize();
    }

    /// Seals a line for the owner, if this build has an owner's key.
    #[must_use]
    pub fn sealed(&self, line: &str) -> Option<String> {
        let record = synthboard_seal::seal(&self.trace_key?, line.as_bytes()).ok()?;
        Some(synthboard_seal::to_hex(&record))
    }

    fn trace_layout(&mut self) {
        let keys: Vec<String> = self.board.iter().flat_map(|board| &board.keys).map(|key| {
            let (x, y) = key.center();
            format!("{}:{x:.0}:{y:.0}:{:.0}:{:.0}", key.label, key.width, key.height)
        }).collect();
        self.layout_traced = true;
        self.trace(format!("K\t{}", keys.join(" ")));
    }

    /// Ranks what the taps may mean. An ordered list of `(word, cost, complete)`.
    fn corrections(&self, typing: &Typing) -> Vec<(String, f32, bool)> {
        let (Some(lexicon), Some(layout)) = (
            self.lexicon.as_ref(),
            self.board.as_ref().and_then(|board| board.layout.as_ref()),
        ) else {
            return Vec::new();
        };
        let previous = self.token_before(&typing.text);
        let history = self.history(&typing.text);
        let context_weight = self.params.context_weight;
        let decoding = self.snapshot();
        let context = |word: &str| decoding.as_ref().map_or(0.0, |d| context_weight * d.score(&previous, &history, word));
        correct(layout, lexicon, &typing.taps, self.params.lambda, SUGGESTIONS, context)
            .into_iter()
            .filter(|found| !self.memory.is_blocked(found.surface))
            .map(|found| (found.surface.to_owned(), found.cost, found.complete))
            .collect()
    }

    /// Shows the likely words while tapping, with the typed text kept reachable.
    fn offer_corrections(&mut self) {
        if !self.field.allows_suggestions() {
            return;
        }
        let Some(typing) = self.typing.clone() else {
            return;
        };
        let mut offered: Vec<String> = self
            .corrections(&typing)
            .into_iter()
            .map(|(word, _, _)| case_like(&word, &typing.text))
            .collect();
        if !offered.contains(&typing.text) && !self.is_valid_word(&typing.text) {
            offered.insert(1.min(offered.len()), typing.text.clone());
        }
        offered.truncate(3);
        self.show_alternatives(&offered);
        if let Some(current) = self.typing.as_mut() {
            current.offered = offered;
        }
    }

    /// Learns a tapped word that stays as written.
    fn finish_typing_literal(&mut self) {
        if let Some(typing) = self.typing.take() {
            let previous = self.token_before(&typing.text);
            self.remember(previous, &typing.text);
        }
    }

    /// Replaces the tapped word when the word ends, under the no-fight rules:
    /// a valid word, a protected word or a word with inner capitals stays.
    fn finish_typing(&mut self) {
        let Some(typing) = self.typing.clone() else {
            return;
        };
        let inner_capital = typing.text.chars().skip(1).any(char::is_uppercase);
        if !self.field.allows_suggestions()
            || !self.field.allows_auto_spacing()
            || inner_capital
            || typing.taps.len() < 2
            || self.is_valid_word(&typing.text)
            || self.protected.iter().any(|word| word.to_lowercase() == typing.text.to_lowercase())
        {
            self.finish_typing_literal();
            return;
        }
        self.typing = None;
        let best = self
            .corrections(&typing)
            .into_iter()
            .find(|(_, cost, complete)| *complete && *cost <= AUTOCORRECT_COST);
        if let Some((word, _, _)) = best {
            let written = case_like(&word, &typing.text);
            self.delete_units(utf16_len(&typing.text));
            self.insert_text(&written);
            let previous = self.token_before(&written);
            self.remember(previous, &written);
            self.corrected = Some(Corrected { typed: typing.text, written });
        } else {
            let previous = self.token_before(&typing.text);
            self.remember(previous, &typing.text);
        }
    }

    /// Undoes the last correction, if the text still ends with it (B-9).
    fn revert_correction(&mut self) -> bool {
        let Some(fix) = self.corrected.take() else {
            return false;
        };
        let spaced = format!("{} ", fix.written);
        let trailing = if self.before.ends_with(&spaced) {
            " "
        } else if self.before.ends_with(&fix.written) {
            ""
        } else {
            return false;
        };
        self.delete_units(utf16_len(&fix.written) + utf16_len(trailing));
        self.insert_text(&fix.typed);
        self.insert_text(trailing);
        self.pending = None;
        self.protected.push(fix.typed);
        self.refresh_shift();
        true
    }

    /// Writes the owed space before a new word, unless a connector forbids it.
    fn settle_phantom(&mut self, next: &str) {
        let wants = self.spacing == Spacing::Phantom
            && next.chars().next().is_some_and(char::is_alphanumeric);
        if wants && self.field.allows_auto_spacing() {
            self.insert_text(" ");
        }
    }

    fn insert_raw(&mut self, text: &str) {
        self.settle_phantom("a");
        self.insert_text(text);
        self.spacing = Spacing::None;
        self.last_word = None;
        self.clear_suggestions();
    }

    fn insert_text(&mut self, text: &str) {
        self.out.push(Command::Commit { text: text.to_owned() });
        if self.field.allows_reading() {
            self.before.push_str(text);
        }
        self.cursor += utf16_len(text);
        self.expected_selection = Some(self.cursor);
    }

    fn delete_units(&mut self, units: u32) {
        self.out.push(Command::DeleteBefore { units });
        self.cursor = self.cursor.saturating_sub(units);
        self.expected_selection = Some(self.cursor);
        let mut remaining = units;
        while remaining > 0 {
            let Some(letter) = self.before.pop() else {
                break;
            };
            remaining = remaining.saturating_sub(letter.len_utf16() as u32);
        }
    }

    fn tap_space(&mut self, t: u64) {
        if self.search_input(" ") {
            return;
        }
        self.finish_typing();
        let double = self.last_space_tap.is_some_and(|last| t.saturating_sub(last) < DOUBLE_SPACE_MS);
        let eligible = self.before.ends_with(' ')
            && self.before.trim_end().chars().next_back().is_some_and(char::is_alphanumeric);
        if double && eligible && self.field.allows_auto_spacing() {
            self.delete_units(1);
            self.insert_text(". ");
            self.last_space_tap = None;
        } else {
            self.insert_text(" ");
            self.last_space_tap = Some(t);
        }
        self.spacing = Spacing::Weak;
        self.last_word = None;
        self.clear_suggestions();
        self.refresh_shift();
    }

    fn delete_tap(&mut self) {
        if let Some(query) = self.search.as_mut() {
            query.pop();
            self.send_search();
            return;
        }
        if self.selection.0 != self.selection.1 {
            self.out.push(Command::DeleteSelection);
            self.out.push(Command::RequestContext);
            return;
        }
        if self.revert_correction() {
            return;
        }
        if self.spacing == Spacing::Phantom && self.last_word.is_none() {
            self.spacing = Spacing::None;
            return;
        }
        self.pending = None;
        if let Some(word) = self.last_word.take()
            && self.before.ends_with(&word.text) {
                self.trace("D".to_owned());
                self.failed_gesture = Some(word.gesture.clone()).filter(|gesture| !gesture.is_empty());
                // Backspace after a swipe removes the whole word (B-7).
                self.delete_units(utf16_len(&word.text));
                self.spacing = Spacing::None;
                self.clear_suggestions();
                self.refresh_shift();
                return;
            }
        let typing = self.typing.take().map(|mut typing| {
            typing.text.pop();
            typing.taps.pop();
            typing
        });
        self.delete_one();
        self.typing = typing.filter(|typing| !typing.text.is_empty());
        self.offer_corrections();
    }

    /// Removes the word before the cursor, with the spaces after it.
    fn delete_word(&mut self) {
        self.end_word();
        let units = self.word_boundaries(1);
        if units > 0 {
            self.delete_units(units);
        }
        self.clear_suggestions();
        self.refresh_shift();
    }

    fn delete_one(&mut self) {
        self.spacing = Spacing::None;
        self.last_word = None;
        self.clear_suggestions();
        let units = self.before.graphemes(true).next_back().map_or(1, utf16_len);
        self.delete_units(units);
        self.refresh_shift();
    }

    fn word_boundaries(&self, words: u32) -> u32 {
        let mut remaining = words;
        let mut units = 0;
        let mut text = self.before.as_str();
        while remaining > 0 && !text.is_empty() {
            let trimmed = text.trim_end();
            let start = trimmed
                .rfind(|c: char| c.is_whitespace() || c == '\'' || c == '’')
                .map_or(0, |index| index + trimmed[index..].chars().next().map_or(1, char::len_utf8));
            let start = if start == trimmed.len() { trimmed.len().saturating_sub(1) } else { start };
            units += utf16_len(text.get(start..).unwrap_or(""));
            text = text.get(..start).unwrap_or("");
            remaining -= 1;
        }
        units
    }

    fn select_words(&mut self, words: u32, base: u32) {
        let units = self.word_boundaries(words);
        self.out.push(Command::Vibrate { kind: Haptic::Step });
        self.out.push(Command::Select { start: base.saturating_sub(units), end: base });
    }

    /// Moves the cursor by whole characters, so an emoji or a combined accent
    /// is never split. At either end of the text it does nothing.
    fn move_cursor(&mut self, steps: i32) {
        let count = steps.unsigned_abs() as usize;
        let units: u32 = if steps < 0 {
            self.before.graphemes(true).rev().take(count).map(utf16_len).sum()
        } else {
            self.after.graphemes(true).take(count).map(utf16_len).sum()
        };
        if units == 0 {
            return;
        }
        let target = if steps < 0 { self.cursor.saturating_sub(units) } else { self.cursor.saturating_add(units) };
        self.out.push(Command::Vibrate { kind: Haptic::Step });
        self.out.push(Command::Select { start: target, end: target });
    }

    /// Names the key under a screen reader's finger, once per key.
    fn hover(&mut self, x: f32, y: f32) {
        let key = self.board.as_ref().and_then(|board| board.key_at(x, y));
        if key != self.hovered {
            self.hovered = key;
            if let Some(found) = key.and_then(|index| self.key(index)) {
                let command = Command::Announce { text: found.label.clone(), icon: icon_of(&found.action) };
                self.out.push(command);
            }
        }
    }

    fn selection_changed(&mut self, start: u32, end: u32) {
        self.selection = (start, end);
        if end == 0 {
            // A chat app keeps the keyboard open after Send (M-3).
            self.leaning = LANGUAGE_START;
            self.confirm_pending();
            self.save_memory();
        }
        if self.expected_selection == Some(end) && start == end {
            return;
        }
        self.end_word();
        self.cursor = end;
        self.expected_selection = None;
        self.clear_suggestions();
        self.out.push(Command::RequestContext);
    }

    /// What ranking a gesture needs, cheap to copy and free of the engine, so
    /// that the live preview can run while the finger keeps moving.
    fn snapshot(&self) -> Option<Decoding> {
        Some(Decoding {
            lexicon: self.lexicon.clone()?,
            affinities: self.affinities.clone(),
            grammar: self.grammar.clone(),
            memory: Arc::clone(&self.memory),
            layout: self.board.as_ref()?.layout.clone()?,
            params: self.params,
            previous: self.previous_token(),
            history: self.history(""),
            leaning: self.leaning,
            shift: self.shift,
        })
    }

    /// The gesture so far and how to read it, if a gesture is under way.
    #[must_use]
    pub fn preview_job(&self) -> Option<(Decoding, Vec<Sample>)> {
        let Touch::Gesture { path, .. } = &self.touch else {
            return None;
        };
        Some((self.snapshot()?, path.clone()))
    }

    fn finish_gesture(&mut self, path: &[Sample]) {
        self.finish_typing();
        let Some(decoding) = self.snapshot() else {
            return;
        };
        let words = decoding.rank(path);
        let Some(first) = words.first().cloned() else {
            return;
        };
        if !self.layout_traced {
            self.trace_layout();
        }
        let points: Vec<String> = path.iter().map(|p| format!("{:.0},{:.0},{}", p.x, p.y, p.t_ms)).collect();
        self.trace(format!("G\t{first}\t{}\t{}\t{}", words.join(","), points.join(" "), self.history("").join(" ")));
        if self.search_input(&first) {
            return;
        }
        let cased = self.apply_case(&first);
        let alternatives: Vec<String> = words.iter().skip(1).map(|word| case_like(word, &cased)).collect();
        self.commit_word(cased, alternatives);
        if let Some(word) = self.last_word.as_mut() {
            word.gesture = path.to_vec();
        }
    }

    /// Writes a chosen word with the right spacing and keeps it replaceable.
    fn commit_word(&mut self, cased: String, alternatives: Vec<String>) {
        let previous = self.previous_token();
        self.remember(previous, &cased);
        let needs_space = self.field.allows_auto_spacing()
            && self.before.chars().next_back().is_some_and(|c| {
                !c.is_whitespace() && !matches!(c, '\'' | '’' | '-' | '(' | '[' | '{' | '«' | '"' | '/' | '@' | '#')
            });
        if needs_space {
            self.insert_text(" ");
        }
        self.insert_text(&cased);
        self.consume_shift();
        let elided = cased.ends_with(['\'', '’']);
        self.spacing = if elided { Spacing::None } else { Spacing::Phantom };
        self.show_alternatives(&alternatives);
        self.last_word = Some(LastWord {
            text: cased,
            alternatives,
            spacing_after: self.spacing,
            gesture: Vec::new(),
        });
    }

    fn apply_case(&self, word: &str) -> String {
        cased(self.shift, word)
    }

    fn show_alternatives(&mut self, alternatives: &[String]) {
        let slot = |index: usize| alternatives.get(index).cloned().unwrap_or_default();
        self.out.push(Command::Suggestions { center: slot(0), left: slot(1), right: slot(2) });
    }

    fn slot_at(&self, x: f32) -> usize {
        // Slots are left, centre at 36% of the width, right (B-1).
        if x < self.width * 0.32 {
            1
        } else if x < self.width * 0.68 {
            0
        } else {
            2
        }
    }

    /// The word shown at a position of the bar, if any.
    fn slot_word(&self, x: f32) -> Option<String> {
        if self.search.is_some() {
            return None;
        }
        let slot = self.slot_at(x);
        let list = match (&self.typing, &self.last_word) {
            (Some(typing), _) => &typing.offered,
            (None, Some(word)) if !word.alternatives.is_empty() => &word.alternatives,
            _ => &self.predictions,
        };
        list.get(slot).cloned()
    }

    fn forget(&mut self, word: &str) {
        let known = self.known();
        Arc::make_mut(&mut self.memory).forget(word, known);
        self.memory_changed = true;
        self.save_memory();
        if let Some(last) = self.last_word.as_mut() {
            last.alternatives.retain(|candidate| candidate != word);
            let remaining = last.alternatives.clone();
            self.show_alternatives(&remaining);
        } else if self.typing.is_some() {
            self.offer_corrections();
        } else {
            self.clear_suggestions();
        }
    }

    fn tap_suggestion(&mut self, x: f32) {
        if self.search.is_some() {
            let cell = self.width / SEARCH_CELLS as f32;
            let index = (x / cell) as usize;
            if let Some(emoji) = self.search_results().into_iter().nth(index) {
                self.pick(&emoji);
                self.close_search();
            }
            return;
        }
        let slot = self.slot_at(x);
        if let Some(typing) = self.typing.take() {
            if let Some(word) = typing.offered.get(slot).cloned() {
                self.delete_units(utf16_len(&typing.text));
                self.commit_word(word, Vec::new());
                self.refresh_shift();
            } else {
                self.typing = Some(typing);
            }
            return;
        }
        let has_alternatives = self.last_word.as_ref().is_some_and(|word| !word.alternatives.is_empty());
        if !has_alternatives {
            if let Some(next) = self.predictions.get(slot).cloned() {
                self.commit_word(next, Vec::new());
                self.refresh_shift();
                self.clear_suggestions();
            }
            return;
        }
        let Some(mut word) = self.last_word.take() else {
            return;
        };
        if !self.before.ends_with(&word.text) || slot >= word.alternatives.len() {
            self.last_word = Some(word);
            return;
        }
        let chosen = word.alternatives.remove(slot);
        self.trace(format!("C\t{chosen}"));
        let gesture = std::mem::take(&mut word.gesture);
        self.teach(&chosen, &gesture);
        word.gesture = gesture;
        if let Some(pending) = self.pending.as_mut() {
            pending.1 = chosen.to_lowercase();
        }
        self.delete_units(utf16_len(&word.text));
        self.insert_text(&chosen);
        word.alternatives.insert(slot, std::mem::replace(&mut word.text, chosen));
        self.spacing = word.spacing_after;
        self.show_alternatives(&word.alternatives);
        self.last_word = Some(word);
    }
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::panic)]
mod tests {
    use super::*;

    const TEXT_FIELD: Field = Field {
        input_type: android::CLASS_TEXT | android::CAP_SENTENCES,
        ime_options: 0,
    };
    const DENSITY: f32 = 2.625;

    struct Rig {
        engine: Engine,
        text: String,
        t: u64,
    }

    impl Rig {
        fn new(field: Field) -> Self {
            let mut engine = Engine::new();
            engine.step(Event::Resize { width: 1080.0, screen_height: 2424.0, density: DENSITY });
            engine.set_lexicon(Arc::new(Lexicon::new([
                ("bonjour", 150.0),
                ("bonsoir", 100.0),
                ("ou", 180.0),
                ("où", 170.0),
                ("l'", 200.0),
                ("été", 150.0),
                ("than", 150.0),
                ("merge", 120.0),
                ("merde", 160.0),
            ])));
            let mut rig = Self { engine, text: String::new(), t: 10_000 };
            let step = rig.engine.step(Event::Shown { field, before: String::new(), after: String::new(), cursor: 0 });
            rig.apply(step);
            rig
        }

        fn apply(&mut self, step: Step) -> Step {
            for command in &step.commands {
                match command {
                    Command::Commit { text } => self.text.push_str(text),
                    Command::DeleteBefore { units } => {
                        for _ in 0..*units {
                            self.text.pop();
                        }
                    }
                    _ => {}
                }
            }
            step
        }

        fn center(&self, label: &str) -> (f32, f32) {
            let board = self.engine.board.as_ref().unwrap();
            board
                .keys
                .iter()
                .find(|key| key.action == Action::Text(label.to_owned()) || key.label == label)
                .unwrap()
                .center()
        }

        fn space(&self) -> (f32, f32) {
            let board = self.engine.board.as_ref().unwrap();
            board.keys.iter().find(|key| key.action == Action::Space).unwrap().center()
        }

        fn send(&mut self, event: Event) -> Step {
            let step = self.engine.step(event);
            self.apply(step)
        }

        fn tap_at(&mut self, (x, y): (f32, f32)) {
            self.t += 700;
            let t = self.t;
            self.send(Event::Down { pointer: 0, x, y, t });
            self.send(Event::Up { pointer: 0, x, y, t: t + 40 });
        }

        fn tap(&mut self, label: &str) {
            let point = self.center(label);
            self.tap_at(point);
        }

        fn swipe(&mut self, word: &str) {
            self.t += 700;
            let mut t = self.t;
            let points: Vec<(f32, f32)> = synthswipe::fold(word)
                .iter()
                .map(|letter| self.center(&letter.to_string()))
                .collect();
            let (x, y) = points[0];
            self.send(Event::Down { pointer: 0, x, y, t });
            for pair in points.windows(2) {
                for step in 1..=8 {
                    let ratio = step as f32 / 8.0;
                    t += 12;
                    self.send(Event::Move {
                        pointer: 0,
                        x: pair[0].0 + (pair[1].0 - pair[0].0) * ratio,
                        y: pair[0].1 + (pair[1].1 - pair[0].1) * ratio,
                        t,
                    });
                }
            }
            let (x, y) = *points.last().unwrap();
            self.send(Event::Up { pointer: 0, x, y, t: t + 10 });
        }
    }

    #[test]
    fn taps_type_letters_with_automatic_capitals() {
        let mut rig = Rig::new(TEXT_FIELD);
        rig.tap("a");
        rig.tap("z");
        assert_eq!(rig.text, "Az");
    }

    #[test]
    fn a_swipe_writes_a_title_cased_word_and_owes_a_space() {
        let mut rig = Rig::new(TEXT_FIELD);
        rig.swipe("bonjour");
        assert_eq!(rig.text, "Bonjour");
        rig.swipe("bonsoir");
        assert_eq!(rig.text, "Bonjour bonsoir");
    }

    #[test]
    fn punctuation_attaches_and_the_space_stays_owed() {
        let mut rig = Rig::new(TEXT_FIELD);
        rig.swipe("bonjour");
        rig.tap(",");
        rig.swipe("bonsoir");
        assert_eq!(rig.text, "Bonjour, bonsoir");
    }

    #[test]
    fn elision_takes_no_space() {
        let mut rig = Rig::new(TEXT_FIELD);
        rig.swipe("été");
        rig.tap("'");
        rig.swipe("ou");
        assert_eq!(rig.text, "Été'ou");
    }

    #[test]
    fn backspace_after_a_swipe_removes_the_whole_word() {
        let mut rig = Rig::new(TEXT_FIELD);
        rig.swipe("bonjour");
        rig.swipe("bonsoir");
        rig.tap("⌫");
        assert_eq!(rig.text, "Bonjour ");
        rig.tap("⌫");
        assert_eq!(rig.text, "Bonjour");
    }

    #[test]
    fn a_suggestion_replaces_the_word_and_can_be_tapped_back() {
        let mut rig = Rig::new(TEXT_FIELD);
        rig.tap("x");
        rig.tap_at(rig.space());
        rig.swipe("ou");
        assert_eq!(rig.text, "X ou");
        let bar_y = 10.0;
        rig.tap_at((540.0, bar_y));
        assert_eq!(rig.text, "X où");
        rig.tap_at((540.0, bar_y));
        assert_eq!(rig.text, "X ou");
    }

    #[test]
    fn double_space_makes_a_full_stop() {
        let mut rig = Rig::new(TEXT_FIELD);
        rig.swipe("bonjour");
        let space = rig.space();
        rig.t += 100;
        let t = rig.t;
        rig.send(Event::Down { pointer: 0, x: space.0, y: space.1, t });
        rig.send(Event::Up { pointer: 0, x: space.0, y: space.1, t: t + 30 });
        rig.send(Event::Down { pointer: 0, x: space.0, y: space.1, t: t + 200 });
        rig.send(Event::Up { pointer: 0, x: space.0, y: space.1, t: t + 230 });
        assert_eq!(rig.text, "Bonjour. ");
    }

    #[test]
    fn a_held_key_opens_its_accents_and_release_types_the_first() {
        let mut rig = Rig::new(Field { input_type: android::CLASS_TEXT, ime_options: 0 });
        let (x, y) = rig.center("e");
        let t = rig.t + 700;
        let step = rig.send(Event::Down { pointer: 0, x, y, t });
        assert_eq!(step.deadline, Some(t + LONG_PRESS_MS));
        let step = rig.send(Event::Deadline { t: t + LONG_PRESS_MS });
        assert!(step.commands.iter().any(|c| matches!(c, Command::Popup { items, .. } if items[0] == "é")));
        rig.send(Event::Up { pointer: 0, x, y, t: t + LONG_PRESS_MS + 50 });
        assert_eq!(rig.text, "é");
    }

    #[test]
    fn a_top_row_letter_without_accents_offers_its_digit() {
        let mut rig = Rig::new(Field { input_type: android::CLASS_TEXT, ime_options: 0 });
        let (x, y) = rig.center("z");
        let t = rig.t + 700;
        rig.send(Event::Down { pointer: 0, x, y, t });
        rig.send(Event::Deadline { t: t + LONG_PRESS_MS });
        rig.send(Event::Up { pointer: 0, x, y, t: t + LONG_PRESS_MS + 50 });
        assert_eq!(rig.text, "2");
    }

    #[test]
    fn sliding_along_the_popup_picks_another_accent() {
        let mut rig = Rig::new(Field { input_type: android::CLASS_TEXT, ime_options: 0 });
        let (x, y) = rig.center("e");
        let t = rig.t + 700;
        rig.send(Event::Down { pointer: 0, x, y, t });
        rig.send(Event::Deadline { t: t + LONG_PRESS_MS });
        let item = POPUP_ITEM_DP * DENSITY;
        rig.send(Event::Move { pointer: 0, x: x + item * 1.1, y, t: t + 400 });
        rig.send(Event::Up { pointer: 0, x: x + item * 1.1, y, t: t + 450 });
        assert_eq!(rig.text, "è");
    }

    #[test]
    fn a_slow_start_turns_the_popup_back_into_a_gesture() {
        let mut rig = Rig::new(Field { input_type: android::CLASS_TEXT, ime_options: 0 });
        let (x, y) = rig.center("b");
        let (ox, oy) = rig.center("o");
        let t = rig.t + 700;
        rig.send(Event::Down { pointer: 0, x, y, t });
        assert_eq!(popup_of("b"), [] as [std::string::String; 0]);
        let (ax, ay) = rig.center("e");
        rig.send(Event::Up { pointer: 0, x, y, t: t + 10 });
        rig.text.clear();
        let t = t + 2000;
        rig.send(Event::Down { pointer: 0, x: ax, y: ay, t });
        rig.send(Event::Deadline { t: t + LONG_PRESS_MS });
        let far = POPUP_BAND_DP * DENSITY + 1.0;
        let step = rig.send(Event::Move { pointer: 0, x: ox, y: oy + far * 2.0, t: t + 500 });
        assert!(step.commands.contains(&Command::Trail { on: true }));
    }

    #[test]
    fn a_slow_start_on_a_digit_key_still_swipes() {
        let mut rig = Rig::new(Field { input_type: android::CLASS_TEXT, ime_options: 0 });
        let (x, y) = rig.center("z");
        let t = rig.t + 700;
        rig.send(Event::Down { pointer: 0, x, y, t });
        rig.send(Event::Deadline { t: t + LONG_PRESS_MS });
        let travel = rig.engine.board.as_ref().unwrap().key_width;
        let step = rig.send(Event::Move { pointer: 0, x: x + travel, y, t: t + 400 });
        assert!(step.commands.contains(&Command::Trail { on: true }));
    }

    #[test]
    fn a_swipe_starts_soon_after_a_tap() {
        let mut rig = Rig::new(Field { input_type: android::CLASS_TEXT, ime_options: 0 });
        rig.tap("a");
        let (x, y) = rig.center("s");
        let t = rig.t + 140;
        rig.send(Event::Down { pointer: 0, x, y, t });
        let travel = rig.engine.board.as_ref().unwrap().key_width * 2.0;
        let step = rig.send(Event::Move { pointer: 0, x: x + travel, y, t: t + 60 });
        assert!(step.commands.contains(&Command::Trail { on: true }));
    }

    #[test]
    fn holding_delete_repeats_and_lifting_stops_it() {
        let mut rig = Rig::new(Field { input_type: android::CLASS_TEXT, ime_options: 0 });
        for letter in ["a", "z", "e"] {
            rig.tap(letter);
        }
        let (x, y) = rig.center("⌫");
        let t = rig.t + 700;
        rig.send(Event::Down { pointer: 0, x, y, t });
        let step = rig.send(Event::Deadline { t: t + DELETE_DELAY_MS });
        assert_eq!(rig.text, "az");
        assert_eq!(step.deadline, Some(t + DELETE_DELAY_MS + DELETE_REPEAT_MS));
        rig.send(Event::Deadline { t: t + DELETE_DELAY_MS + DELETE_REPEAT_MS });
        assert_eq!(rig.text, "a");
        let step = rig.send(Event::Up { pointer: 0, x, y, t: t + 500 });
        assert_eq!(step.deadline, None);
    }

    #[test]
    fn password_fields_never_gesture() {
        let mut rig = Rig::new(Field { input_type: android::CLASS_TEXT | android::PASSWORD, ime_options: 0 });
        let (ax, ay) = rig.center("a");
        let (px, py) = rig.center("p");
        let t = rig.t + 700;
        rig.send(Event::Down { pointer: 0, x: ax, y: ay, t });
        let step = rig.send(Event::Move { pointer: 0, x: px, y: py, t: t + 50 });
        assert!(!step.commands.contains(&Command::Trail { on: true }));
    }

    #[test]
    fn two_quick_shift_taps_lock_capitals() {
        let mut rig = Rig::new(Field { input_type: android::CLASS_TEXT, ime_options: 0 });
        let (x, y) = rig.center("⇧");
        let t = rig.t + 700;
        rig.send(Event::Down { pointer: 0, x, y, t });
        rig.send(Event::Up { pointer: 0, x, y, t: t + 30 });
        rig.send(Event::Down { pointer: 0, x, y, t: t + 120 });
        rig.send(Event::Up { pointer: 0, x, y, t: t + 150 });
        rig.tap("a");
        rig.tap("b");
        assert_eq!(rig.text, "AB");
    }

    #[test]
    fn a_sliding_shift_types_one_capital() {
        let mut rig = Rig::new(Field { input_type: android::CLASS_TEXT, ime_options: 0 });
        let (x, y) = rig.center("⇧");
        let (ax, ay) = rig.center("a");
        let t = rig.t + 700;
        rig.send(Event::Down { pointer: 0, x, y, t });
        rig.send(Event::Move { pointer: 0, x: ax, y: ay, t: t + 50 });
        rig.send(Event::Up { pointer: 0, x: ax, y: ay, t: t + 80 });
        rig.tap("b");
        assert_eq!(rig.text, "Ab");
    }

    #[test]
    fn no_finger_means_no_deadline() {
        let mut rig = Rig::new(TEXT_FIELD);
        let (x, y) = rig.center("a");
        let t = rig.t + 700;
        rig.send(Event::Down { pointer: 0, x, y, t });
        let step = rig.send(Event::Cancel);
        assert_eq!(step.deadline, None);
    }

    #[test]
    fn a_second_finger_completes_the_first_tap_in_order() {
        let mut rig = Rig::new(Field { input_type: android::CLASS_TEXT, ime_options: 0 });
        let (ax, ay) = rig.center("a");
        let (zx, zy) = rig.center("z");
        let t = rig.t + 700;
        rig.send(Event::Down { pointer: 0, x: ax, y: ay, t });
        rig.send(Event::Down { pointer: 1, x: zx, y: zy, t: t + 30 });
        rig.send(Event::Up { pointer: 0, x: ax, y: ay, t: t + 40 });
        rig.send(Event::Up { pointer: 1, x: zx, y: zy, t: t + 60 });
        assert_eq!(rig.text, "az");
    }

    #[test]
    fn emoji_search_takes_the_keys_and_leaves_the_field_alone() {
        let mut rig = Rig::new(TEXT_FIELD);
        rig.engine.set_catalog(Arc::new(Catalog::parse("Food\t🍕\t\tpizza|pizza|fromage|cheese\n")));
        rig.send(Event::OpenEmojiSearch);
        rig.tap("f");
        rig.tap("r");
        assert_eq!(rig.text, "");
        assert_eq!(rig.engine.search_results(), ["🍕"]);
        rig.tap_at((10.0, 10.0));
        assert_eq!(rig.text, "🍕");
        assert!(rig.engine.search.is_none());
        assert_eq!(rig.engine.recents, ["🍕"]);
    }

    #[test]
    fn the_search_row_clears_then_leaves() {
        let mut rig = Rig::new(TEXT_FIELD);
        rig.send(Event::OpenEmojiSearch);
        rig.tap("f");
        assert_eq!(rig.engine.search.as_deref(), Some("f"));
        rig.tap_at((1070.0, -10.0));
        assert_eq!(rig.engine.search.as_deref(), Some(""));
        rig.tap_at((1070.0, -10.0));
        assert!(rig.engine.search.is_none());
    }

    #[test]
    fn the_bar_offers_sentence_starts_and_next_words() {
        let mut rig = Rig::new(TEXT_FIELD);
        rig.engine.set_language(
            Arc::new(Affinities::default()),
            Arc::new(Grammar::default()),
            Arc::new(NextWords::parse("<s>\tje the\nje\tsuis\n")),
        );
        rig.engine.clear_suggestions();
        assert_eq!(rig.engine.predictions, ["Je", "The"]);
        rig.tap_at((540.0, 10.0));
        assert_eq!(rig.text, "Je");
        rig.engine.clear_suggestions();
        assert_eq!(rig.engine.predictions, ["suis"]);
        rig.tap_at((540.0, 10.0));
        assert_eq!(rig.text, "Je suis");
    }

    #[test]
    fn predictions_after_a_sentence_are_capitalised() {
        let mut rig = Rig::new(TEXT_FIELD);
        rig.engine.set_language(
            Arc::new(Affinities::default()),
            Arc::new(Grammar::default()),
            Arc::new(NextWords::parse("<s>\tje the\n")),
        );
        rig.send(Event::LanguageReady);
        assert_eq!(rig.engine.predictions, ["Je", "The"]);
        rig.tap("a");
        rig.tap(".");
        rig.tap_at(rig.space());
        assert_eq!(rig.engine.predictions, ["Je", "The"]);
    }

    #[test]
    fn the_preview_follows_the_finger_and_matches_the_final_word() {
        let mut rig = Rig::new(Field { input_type: android::CLASS_TEXT, ime_options: 0 });
        assert!(rig.engine.preview_job().is_none());
        let word = "bonjour";
        let points: Vec<(f32, f32)> = synthswipe::fold(word).iter().map(|l| rig.center(&l.to_string())).collect();
        let t = rig.t + 700;
        rig.send(Event::Down { pointer: 0, x: points[0].0, y: points[0].1, t });
        for (index, point) in points.iter().enumerate().skip(1) {
            rig.send(Event::Move { pointer: 0, x: point.0, y: point.1, t: t + 40 * index as u64 });
        }
        let (decoding, path) = rig.engine.preview_job().unwrap();
        assert_eq!(decoding.preview(&path), "bonjour");
        let (x, y) = *points.last().unwrap();
        rig.send(Event::Up { pointer: 0, x, y, t: t + 400 });
        assert_eq!(rig.text, "bonjour");
        assert!(rig.engine.preview_job().is_none());
    }

    fn traces(step: &Step) -> Vec<&str> {
        step.commands
            .iter()
            .filter_map(|c| match c {
                Command::Trace { record } => Some(record.as_str()),
                _ => None,
            })
            .collect()
    }

    fn opened(secret: &[u8; 32], record: &str) -> String {
        let bytes = synthboard_seal::from_hex(record).unwrap();
        String::from_utf8(synthboard_seal::open(secret, &bytes).unwrap()).unwrap()
    }

    fn traced_rig(field: Field) -> (Rig, [u8; 32]) {
        let (secret, public) = synthboard_seal::generate_keys().unwrap();
        let mut rig = Rig::new(field);
        rig.engine.set_trace_key(public);
        (rig, secret)
    }

    #[test]
    fn without_a_key_no_trace_can_exist() {
        let mut rig = Rig::new(TEXT_FIELD);
        let step = rig.send(Event::Session { unix: 1 });
        assert_eq!(traces(&step), Vec::<&str>::new());
    }

    #[test]
    fn traces_are_sealed_before_they_leave_the_engine() {
        let (mut rig, secret) = traced_rig(TEXT_FIELD);
        let step = rig.send(Event::Session { unix: 42 });
        let record = traces(&step)[0];
        assert!(!record.contains("S\t42"));
        assert_eq!(opened(&secret, record), "S\t42");
    }

    #[test]
    fn a_field_that_forbids_learning_records_nothing() {
        let private = Field { input_type: android::CLASS_TEXT, ime_options: android::NO_PERSONALIZED_LEARNING };
        let (mut rig, _) = traced_rig(private);
        let step = rig.send(Event::Session { unix: 1 });
        assert_eq!(traces(&step), Vec::<&str>::new());
    }

    #[test]
    fn addresses_are_neither_learned_nor_traced() {
        let field = Field { input_type: android::CLASS_TEXT | android::URI, ime_options: 0 };
        assert!(!field.allows_learning());
    }

    #[test]
    fn a_password_never_enters_the_mirror() {
        let password = Field { input_type: android::CLASS_TEXT | android::PASSWORD, ime_options: 0 };
        let mut rig = Rig::new(password);
        rig.send(Event::Shown { field: password, before: "secret".to_owned(), after: "deux".to_owned(), cursor: 6 });
        rig.tap("a");
        rig.send(Event::Context { before: "secret".to_owned(), after: "deux".to_owned() });
        assert_eq!(rig.engine.before, "");
        assert_eq!(rig.engine.after, "");
        assert!(!password.allows_reading());
    }

    #[test]
    fn closing_the_keyboard_wipes_what_it_read() {
        let mut rig = rig_with_text("mon message prive");
        rig.send(Event::OpenEmojiSearch);
        rig.tap("a");
        rig.send(Event::Hidden);
        assert_eq!(rig.engine.before, "");
        assert_eq!(rig.engine.after, "");
        assert!(rig.engine.search.is_none());
        assert_eq!(rig.engine.protected, Vec::<String>::new());
    }

    #[test]
    fn grammar_turns_sous_into_suis_after_je() {
        let mut rig = Rig::new(Field { input_type: android::CLASS_TEXT, ime_options: 0 });
        let lexicon = Lexicon::new([("sous", 172.0), ("suis", 124.0), ("je", 200.0)]);
        rig.engine.set_lexicon(Arc::new(lexicon));
        rig.engine.set_language(
            Arc::new(Affinities::default()),
            Arc::new(Grammar::parse("suis\t1\nsous\t0\n")),
            Arc::new(NextWords::default()),
        );
        rig.send(Event::Text { text: "je ".to_owned() });
        rig.swipe("sous");
        assert!(rig.text.ends_with("suis"), "{}", rig.text);
    }

    #[test]
    fn each_field_type_opens_its_own_keys() {
        let keys = |input_type| {
            let rig = Rig::new(Field { input_type, ime_options: 0 });
            rig.engine.board.as_ref().unwrap().keys.iter().map(|key| key.label.clone()).collect::<Vec<_>>()
        };
        assert!(keys(android::CLASS_NUMBER).len() <= 16);
        assert!(keys(android::CLASS_PHONE).contains(&"*".to_owned()));
        assert!(keys(android::CLASS_TEXT | android::URI).contains(&"/".to_owned()));
        assert!(keys(android::CLASS_TEXT | android::EMAIL).contains(&"@".to_owned()));
    }

    #[test]
    fn forgetting_everything_empties_the_memory_and_recents() {
        let mut rig = Rig::new(Field { input_type: android::CLASS_TEXT, ime_options: 0 });
        rig.swipe("bonjour");
        rig.swipe("bonsoir");
        rig.send(Event::Text { text: "🍕".to_owned() });
        rig.send(Event::ForgetAll);
        assert_eq!(rig.engine.memory.to_text(), "");
        assert_eq!(rig.engine.recents, [] as [std::string::String; 0]);
    }

    #[test]
    fn the_same_gesture_follows_the_sessions_language() {
        let mut rig = Rig::new(Field { input_type: android::CLASS_TEXT, ime_options: 0 });
        rig.engine.set_lexicon(Arc::new(Lexicon::with_languages([
            ("il", 120.0, FRENCH),
            ("i'll", 125.0, ENGLISH),
            ("the", 180.0, ENGLISH),
            ("what", 170.0, ENGLISH),
        ])));
        rig.swipe("il");
        assert_eq!(rig.text, "il");
        rig.send(Event::Hidden);
        rig.text.clear();
        rig.send(Event::Shown { field: Field { input_type: android::CLASS_TEXT, ime_options: 0 }, before: String::new(), after: String::new(), cursor: 0 });
        rig.swipe("the");
        rig.swipe("what");
        rig.swipe("il");
        assert!(rig.text.ends_with("I'll"), "{}", rig.text);
    }

    #[test]
    fn english_i_is_always_capital() {
        assert_eq!(cased(Shift::Off, "i'll"), "I'll");
        assert_eq!(cased(Shift::Off, "il"), "il");
    }

    #[test]
    fn the_word_typed_after_deleting_a_swipe_owns_that_gesture() {
        let mut rig = Rig::new(Field { input_type: android::CLASS_TEXT, ime_options: 0 });
        rig.swipe("bonjour");
        rig.tap("⌫");
        assert_eq!(rig.text, "");
        type_word(&mut rig, "zxcv");
        rig.tap_at(rig.space());
        rig.text.clear();
        rig.engine.before.clear();
        rig.swipe("bonjour");
        assert!(rig.text.ends_with("zxcv"), "{}", rig.text);
    }

    #[test]
    fn the_trace_names_the_word_a_deleted_gesture_meant() {
        let mut rig = Rig::new(Field { input_type: android::CLASS_TEXT, ime_options: 0 });
        let (secret, public) = synthboard_seal::generate_keys().unwrap();
        rig.engine.set_trace_key(public);
        rig.swipe("bonjour");
        rig.tap("⌫");
        type_word(&mut rig, "zxcv");
        let (x, y) = rig.space();
        let t = rig.t + 700;
        rig.send(Event::Down { pointer: 0, x, y, t });
        let step = rig.send(Event::Up { pointer: 0, x, y, t: t + 30 });
        assert!(traces(&step).iter().any(|record| opened(&secret, record) == "F\tzxcv"));
    }

    #[test]
    fn a_word_picked_from_the_bar_owns_that_gesture() {
        let mut rig = Rig::new(Field { input_type: android::CLASS_TEXT, ime_options: 0 });
        rig.swipe("ou");
        rig.tap_at((540.0, 10.0));
        assert_eq!(rig.text, "où");
        rig.swipe("ou");
        assert!(rig.text.ends_with("où"), "{}", rig.text);
    }

    #[test]
    fn choosing_a_word_twice_for_a_loose_gesture_makes_it_lead() {
        let mut rig = Rig::new(Field { input_type: android::CLASS_TEXT, ime_options: 0 });
        let pitch = rig.engine.board.as_ref().and_then(|b| b.layout.as_ref()).unwrap().pitch();
        let close = |shift: f32| vec![[1.0 + shift, 1.0], [2.0 + shift, 1.0]];
        Arc::make_mut(&mut rig.engine.memory).add_trace("pue", close(0.3));
        assert_eq!(rig.engine.memory.trace_matches(&close(0.0)).first().map(|m| m.2), Some(1));
        Arc::make_mut(&mut rig.engine.memory).add_trace("pue", close(-0.3));
        assert_eq!(rig.engine.memory.trace_matches(&close(0.0)).first().map(|m| m.2), Some(2));
        assert!(pitch > 0.0);
    }

    #[test]
    fn teaching_saves_at_once_while_the_keyboard_stays_open() {
        let mut rig = Rig::new(Field { input_type: android::CLASS_TEXT, ime_options: 0 });
        rig.swipe("ou");
        let (x, y) = (540.0, 10.0);
        let t = rig.t + 700;
        rig.send(Event::Down { pointer: 0, x, y, t });
        let step = rig.send(Event::Up { pointer: 0, x, y, t: t + 40 });
        assert!(step.commands.iter().any(|c| matches!(c, Command::SaveMemory { text } if text.contains('~'))));
    }

    fn plain_rig() -> Rig {
        Rig::new(Field { input_type: android::CLASS_TEXT, ime_options: 0 })
    }

    fn rig_with_text(before: &str) -> Rig {
        let mut rig = plain_rig();
        let cursor = utf16_len(before);
        rig.send(Event::Shown {
            field: Field { input_type: android::CLASS_TEXT, ime_options: 0 },
            before: before.to_owned(),
            after: String::new(),
            cursor,
        });
        rig.text = before.to_owned();
        rig
    }

    fn selects(step: &Step, start: u32, end: u32) -> bool {
        step.commands.contains(&Command::Select { start, end })
    }

    #[test]
    fn dragging_space_left_moves_by_grapheme_and_ignores_small_moves() {
        let mut rig = rig_with_text("abc");
        let (x, y) = rig.space();
        let t = rig.t + 700;
        rig.send(Event::Down { pointer: 0, x, y, t });
        let step = rig.send(Event::Move { pointer: 0, x: x - 10.0, y, t: t + 10 });
        assert!(!step.commands.iter().any(|c| matches!(c, Command::Select { .. })));
        rig.send(Event::Move { pointer: 0, x: x - 30.0, y, t: t + 20 });
        let step = rig.send(Event::Move { pointer: 0, x: x - 60.0, y, t: t + 30 });
        assert!(selects(&step, 1, 1), "{:?}", step.commands);
        rig.send(Event::Up { pointer: 0, x: x - 60.0, y, t: t + 40 });
        assert_eq!(rig.text, "abc");
    }

    #[test]
    fn a_cursor_drag_steps_over_a_whole_emoji() {
        let mut rig = rig_with_text("a👍🏽");
        let (x, y) = rig.space();
        let t = rig.t + 700;
        rig.send(Event::Down { pointer: 0, x, y, t });
        rig.send(Event::Move { pointer: 0, x: x - 30.0, y, t: t + 20 });
        let step = rig.send(Event::Move { pointer: 0, x: x - 40.0, y, t: t + 30 });
        assert!(selects(&step, 1, 1), "{:?}", step.commands);
    }

    #[test]
    fn the_gesture_threshold_relaxes_after_a_tap() {
        let close = |a: f32, b: f32| (a - b).abs() < 0.001;
        assert!(close(gesture_slop(None), SLOP_KEYS));
        assert!(close(gesture_slop(Some(0)), FAST_TAP_SLOP_KEYS));
        assert!(close(gesture_slop(Some(225)), FAST_TAP_SLOP_KEYS.midpoint(SLOP_KEYS)));
        assert!(close(gesture_slop(Some(450)), SLOP_KEYS));
        assert!(close(gesture_slop(Some(FAST_TAP_MS - 1)), SLOP_KEYS));
        assert!(close(gesture_slop(Some(FAST_TAP_MS)), SLOP_KEYS));
        assert!(close(gesture_slop(Some(10_000)), SLOP_KEYS));
    }

    #[test]
    fn a_long_cursor_drag_keeps_counting_from_where_it_stood() {
        let mut rig = rig_with_text("abcdef");
        let (x, y) = rig.space();
        let step_px = CURSOR_STEP_DP * DENSITY;
        let t = rig.t + 700;
        rig.send(Event::Down { pointer: 0, x, y, t });
        rig.send(Event::Move { pointer: 0, x: x - step_px - 1.0, y, t: t + 10 });
        let first = rig.send(Event::Move { pointer: 0, x: x - step_px * 2.0 - 1.0, y, t: t + 20 });
        assert!(selects(&first, 4, 4), "{:?}", first.commands);
        rig.send(Event::Selection { start: 4, end: 4 });
        let none = rig.send(Event::Move { pointer: 0, x: x - step_px * 2.0 - 2.0, y, t: t + 30 });
        assert!(!none.commands.iter().any(|c| matches!(c, Command::Select { .. })));
        let next = rig.send(Event::Move { pointer: 0, x: x - step_px * 3.0 - 1.0, y, t: t + 40 });
        assert!(selects(&next, 3, 3), "{:?}", next.commands);
    }

    #[test]
    fn delete_drag_counts_whole_word_steps_and_shrinks_back() {
        let mut rig = rig_with_text("une deux trois");
        let (x, y) = rig.center("⌫");
        let (start, word) = (DELETE_SWIPE_DP * DENSITY, DELETE_WORD_DP * DENSITY);
        let t = rig.t + 700;
        rig.send(Event::Down { pointer: 0, x, y, t });
        rig.send(Event::Move { pointer: 0, x: x - start - 1.0, y, t: t + 10 });
        let one = rig.send(Event::Move { pointer: 0, x: x - start - 2.0, y, t: t + 20 });
        assert!(selects(&one, 9, 14), "{:?}", one.commands);
        let same = rig.send(Event::Move { pointer: 0, x: x - start - word + 4.0, y, t: t + 30 });
        assert!(!same.commands.iter().any(|c| matches!(c, Command::Select { .. })));
        let two = rig.send(Event::Move { pointer: 0, x: x - start - word - 1.0, y, t: t + 40 });
        assert!(selects(&two, 4, 14), "{:?}", two.commands);
        let back = rig.send(Event::Move { pointer: 0, x: x - start - 2.0, y, t: t + 50 });
        assert!(selects(&back, 9, 14), "{:?}", back.commands);
    }

    #[test]
    fn a_gesture_needs_the_full_slop_and_not_a_pixel_less() {
        let mut rig = plain_rig();
        let (x, y) = rig.center("s");
        let width = rig.engine.board.as_ref().unwrap().key_width;
        let t = rig.t + 700;
        rig.send(Event::Down { pointer: 0, x, y, t });
        let short = rig.send(Event::Move { pointer: 0, x: x + width * SLOP_KEYS - 1.0, y, t: t + 20 });
        assert!(!short.commands.contains(&Command::Trail { on: true }));
        let far = rig.send(Event::Move { pointer: 0, x: x + width * SLOP_KEYS + 1.0, y, t: t + 40 });
        assert!(far.commands.contains(&Command::Trail { on: true }));
    }

    fn drag_right(before: &str, after: &str, cursor: u32, distance: f32) -> Step {
        let mut rig = plain_rig();
        rig.send(Event::Shown {
            field: Field { input_type: android::CLASS_TEXT, ime_options: 0 },
            before: before.to_owned(),
            after: after.to_owned(),
            cursor,
        });
        let (x, y) = rig.space();
        let t = rig.t + 700;
        rig.send(Event::Down { pointer: 0, x, y, t });
        rig.send(Event::Move { pointer: 0, x: x + 30.0, y, t: t + 20 });
        rig.send(Event::Move { pointer: 0, x: x + distance, y, t: t + 30 })
    }

    #[test]
    fn dragging_space_right_moves_forward_by_whole_characters() {
        assert!(selects(&drag_right("ab", "cdef", 2, 60.0), 4, 4));
        let step = drag_right("a", "👍🏽b", 1, 40.0);
        assert!(selects(&step, 5, 5), "{:?}", step.commands);
    }

    #[test]
    fn dragging_past_the_end_of_the_text_does_nothing() {
        let step = drag_right("abc", "", 3, 60.0);
        assert!(!step.commands.iter().any(|c| matches!(c, Command::Select { .. })));
    }

    #[test]
    fn a_screen_reader_hears_each_key_once() {
        let mut rig = plain_rig();
        let (x, y) = rig.center("a");
        let first = rig.send(Event::Hover { x, y });
        assert!(first.commands.contains(&Command::Announce { text: "a".to_owned(), icon: Icon::None }));
        let same = rig.send(Event::Hover { x: x + 1.0, y });
        assert!(!same.commands.iter().any(|c| matches!(c, Command::Announce { .. })));
        let (dx, dy) = rig.center("⌫");
        let delete = rig.send(Event::Hover { x: dx, y: dy });
        assert!(delete.commands.iter().any(|c| matches!(c, Command::Announce { icon: Icon::Delete, .. })));
    }

    #[test]
    fn right_after_a_tap_the_space_bar_needs_a_longer_drag() {
        let mut rig = plain_rig();
        rig.tap("a");
        let (x, y) = rig.space();
        let t = rig.t + 140;
        rig.send(Event::Down { pointer: 0, x, y, t });
        rig.send(Event::Move { pointer: 0, x: x - 40.0, y, t: t + 10 });
        assert!(matches!(rig.engine.touch, Touch::Key { .. }));
        rig.send(Event::Move { pointer: 0, x: x - 70.0, y, t: t + 20 });
        assert!(matches!(rig.engine.touch, Touch::Cursor { .. }));
    }

    #[test]
    fn dragging_delete_selects_words_and_lifting_deletes_them() {
        let mut rig = rig_with_text("une deux trois");
        let (x, y) = rig.center("⌫");
        let t = rig.t + 700;
        rig.send(Event::Down { pointer: 0, x, y, t });
        rig.send(Event::Move { pointer: 0, x: x - 44.0, y, t: t + 10 });
        let step = rig.send(Event::Move { pointer: 0, x: x - 50.0, y, t: t + 20 });
        assert!(selects(&step, 9, 14), "{:?}", step.commands);
        let step = rig.send(Event::Move { pointer: 0, x: x - 110.0, y, t: t + 30 });
        assert!(selects(&step, 4, 14), "{:?}", step.commands);
        let step = rig.send(Event::Up { pointer: 0, x: x - 110.0, y, t: t + 40 });
        assert!(step.commands.contains(&Command::DeleteSelection));
        assert!(step.commands.contains(&Command::RequestContext));
    }

    #[test]
    fn a_small_drag_on_delete_stays_a_tap() {
        let mut rig = rig_with_text("ab");
        let (x, y) = rig.center("⌫");
        let t = rig.t + 700;
        rig.send(Event::Down { pointer: 0, x, y, t });
        rig.send(Event::Move { pointer: 0, x: x - 40.0, y, t: t + 10 });
        rig.send(Event::Up { pointer: 0, x: x - 40.0, y, t: t + 20 });
        assert_eq!(rig.text, "a");
    }

    #[test]
    fn each_key_arms_its_own_deadline() {
        let mut rig = plain_rig();
        let t = rig.t + 700;
        let due = |rig: &mut Rig, point: (f32, f32)| {
            let step = rig.send(Event::Down { pointer: 0, x: point.0, y: point.1, t });
            rig.send(Event::Cancel);
            step.deadline
        };
        let (space, delete, shift, letter) =
            (rig.space(), rig.center("⌫"), rig.center("⇧"), rig.center("a"));
        assert_eq!(due(&mut rig, space), Some(t + MENU_PRESS_MS));
        assert_eq!(due(&mut rig, delete), Some(t + DELETE_DELAY_MS));
        assert_eq!(due(&mut rig, shift), Some(t + MENU_PRESS_MS));
        assert_eq!(due(&mut rig, letter), Some(t + LONG_PRESS_MS));
        let enter = rig.engine.board.as_ref().unwrap().keys.iter().find(|k| k.action == Action::Enter).unwrap().center();
        assert_eq!(due(&mut rig, enter), None);
    }

    #[test]
    fn an_early_deadline_does_nothing_and_a_late_one_fires() {
        let mut rig = plain_rig();
        let (x, y) = rig.center("e");
        let t = rig.t + 700;
        rig.send(Event::Down { pointer: 0, x, y, t });
        let early = rig.send(Event::Deadline { t: t + LONG_PRESS_MS - 1 });
        assert!(!early.commands.iter().any(|c| matches!(c, Command::Popup { .. })));
        assert_eq!(early.deadline, Some(t + LONG_PRESS_MS));
        let late = rig.send(Event::Deadline { t: t + LONG_PRESS_MS + 5 });
        assert!(late.commands.iter().any(|c| matches!(c, Command::Popup { .. })));
    }

    #[test]
    fn holding_space_opens_the_picker_and_types_nothing() {
        let mut rig = plain_rig();
        let (x, y) = rig.space();
        let t = rig.t + 700;
        rig.send(Event::Down { pointer: 0, x, y, t });
        let step = rig.send(Event::Deadline { t: t + MENU_PRESS_MS });
        assert!(step.commands.contains(&Command::Picker));
        assert_eq!(step.deadline, None);
        rig.send(Event::Up { pointer: 0, x, y, t: t + 600 });
        assert_eq!(rig.text, "");
    }

    #[test]
    fn holding_shift_locks_capitals() {
        let mut rig = plain_rig();
        let (x, y) = rig.center("⇧");
        let t = rig.t + 700;
        rig.send(Event::Down { pointer: 0, x, y, t });
        rig.send(Event::Deadline { t: t + MENU_PRESS_MS });
        rig.send(Event::Up { pointer: 0, x, y, t: t + 600 });
        assert_eq!(rig.engine.shift, Shift::Locked);
        rig.tap("a");
        rig.tap("z");
        assert_eq!(rig.text, "AZ");
        rig.tap("⇧");
        rig.tap("a");
        assert_eq!(rig.text, "AZa");
    }

    #[test]
    fn one_shift_tap_lasts_one_letter() {
        let mut rig = plain_rig();
        rig.tap("⇧");
        assert_eq!(rig.engine.shift, Shift::OneShot);
        rig.tap("a");
        rig.tap("z");
        assert_eq!(rig.text, "Az");
    }

    #[test]
    fn a_shifted_popup_offers_capitals() {
        let mut rig = plain_rig();
        rig.tap("⇧");
        let (x, y) = rig.center("e");
        let t = rig.t + 700;
        rig.send(Event::Down { pointer: 0, x, y, t });
        let step = rig.send(Event::Deadline { t: t + LONG_PRESS_MS });
        assert!(step.commands.iter().any(|c| matches!(c, Command::Popup { items, .. } if items[0] == "É")));
    }

    #[test]
    fn the_popup_stays_on_screen_and_above_its_key() {
        let mut rig = plain_rig();
        let (x, y) = rig.center("p");
        let t = rig.t + 700;
        rig.send(Event::Down { pointer: 0, x, y, t });
        let step = rig.send(Event::Deadline { t: t + LONG_PRESS_MS });
        let popup = step.commands.iter().find_map(|c| match c {
            Command::Popup { x, y, item_width, items, .. } => Some((*x, *y, *item_width, items.len())),
            _ => None,
        });
        let (left, top, width, count) = popup.unwrap();
        assert!(left + width * count as f32 <= 1080.0 + 0.5);
        assert!(top < y);
        assert!(left >= 0.0);
        let item = POPUP_ITEM_DP * DENSITY;
        assert!((width - item).abs() < 0.01);
    }

    #[test]
    fn leaving_a_popup_below_cancels_when_another_item_was_reached() {
        let mut rig = plain_rig();
        let (x, y) = rig.center("e");
        let t = rig.t + 700;
        rig.send(Event::Down { pointer: 0, x, y, t });
        rig.send(Event::Deadline { t: t + LONG_PRESS_MS });
        let item = POPUP_ITEM_DP * DENSITY;
        let step = rig.send(Event::Move { pointer: 0, x: x + item * 1.1, y, t: t + 400 });
        assert!(step.commands.iter().any(|c| matches!(c, Command::Popup { selected: 1, .. })));
        let below = y + POPUP_BAND_DP * DENSITY + 10.0;
        let step = rig.send(Event::Move { pointer: 0, x: x + item * 1.1, y: below, t: t + 450 });
        assert!(step.commands.iter().any(|c| matches!(c, Command::Popup { selected: -1, .. })));
        rig.send(Event::Up { pointer: 0, x, y: below, t: t + 500 });
        assert_eq!(rig.text, "");
    }

    #[test]
    fn the_bar_has_three_slots_at_fixed_places() {
        let rig = plain_rig();
        assert_eq!(rig.engine.slot_at(0.0), 1);
        assert_eq!(rig.engine.slot_at(1080.0 * 0.32 - 1.0), 1);
        assert_eq!(rig.engine.slot_at(1080.0 * 0.32), 0);
        assert_eq!(rig.engine.slot_at(1080.0 * 0.68 - 1.0), 0);
        assert_eq!(rig.engine.slot_at(1080.0 * 0.68), 2);
        assert!((rig.engine.bar_height() - BAR_DP * DENSITY).abs() < 0.01);
    }

    #[test]
    fn a_tap_in_the_bar_is_not_a_key() {
        let mut rig = plain_rig();
        rig.tap_at((100.0, BAR_DP * DENSITY - 1.0));
        assert_eq!(rig.text, "");
        rig.tap_at((100.0, BAR_DP * DENSITY + 1.0));
        assert_eq!(rig.text.chars().count(), 1);
    }

    #[test]
    fn moving_the_cursor_away_ends_the_word_in_progress() {
        let mut rig = plain_rig();
        type_word(&mut rig, "yha");
        assert!(rig.engine.typing.is_some());
        let step = rig.send(Event::Selection { start: 0, end: 0 });
        assert!(rig.engine.typing.is_none());
        assert!(step.commands.contains(&Command::RequestContext));
    }

    #[test]
    fn the_keyboards_own_edits_are_not_taken_for_a_cursor_move() {
        let mut rig = plain_rig();
        type_word(&mut rig, "yha");
        let step = rig.send(Event::Selection { start: 3, end: 3 });
        assert!(rig.engine.typing.is_some());
        assert!(!step.commands.contains(&Command::RequestContext));
    }

    #[test]
    fn an_emptied_field_restarts_the_language_and_learns_the_last_word() {
        let mut rig = plain_rig();
        rig.engine.leaning = -2.0;
        rig.swipe("bonjour");
        rig.send(Event::Selection { start: 0, end: 0 });
        assert!((rig.engine.leaning - LANGUAGE_START).abs() < f32::EPSILON);
        assert!(rig.engine.memory.familiarity("bonjour") > 0.0);
    }

    #[test]
    fn language_evidence_decays_and_stays_bounded() {
        let mut rig = plain_rig();
        rig.engine.set_lexicon(Arc::new(Lexicon::with_languages([("the", 180.0, ENGLISH), ("le", 180.0, FRENCH), ("x", 1.0, 3)])));
        rig.engine.leaning = 1.0;
        rig.engine.update_leaning("the");
        assert!((rig.engine.leaning - (1.0 * LANGUAGE_MEMORY - 1.0)).abs() < 0.001);
        rig.engine.leaning = 1.0;
        rig.engine.update_leaning("x");
        assert!((rig.engine.leaning - 1.0).abs() < f32::EPSILON);
        for _ in 0..50 {
            rig.engine.update_leaning("le");
        }
        assert!(rig.engine.leaning <= LANGUAGE_LIMIT);
        for _ in 0..50 {
            rig.engine.update_leaning("the");
        }
        assert!(rig.engine.leaning >= -LANGUAGE_LIMIT);
    }

    #[test]
    fn the_language_tilt_only_moves_words_of_one_language() {
        assert!(language_tilt(FRENCH, 2.0) > 0.0);
        assert!(language_tilt(ENGLISH, 2.0) < 0.0);
        assert!(language_tilt(FRENCH | ENGLISH, 2.0).abs() < f32::EPSILON);
        assert!(language_tilt(0, 2.0).abs() < f32::EPSILON);
    }

    #[test]
    fn case_follows_the_model_word() {
        assert_eq!(case_like("été", "été"), "été");
        assert_eq!(case_like("été", "Bon"), "Été");
        assert_eq!(case_like("été", "BON"), "ÉTÉ");
        assert_eq!(case_like("été", "A"), "Été");
    }

    #[test]
    fn every_function_key_has_its_icon() {
        assert_eq!(icon_of(&Action::Shift), Icon::Shift);
        assert_eq!(icon_of(&Action::Delete), Icon::Delete);
        assert_eq!(icon_of(&Action::Enter), Icon::Enter);
        assert_eq!(icon_of(&Action::Emoji), Icon::Emoji);
        assert_eq!(icon_of(&Action::Space), Icon::None);
    }

    #[test]
    fn the_search_cross_acts_only_at_the_right_end() {
        let mut rig = plain_rig();
        rig.send(Event::OpenEmojiSearch);
        rig.tap("f");
        rig.tap_at((100.0, -10.0));
        assert_eq!(rig.engine.search.as_deref(), Some("f"));
        rig.tap_at((1080.0 - BAR_DP * DENSITY + 1.0, -10.0));
        assert_eq!(rig.engine.search.as_deref(), Some(""));
    }

    #[test]
    fn addresses_get_no_automatic_space() {
        let mut rig = Rig::new(Field { input_type: android::CLASS_TEXT | android::URI, ime_options: 0 });
        assert!(!rig.engine.field.allows_auto_spacing());
        rig.swipe("bonjour");
        rig.swipe("bonsoir");
        assert_eq!(rig.text, "bonjourbonsoir");
    }

    #[test]
    fn only_a_real_change_of_shift_relabels_the_keys() {
        let mut rig = plain_rig();
        let step = rig.engine.step(Event::Hidden);
        assert!(!step.commands.iter().any(|c| matches!(c, Command::Keys { .. })));
        rig.tap("⇧");
        let labels = rig.engine.step(Event::Resize { width: 1080.0, screen_height: 2424.0, density: DENSITY });
        let upper = labels.commands.iter().any(|c| matches!(c, Command::Keys { keys, .. } if keys.iter().any(|k| k.label == "A")));
        assert!(upper);
    }

    #[test]
    fn a_password_field_never_predicts() {
        let mut rig = Rig::new(Field { input_type: android::CLASS_TEXT | android::PASSWORD, ime_options: 0 });
        rig.engine.set_language(Arc::new(Affinities::default()), Arc::new(Grammar::default()), Arc::new(NextWords::parse("<s>\tje\n")));
        rig.engine.clear_suggestions();
        assert_eq!(rig.engine.predictions, [] as [std::string::String; 0]);
    }

    fn type_word(rig: &mut Rig, word: &str) {
        for letter in word.chars() {
            rig.tap(&letter.to_string());
        }
    }

    #[test]
    fn yhan_is_offered_as_than_and_corrected_at_the_space() {
        let mut rig = Rig::new(Field { input_type: android::CLASS_TEXT, ime_options: 0 });
        type_word(&mut rig, "yhan");
        let offered = rig.engine.typing.as_ref().map(|t| t.offered.clone()).unwrap_or_default();
        assert_eq!(offered.first().map(String::as_str), Some("than"));
        assert!(offered.contains(&"yhan".to_owned()));
        rig.tap_at(rig.space());
        assert_eq!(rig.text, "than ");
    }

    #[test]
    fn one_backspace_restores_the_typed_word_and_it_stays_protected() {
        let mut rig = Rig::new(Field { input_type: android::CLASS_TEXT, ime_options: 0 });
        type_word(&mut rig, "yhan");
        rig.tap_at(rig.space());
        rig.tap("⌫");
        assert_eq!(rig.text, "yhan ");
        type_word(&mut rig, "yhan");
        rig.tap_at(rig.space());
        assert_eq!(rig.text, "yhan yhan ");
    }

    #[test]
    fn a_valid_word_is_never_corrected() {
        let mut rig = Rig::new(Field { input_type: android::CLASS_TEXT, ime_options: 0 });
        type_word(&mut rig, "merge");
        rig.tap_at(rig.space());
        assert_eq!(rig.text, "merge ");
    }

    #[test]
    fn a_tap_on_the_bar_replaces_the_typed_word() {
        let mut rig = Rig::new(Field { input_type: android::CLASS_TEXT, ime_options: 0 });
        type_word(&mut rig, "yha");
        rig.tap_at((540.0, 10.0));
        assert_eq!(rig.text, "than");
    }

    #[test]
    fn a_held_suggestion_can_be_forgotten() {
        let mut rig = Rig::new(Field { input_type: android::CLASS_TEXT, ime_options: 0 });
        rig.engine.set_language(
            Arc::new(Affinities::default()),
            Arc::new(Grammar::default()),
            Arc::new(NextWords::parse("<s>\tje the\n")),
        );
        rig.engine.clear_suggestions();
        let t = rig.t + 700;
        rig.send(Event::Down { pointer: 0, x: 540.0, y: 10.0, t });
        let step = rig.send(Event::Deadline { t: t + LONG_PRESS_MS });
        assert!(step.commands.iter().any(|c| matches!(c, Command::ForgetBubble { word, .. } if word == "je")));
        rig.send(Event::Up { pointer: 0, x: 540.0, y: 10.0, t: t + LONG_PRESS_MS + 50 });
        assert_eq!(rig.engine.predictions, ["the"]);
        assert_eq!(rig.text, "");
    }

    #[test]
    fn dragging_away_keeps_the_suggestion() {
        let mut rig = Rig::new(Field { input_type: android::CLASS_TEXT, ime_options: 0 });
        rig.engine.set_language(Arc::new(Affinities::default()), Arc::new(Grammar::default()), Arc::new(NextWords::parse("<s>\tje\n")));
        rig.engine.clear_suggestions();
        let t = rig.t + 700;
        rig.send(Event::Down { pointer: 0, x: 540.0, y: 10.0, t });
        rig.send(Event::Deadline { t: t + LONG_PRESS_MS });
        rig.send(Event::Up { pointer: 0, x: 540.0, y: 500.0, t: t + LONG_PRESS_MS + 50 });
        assert_eq!(rig.engine.predictions, ["je"]);
    }

    #[test]
    fn a_held_delete_takes_two_letters_then_whole_words() {
        let mut rig = Rig::new(Field { input_type: android::CLASS_TEXT, ime_options: 0 });
        rig.send(Event::Text { text: "une deux trois".to_owned() });
        let (x, y) = rig.center("⌫");
        let t = rig.t + 700;
        rig.send(Event::Down { pointer: 0, x, y, t });
        rig.send(Event::Deadline { t: t + DELETE_DELAY_MS });
        assert_eq!(rig.text, "une deux troi");
        rig.send(Event::Deadline { t: t + DELETE_DELAY_MS + 50 });
        assert_eq!(rig.text, "une deux tro");
        rig.send(Event::Deadline { t: t + DELETE_DELAY_MS + 100 });
        assert_eq!(rig.text, "une deux ");
        rig.send(Event::Deadline { t: t + DELETE_DELAY_MS + 220 });
        assert_eq!(rig.text, "une ");
    }

    #[test]
    fn repeated_phrasing_is_predicted() {
        let mut rig = Rig::new(Field { input_type: android::CLASS_TEXT, ime_options: 0 });
        rig.swipe("bonjour");
        rig.swipe("bonsoir");
        rig.swipe("été");
        rig.tap_at(rig.space());
        assert_eq!(rig.engine.memory.next_after("bonjour"), ["bonsoir"]);
        rig.send(Event::Hidden);
        let step = rig.engine.step(Event::Hidden);
        assert!(step.commands.iter().all(|c| !matches!(c, Command::SaveMemory { .. })));
    }

    #[test]
    fn a_word_deleted_at_once_is_not_learned() {
        let mut rig = Rig::new(Field { input_type: android::CLASS_TEXT, ime_options: 0 });
        rig.swipe("bonjour");
        rig.tap("⌫");
        rig.swipe("bonsoir");
        rig.swipe("été");
        assert_eq!(rig.engine.memory.next_after("<s>"), ["bonsoir"]);
    }

    #[test]
    fn a_no_learning_field_learns_nothing() {
        let flags = android::CLASS_TEXT;
        let mut rig = Rig::new(Field { input_type: flags, ime_options: android::NO_PERSONALIZED_LEARNING });
        rig.swipe("bonjour");
        rig.swipe("bonsoir");
        rig.send(Event::Hidden);
        assert_eq!(rig.engine.memory.to_text(), "");
    }

    #[test]
    fn a_word_typed_twice_is_learned_and_not_corrected() {
        let mut rig = Rig::new(Field { input_type: android::CLASS_TEXT, ime_options: 0 });
        type_word(&mut rig, "zxcv");
        rig.tap_at(rig.space());
        type_word(&mut rig, "zxcv");
        rig.tap_at(rig.space());
        rig.swipe("ou");
        assert!(rig.engine.is_valid_word("zxcv"));
    }

    #[test]
    fn a_password_field_is_never_corrected() {
        let mut rig = Rig::new(Field { input_type: android::CLASS_TEXT | android::PASSWORD, ime_options: 0 });
        type_word(&mut rig, "yhan");
        rig.tap_at(rig.space());
        assert_eq!(rig.text, "yhan ");
    }

    #[test]
    fn a_password_field_does_not_remember_emoji() {
        let mut rig = Rig::new(Field { input_type: android::CLASS_TEXT | android::PASSWORD, ime_options: 0 });
        rig.send(Event::Text { text: "🍕".to_owned() });
        assert_eq!(rig.engine.recents, [] as [std::string::String; 0]);
    }

    #[test]
    fn sentence_rules_skip_abbreviations() {
        assert!(opens_sentence(""));
        assert!(opens_sentence("Fin. "));
        assert!(opens_sentence("Ligne\n"));
        assert!(!opens_sentence("Bonjour M. "));
        assert!(!opens_sentence("le 3. "));
        assert!(!opens_sentence("mot "));
    }

    #[test]
    fn delete_removes_a_whole_emoji() {
        let mut rig = Rig::new(Field { input_type: android::CLASS_TEXT, ime_options: 0 });
        rig.send(Event::Text { text: "👍🏽".to_owned() });
        assert_eq!(rig.engine.recents, ["👍🏽"]);
        let (x, y) = rig.center("⌫");
        let t = rig.t + 700;
        let before = rig.engine.before.clone();
        rig.send(Event::Down { pointer: 0, x, y, t });
        let step = rig.send(Event::Up { pointer: 0, x, y, t: t + 30 });
        assert_eq!(before, "👍🏽");
        assert!(step.commands.contains(&Command::DeleteBefore { units: 4 }));
        assert_eq!(rig.engine.before, "");
    }
}
