//! Commands sent to the Android shell.

crate::wire_enum! {
    /// One thing for the shell to do.
    #[derive(Debug, Clone, PartialEq)]
    pub enum Command {
        /// Writes text at the cursor.
        Commit { text: String },
        /// Deletes this many UTF-16 units before the cursor.
        DeleteBefore { units: u32 },
        /// Moves or sets the selection.
        Select { start: u32, end: u32 },
        /// Deletes the current selection.
        DeleteSelection,
        /// Sends a plain Enter to the field.
        NewLine,
        /// Runs the action the field asked for on its Enter key.
        EditorAction { action: u32 },
        /// Keeps the keyboard out of screenshots and recordings, or not.
        ProtectScreen { on: bool },
        /// Opens the system keyboard picker.
        Picker,
        /// Plays a haptic effect.
        Vibrate { kind: Haptic },
        /// Replaces the keys on screen, with the heights of the keyboard and its bar in pixels.
        Keys { height: f32, bar_height: f32, keys: Vec<DrawnKey> },
        /// Marks which key looks pressed, if any, and the shift look.
        Look { pressed: i32, shift: u8 },
        /// Shows an accent popup, or hides it when `items` is empty.
        Popup { x: f32, y: f32, item_width: f32, selected: i32, items: Vec<String> },
        /// Offers to forget a word, in a bubble over the bar.
        ForgetBubble { x: f32, width: f32, word: String },
        /// Offers to restore deleted text in the bar, or withdraws the offer.
        UndoBar { offered: bool },
        /// Fills the three suggestion slots: centre, left, right.
        Suggestions { center: String, left: String, right: String },
        /// Asks for the text before the cursor, answered by `Event::Context`.
        RequestContext,
        /// Asks the shell to start or stop drawing the finger trail.
        Trail { on: bool },
        /// Shows or hides the emoji panel, with the recent emoji.
        EmojiPanel { on: bool, recents: Vec<String> },
        /// Shows the emoji search row above the letters, or hides it when `on` is false.
        EmojiSearch { on: bool, query: String, results: Vec<String> },
        /// Names a key for touch exploration.
        Announce { text: String, icon: Icon },
        /// Appends a sealed record to the trace, as hex.
        Trace { record: String },
        /// Asks for memory to be sealed and stored, away from the typing thread.
        SaveMemory,
        /// Stores the recent emoji for the next start.
        SaveRecents { items: Vec<String> },
    }
}

crate::wire_enum! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Haptic {
        Tap,
        GestureStart,
        LongPress,
        Step,
    }
}

crate::wire_struct! {
    #[derive(Debug, Clone, PartialEq)]
    pub struct DrawnKey {
        pub x: f32,
        pub y: f32,
        pub width: f32,
        pub height: f32,
        pub label: String,
        pub hint: String,
        pub icon: Icon,
        pub is_function: bool,
        /// Drawn with fully rounded ends, like Gboard's `?123` and Enter.
        pub is_pill: bool,
    }
}

crate::wire_enum! {
    /// Function keys drawn as one family of vector icons, not as font glyphs.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Icon {
        None,
        Shift,
        Delete,
        Enter,
        Emoji,
    }
}
