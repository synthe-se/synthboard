//! Keyboards described as data: a new layout is its rows and one name here.

use synthswipe::{Key as DecoderKey, Layout, is_key_label};

/// What pressing a key does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Text(String),
    Shift,
    Delete,
    Space,
    Enter,
    Layer(Layer),
    Emoji,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Layer {
    #[default]
    Alpha,
    Symbols,
    MoreSymbols,
    /// Digits for number and date fields.
    Numbers,
    /// The dial pad for phone fields.
    Phone,
    /// Digits with `/` and `:`, for date and time fields.
    Date,
}

/// What the bottom row offers next to the space bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Purpose {
    #[default]
    Text,
    /// No emoji, no gestures.
    Password,
    /// `/` next to the space bar.
    Url,
    /// `@` next to the space bar.
    Email,
}

/// Alphabetic layouts. Both produce the same behaviour from the same code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Alphabet {
    #[default]
    Azerty,
    Qwerty,
}

const AZERTY: &str = "a z e r t y u i o p\n\
                      q s d f g h j k l m\n\
                      shift:1.5 w x c v b n ' del:1.5";
const QWERTY: &str = "q w e r t y u i o p\n\
                      gap:0.5 a s d f g h j k l gap:0.5\n\
                      shift:1.5 z x c v b n m del:1.5";
const SYMBOLS: &str = "1 2 3 4 5 6 7 8 9 0\n\
                       @ # € % & - + ( ) /\n\
                       more:1.5 * \" ' : ; ! ? del:1.5";
const MORE_SYMBOLS: &str = "~ ` | • √ π ÷ × ¶ ∆\n\
                            £ $ ¥ ^ ° = { } [ ]\n\
                            sym:1.5 \\ © ® ™ « » _ del:1.5";
const NUMBERS: &str = "1 2 3 -:1\n\
                       4 5 6 ,\n\
                       7 8 9 del\n\
                       abc 0 . enter";
const PHONE: &str = "1 2 3 +\n\
                     4 5 6 *\n\
                     7 8 9 del\n\
                     # 0 space enter";
const DATE: &str = "1 2 3 /\n\
                    4 5 6 :\n\
                    7 8 9 del\n\
                    abc 0 space enter";
const LETTER_BOTTOM: &str = "sym:1.5 , emoji space:4 . enter:1.5";
const SYMBOL_BOTTOM: &str = "abc:1.5 , emoji space:4 . enter:1.5";
const PASSWORD_BOTTOM: &str = "sym:1.5 , space:5 . enter:1.5";
const URL_BOTTOM: &str = "sym:1.5 / space:5 . enter:1.5";
const EMAIL_BOTTOM: &str = "sym:1.5 @ space:5 . enter:1.5";

const DIGITS: &str = "1234567890";

/// Accent popups by base letter, first listed preselected (A-1).
const POPUPS: &[(char, &str)] = &[
    ('a', "à â æ ä"),
    ('e', "é è ê ë €"),
    ('i', "î ï"),
    ('o', "ô œ ö"),
    ('u', "ù û ü"),
    ('c', "ç"),
    ('y', "ÿ"),
    ('n', "ñ"),
    ('\'', "’ ‹ ›"),
    ('"', "« » “ ”"),
];

/// Endings offered by the `.` of an address field.
const ADDRESS_ENDINGS: &str = ".com .fr .org .net";

/// What a long press on a key offers, if anything, in a field of this purpose.
#[must_use]
pub fn popup_of(text: &str, purpose: Purpose) -> Vec<String> {
    if text == "." && matches!(purpose, Purpose::Url | Purpose::Email) {
        return ADDRESS_ENDINGS.split(' ').map(str::to_owned).collect();
    }
    let mut letters = text.chars();
    let (Some(base), None) = (letters.next(), letters.next()) else {
        return Vec::new();
    };
    let lower = base.to_lowercase().next().unwrap_or(base);
    POPUPS
        .iter()
        .find(|(key, _)| *key == lower)
        .map(|(_, items)| items.split(' ').map(str::to_owned).collect())
        .unwrap_or_default()
}

/// A key placed on the screen, in pixels.
#[derive(Debug, Clone, PartialEq)]
pub struct Key {
    pub action: Action,
    pub label: String,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    /// The small character shown in a corner and offered on long press.
    pub hint: String,
}

impl Key {
    #[must_use]
    pub fn center(&self) -> (f32, f32) {
        (self.x + self.width / 2.0, self.y + self.height / 2.0)
    }

    fn distance_squared(&self, x: f32, y: f32) -> f32 {
        let dx = (self.x - x).max(x - (self.x + self.width)).max(0.0);
        let dy = (self.y - y).max(y - (self.y + self.height)).max(0.0);
        dx * dx + dy * dy
    }
}

fn parse_token(token: &str) -> (Option<Action>, String, f32) {
    let (name, width) = match token.rsplit_once(':') {
        Some((name, width)) if !name.is_empty() => (name, width.parse().unwrap_or(1.0)),
        _ => (token, 1.0),
    };
    let (action, label) = match name {
        "gap" => (None, ""),
        "shift" => (Some(Action::Shift), "⇧"),
        "del" => (Some(Action::Delete), "⌫"),
        "space" => (Some(Action::Space), ""),
        "enter" => (Some(Action::Enter), "⏎"),
        "emoji" => (Some(Action::Emoji), "☺"),
        "sym" => (Some(Action::Layer(Layer::Symbols)), "?123"),
        "more" => (Some(Action::Layer(Layer::MoreSymbols)), "=\\<"),
        "abc" => (Some(Action::Layer(Layer::Alpha)), LETTERS_LABEL),
        text => (Some(Action::Text(text.to_owned())), text),
    };
    (action, label.to_owned(), width)
}

/// The key that goes back to letters, in every language.
pub const LETTERS_LABEL: &str = "ABC";
/// Height of the accent popup. Taken from Gboard, like the rows.
pub const POPUP_HEIGHT_DP: f32 = 52.8;
/// Keys in a row of the digit pads.
const DIGIT_COLUMNS: f32 = 4.0;
/// Keys in the widest row of letters.
const LETTER_COLUMNS: f32 = 10.0;
/// Height of one key row with its gaps, in density-independent pixels. Taken
/// from Gboard on a Pixel 9a, so that thumb memory carries over.
pub const ROW_DP: f32 = 62.4;
/// Height of the suggestion bar (B-1).
pub const BAR_DP: f32 = 48.0;

/// The most of the screen's height the keyboard may take.
pub const MAX_HEIGHT_SHARE: f32 = 0.5;
/// Below this a key cannot be hit, whatever the screen.
const MIN_ROW_DP: f32 = 32.0;

/// The window a keyboard is laid out for, in pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Screen {
    pub width: f32,
    pub height: f32,
    pub density: f32,
}

/// One layer of a keyboard laid out for a screen.
#[derive(Debug, Clone)]
pub struct Board {
    pub keys: Vec<Key>,
    pub height: f32,
    pub layout: Option<Layout>,
    pub key_width: f32,
}

impl Board {
    /// Lays out a layer for a window `width` pixels wide.
    #[must_use]
    pub fn new(alphabet: Alphabet, layer: Layer, purpose: Purpose, screen: Screen, height_scale: f32) -> Self {
        let Screen { width, height, density } = screen;
        let bottom = match purpose {
            Purpose::Text => LETTER_BOTTOM,
            Purpose::Password => PASSWORD_BOTTOM,
            Purpose::Url => URL_BOTTOM,
            Purpose::Email => EMAIL_BOTTOM,
        };
        let rows = match (layer, alphabet) {
            (Layer::Alpha, Alphabet::Azerty) => [AZERTY, bottom],
            (Layer::Alpha, Alphabet::Qwerty) => [QWERTY, bottom],
            (Layer::Symbols, _) => [SYMBOLS, SYMBOL_BOTTOM],
            (Layer::MoreSymbols, _) => [MORE_SYMBOLS, SYMBOL_BOTTOM],
            (Layer::Numbers, _) => [NUMBERS, ""],
            (Layer::Phone, _) => [PHONE, ""],
            (Layer::Date, _) => [DATE, ""],
        };
        let lines: Vec<&str> = rows.iter().flat_map(|text| text.lines()).collect();
        let bar = BAR_DP * density;
        // Landscape has little height, and the text being typed must stay visible.
        let available = (height * MAX_HEIGHT_SHARE - bar).max(0.0) / lines.len().max(1) as f32;
        let row_height = (ROW_DP * density * height_scale).min(available.max(MIN_ROW_DP * density));
        let key_width = match layer {
            Layer::Numbers | Layer::Phone | Layer::Date => width / DIGIT_COLUMNS,
            _ => width / LETTER_COLUMNS,
        };
        let mut keys = Vec::new();
        let mut y = bar;
        for line in &lines {
            let tokens: Vec<_> = line.split_whitespace().map(parse_token).collect();
            let units: f32 = tokens.iter().map(|token| token.2).sum();
            let mut x = (width - units * key_width).max(0.0) / 2.0;
            for (action, label, units) in tokens {
                let key_span = units * key_width;
                if let Some(action) = action {
                    keys.push(Key {
                        action,
                        label,
                        x,
                        y,
                        width: key_span,
                        height: row_height,
                        hint: String::new(),
                    });
                }
                x += key_span;
            }
            y += row_height;
        }
        if layer == Layer::Alpha {
            let top_row = keys.iter_mut().take(DIGITS.len());
            for (key, digit) in top_row.zip(DIGITS.chars()) {
                key.hint = digit.to_string();
            }
        }
        let layout = (layer == Layer::Alpha)
            .then(|| {
                Layout::new(keys.iter().filter_map(|key| match &key.action {
                    Action::Text(text) => {
                        let mut letters = text.chars();
                        let label = letters.next()?;
                        (letters.next().is_none() && is_key_label(label)).then(|| {
                            let (x, y) = key.center();
                            DecoderKey { label, x, y }
                        })
                    }
                    _ => None,
                }))
                .ok()
            })
            .flatten();
        Self {
            keys,
            height: y,
            layout,
            key_width,
        }
    }

    /// The key at a point. Hit boxes fill every gap and edge, so any point on
    /// the keyboard reaches the nearest key (L-4).
    #[must_use]
    pub fn key_at(&self, x: f32, y: f32) -> Option<usize> {
        self.keys
            .iter()
            .enumerate()
            .min_by(|a, b| a.1.distance_squared(x, y).total_cmp(&b.1.distance_squared(x, y)))
            .map(|(index, _)| index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PHONE: Screen = Screen { width: 1080.0, height: 2424.0, density: 2.625 };

    fn index_of(board: &Board, text: &str) -> Option<usize> {
        board
            .keys
            .iter()
            .position(|key| key.action == Action::Text(text.to_owned()))
    }

    #[test]
    fn both_alphabets_have_all_letters_and_a_decoder_layout() {
        for alphabet in [Alphabet::Azerty, Alphabet::Qwerty] {
            let board = Board::new(alphabet, Layer::Alpha, Purpose::Text, PHONE, 1.0);
            assert!(board.layout.is_some());
            for letter in 'a'..='z' {
                assert!(index_of(&board, &letter.to_string()).is_some(), "{letter}");
            }
        }
    }

    #[test]
    fn rows_fit_the_window() {
        for alphabet in [Alphabet::Azerty, Alphabet::Qwerty] {
            for layer in [Layer::Alpha, Layer::Symbols, Layer::MoreSymbols, Layer::Numbers, Layer::Phone] {
                let board = Board::new(alphabet, layer, Purpose::Text, PHONE, 1.0);
                assert!(board.keys.iter().all(|key| key.x >= 0.0 && key.x + key.width <= 1080.5));
            }
        }
    }

    #[test]
    fn gaps_and_edges_resolve_to_the_nearest_key() {
        let board = Board::new(Alphabet::Azerty, Layer::Alpha, Purpose::Text, PHONE, 1.0);
        let a = index_of(&board, "a");
        let (_, row_y) = board.keys.first().map_or((0.0, 0.0), Key::center);
        assert_eq!(board.key_at(0.0, row_y), a);
        assert_eq!(board.key_at(-20.0, row_y), a);
        assert_eq!(board.key_at(5.0, -10.0), a);
    }

    #[test]
    fn a_password_board_has_no_emoji_key_and_addresses_get_their_symbol() {
        let keys = |purpose| Board::new(Alphabet::Azerty, Layer::Alpha, purpose, PHONE, 1.0).keys;
        assert!(!keys(Purpose::Password).iter().any(|key| key.action == Action::Emoji));
        assert!(keys(Purpose::Url).iter().any(|key| key.label == "/"));
        assert!(keys(Purpose::Email).iter().any(|key| key.label == "@"));
    }

    #[test]
    fn the_dial_pad_has_every_digit() {
        let board = Board::new(Alphabet::Azerty, Layer::Phone, Purpose::Text, PHONE, 1.0);
        for digit in "0123456789#*+".chars() {
            assert!(board.keys.iter().any(|key| key.label == digit.to_string()), "{digit}");
        }
    }

    #[test]
    fn inside_a_key_the_distance_is_zero_and_outside_it_grows() {
        let key = Key { action: Action::Space, label: String::new(), x: 10.0, y: 20.0, width: 10.0, height: 20.0, hint: String::new() };
        assert!(key.distance_squared(15.0, 30.0).abs() < f32::EPSILON);
        assert!((key.distance_squared(7.0, 30.0) - 9.0).abs() < 0.001);
        assert!((key.distance_squared(23.0, 30.0) - 9.0).abs() < 0.001);
        assert!((key.distance_squared(15.0, 16.0) - 16.0).abs() < 0.001);
        assert!((key.distance_squared(15.0, 44.0) - 16.0).abs() < 0.001);
        assert_eq!(key.center(), (15.0, 30.0));
    }

    #[test]
    fn landscape_leaves_half_the_screen_to_the_text() {
        let landscape = Screen { width: 2424.0, height: 1080.0, density: 2.625 };
        let board = Board::new(Alphabet::Azerty, Layer::Alpha, Purpose::Text, landscape, 1.0);
        assert!(board.height <= 1080.0 * MAX_HEIGHT_SHARE + 0.5, "{}", board.height);
        let portrait = Board::new(Alphabet::Azerty, Layer::Alpha, Purpose::Text, PHONE, 1.0);
        assert!((portrait.height - (BAR_DP + ROW_DP * 4.0) * 2.625).abs() < 0.5);
    }

    #[test]
    fn keys_stay_hittable_on_a_very_short_window() {
        let tiny = Screen { width: 2424.0, height: 300.0, density: 2.625 };
        let board = Board::new(Alphabet::Azerty, Layer::Alpha, Purpose::Text, tiny, 1.0);
        assert!(board.keys.iter().all(|key| key.height >= MIN_ROW_DP * 2.625 - 0.5));
    }

    #[test]
    fn the_height_setting_scales_the_rows() {
        let tall = Board::new(Alphabet::Azerty, Layer::Alpha, Purpose::Text, PHONE, 1.2).height;
        let usual = Board::new(Alphabet::Azerty, Layer::Alpha, Purpose::Text, PHONE, 1.0).height;
        assert!(tall > usual);
    }

    #[test]
    fn the_top_letter_row_carries_the_digits() {
        let board = Board::new(Alphabet::Qwerty, Layer::Alpha, Purpose::Text, PHONE, 1.0);
        let hints: String = board.keys.iter().map(|key| key.hint.as_str()).collect();
        assert_eq!(hints, DIGITS);
        assert_eq!(board.keys.first().map(|key| key.label.as_str()), Some("q"));
    }

    #[test]
    fn popups_come_from_data_and_ignore_case() {
        assert_eq!(popup_of("e", Purpose::Text).first().map(String::as_str), Some("é"));
        assert_eq!(popup_of("E", Purpose::Text).first().map(String::as_str), Some("é"));
        assert_eq!(popup_of("z", Purpose::Text), [] as [std::string::String; 0]);
    }
}
