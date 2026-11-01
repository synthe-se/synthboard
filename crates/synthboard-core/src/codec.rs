//! The format between Rust and the shell, and the one definition of what
//! crosses. Every length is checked against what arrived and every string is
//! checked as UTF-8, because the other side of a boundary proves nothing.
//! `wire_struct!` and `wire_enum!` describe each type once: Rust derives its
//! coding from that, and `kotlin_wire` generates the Kotlin side from `Shape`.

/// Whether a type has been written to or read from the wire.
pub trait Wire: Sized {
    fn put(&self, out: &mut Vec<u8>);
    fn take(reader: &mut Reader<'_>) -> Option<Self>;

    /// Writes many values: raw bytes have a faster way than one by one.
    fn put_many(values: &[Self], out: &mut Vec<u8>) {
        for value in values {
            value.put(out);
        }
    }

    /// Reads `count` values, which the caller has bounded.
    fn take_many(reader: &mut Reader<'_>, count: usize) -> Option<Vec<Self>> {
        (0..count).map(|_| Self::take(reader)).collect()
    }
}

/// Reads what arrived, and refuses to go past its end.
#[derive(Debug)]
pub struct Reader<'bytes> {
    bytes: &'bytes [u8],
}

impl<'bytes> Reader<'bytes> {
    #[must_use]
    pub const fn new(bytes: &'bytes [u8]) -> Self {
        Self { bytes }
    }

    /// Whether everything was read: anything left means the two sides disagree.
    #[must_use]
    pub const fn is_done(&self) -> bool {
        self.bytes.is_empty()
    }

    fn bytes(&mut self, count: usize) -> Option<&'bytes [u8]> {
        let (taken, rest) = self.bytes.split_at_checked(count)?;
        self.bytes = rest;
        Some(taken)
    }

    fn array<const N: usize>(&mut self) -> Option<[u8; N]> {
        let (taken, rest) = self.bytes.split_first_chunk::<N>()?;
        self.bytes = rest;
        Some(*taken)
    }

    /// A length, which no valid message can make larger than what is left.
    fn count(&mut self) -> Option<usize> {
        let count = usize::try_from(u32::take(self)?).ok()?;
        (count <= self.bytes.len()).then_some(count)
    }
}

macro_rules! numbers {
    ($($number:ty),*) => {$(
        impl Wire for $number {
            fn put(&self, out: &mut Vec<u8>) {
                out.extend_from_slice(&self.to_le_bytes());
            }

            fn take(reader: &mut Reader<'_>) -> Option<Self> {
                reader.array().map(Self::from_le_bytes)
            }
        }
    )*};
}

numbers!(u32, i32, u64, f32);

impl Wire for u8 {
    fn put(&self, out: &mut Vec<u8>) {
        out.push(*self);
    }

    fn take(reader: &mut Reader<'_>) -> Option<Self> {
        reader.array().map(|[byte]| byte)
    }

    fn put_many(values: &[Self], out: &mut Vec<u8>) {
        out.extend_from_slice(values);
    }

    fn take_many(reader: &mut Reader<'_>, count: usize) -> Option<Vec<Self>> {
        reader.bytes(count).map(<[u8]>::to_vec)
    }
}

impl Wire for bool {
    fn put(&self, out: &mut Vec<u8>) {
        out.push(u8::from(*self));
    }

    fn take(reader: &mut Reader<'_>) -> Option<Self> {
        match u8::take(reader)? {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        }
    }
}

fn put_length(length: usize, out: &mut Vec<u8>) {
    u32::try_from(length).unwrap_or(u32::MAX).put(out);
}

impl Wire for String {
    fn put(&self, out: &mut Vec<u8>) {
        put_length(self.len(), out);
        out.extend_from_slice(self.as_bytes());
    }

    fn take(reader: &mut Reader<'_>) -> Option<Self> {
        let count = reader.count()?;
        std::str::from_utf8(reader.bytes(count)?).ok().map(str::to_owned)
    }
}

impl<T: Wire> Wire for Vec<T> {
    fn put(&self, out: &mut Vec<u8>) {
        put_length(self.len(), out);
        T::put_many(self, out);
    }

    fn take(reader: &mut Reader<'_>) -> Option<Self> {
        let count = reader.count()?;
        T::take_many(reader, count)
    }
}

impl<T: Wire> Wire for Option<T> {
    fn put(&self, out: &mut Vec<u8>) {
        self.is_some().put(out);
        if let Some(value) = self {
            value.put(out);
        }
    }

    fn take(reader: &mut Reader<'_>) -> Option<Self> {
        if bool::take(reader)? { T::take(reader).map(Some) } else { Some(None) }
    }
}

/// One named value of a type, with its type as Rust spells it.
#[derive(Debug)]
pub struct Slot {
    pub name: &'static str,
    pub ty: &'static str,
}

#[derive(Debug)]
pub struct Variant {
    pub name: &'static str,
    pub slots: &'static [Slot],
}

/// What a type looks like on the wire.
#[derive(Debug)]
pub enum Shape {
    Struct { name: &'static str, slots: &'static [Slot] },
    Enum { name: &'static str, variants: &'static [Variant] },
}

/// A type whose Kotlin side can be generated.
pub trait Described {
    const SHAPE: Shape;
}

/// The wire tag of a variant: its position in the declaration.
#[must_use]
pub const fn tag_of(names: &[&str], name: &str) -> u8 {
    let mut tag = 0;
    while tag < names.len() {
        #[allow(clippy::indexing_slicing, clippy::cast_possible_truncation)]
        if names[tag].eq_ignore_ascii_case(name) {
            return tag as u8;
        }
        tag += 1;
    }
    0
}

/// Defines a struct and its place on the wire.
#[macro_export]
macro_rules! wire_struct {
    ($(#[$meta:meta])* pub struct $name:ident {
        $($(#[$slot_meta:meta])* pub $slot:ident: $ty:ty),* $(,)?
    }) => {
        $(#[$meta])*
        pub struct $name {
            $($(#[$slot_meta])* pub $slot: $ty),*
        }

        impl $crate::codec::Wire for $name {
            fn put(&self, out: &mut Vec<u8>) {
                $($crate::codec::Wire::put(&self.$slot, out);)*
            }

            fn take(reader: &mut $crate::codec::Reader<'_>) -> Option<Self> {
                Some(Self { $($slot: <$ty as $crate::codec::Wire>::take(reader)?),* })
            }
        }

        impl $crate::codec::Described for $name {
            const SHAPE: $crate::codec::Shape = $crate::codec::Shape::Struct {
                name: stringify!($name),
                slots: &[$($crate::codec::Slot { name: stringify!($slot), ty: stringify!($ty) }),*],
            };
        }
    };
}

/// Defines an enum and its place on the wire.
#[macro_export]
macro_rules! wire_enum {
    ($(#[$meta:meta])* pub enum $name:ident {
        $($(#[$variant_meta:meta])* $variant:ident $({ $($slot:ident: $ty:ty),* $(,)? })?),* $(,)?
    }) => {
        $(#[$meta])*
        pub enum $name {
            $($(#[$variant_meta])* $variant $({ $($slot: $ty),* })?),*
        }

        impl $name {
            const NAMES: &'static [&'static str] = &[$(stringify!($variant)),*];
        }

        impl $crate::codec::Wire for $name {
            fn put(&self, out: &mut Vec<u8>) {
                match self {
                    $(Self::$variant $({ $($slot),* })? => {
                        out.push(const { $crate::codec::tag_of(Self::NAMES, stringify!($variant)) });
                        $($($crate::codec::Wire::put($slot, out);)*)?
                    })*
                }
            }

            fn take(reader: &mut $crate::codec::Reader<'_>) -> Option<Self> {
                let tag = <u8 as $crate::codec::Wire>::take(reader)?;
                $(if tag == const { $crate::codec::tag_of(Self::NAMES, stringify!($variant)) } {
                    return Some(Self::$variant $({ $($slot: <$ty as $crate::codec::Wire>::take(reader)?),* })?);
                })*
                None
            }
        }

        impl $crate::codec::Described for $name {
            const SHAPE: $crate::codec::Shape = $crate::codec::Shape::Enum {
                name: stringify!($name),
                variants: &[$($crate::codec::Variant {
                    name: stringify!($variant),
                    slots: &[$($($crate::codec::Slot { name: stringify!($slot), ty: stringify!($ty) }),*)?],
                }),*],
            };
        }
    };
}

/// Decodes one whole message.
#[must_use]
pub fn decode<T: Wire>(bytes: &[u8]) -> Option<T> {
    let mut reader = Reader::new(bytes);
    let value = T::take(&mut reader)?;
    reader.is_done().then_some(value)
}

/// Encodes one message.
#[must_use]
pub fn encode<T: Wire>(value: &T) -> Vec<u8> {
    let mut out = Vec::new();
    value.put(&mut out);
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::{Command, Event, Field, Icon, Step};

    fn round_trip<T: Wire + PartialEq + std::fmt::Debug>(value: &T) {
        assert_eq!(decode::<T>(&encode(value)).as_ref(), Some(value));
    }

    #[test]
    fn events_and_commands_survive_the_wire() {
        round_trip(&Event::Hidden);
        round_trip(&Event::Move { pointer: -3, x: 1.5, y: 2.5, t: u64::MAX });
        let field = Field { input_type: 1, ime_options: 2 };
        round_trip(&Event::Shown { field, before: "été".to_owned(), after: String::new(), cursor: 3 });
        round_trip(&Step {
            commands: vec![
                Command::Announce { text: "é".to_owned(), icon: Icon::Enter },
                Command::SaveMemory { sealed: vec![0, 255] },
                Command::EmojiPanel { on: true, recents: vec!["🙂".to_owned()] },
            ],
            deadline: Some(7),
        });
    }

    #[test]
    fn plain_values_keep_their_value() {
        round_trip(&7_u8);
        round_trip(&false);
        round_trip(&true);
        round_trip(&None::<u64>);
        assert_eq!(tag_of(&["a"], "missing"), 0);
    }

    #[test]
    fn invalid_text_is_refused() {
        assert_eq!(decode::<String>(&[2, 0, 0, 0, 0xc3, 0x28]), None);
        assert_eq!(decode::<String>(&[1, 0, 0, 0, 0x97]), None);
    }

    #[test]
    fn a_length_past_the_end_is_refused_before_allocating() {
        assert_eq!(decode::<Vec<String>>(&[255, 255, 255, 255]), None);
        assert_eq!(decode::<Vec<u8>>(&[3, 0, 0, 0, 1]), None);
    }

    #[test]
    fn trailing_bytes_and_unknown_tags_are_refused() {
        assert_eq!(decode::<Event>(&[250]), None);
        assert_eq!(decode::<u8>(&[1, 2]), None);
        assert_eq!(decode::<bool>(&[2]), None);
    }

    #[test]
    fn tags_follow_the_declaration() {
        assert_eq!(tag_of(&["a", "b", "c"], "c"), 2);
        assert_eq!(encode(&Icon::Delete), [2]);
    }

    /// The Kotlin test reads and writes the same bytes, so that the two sides
    /// cannot drift apart.
    #[test]
    fn the_golden_message_matches_the_shell() {
        let event = Event::Down { pointer: 1, x: 2.0, y: 3.0, t: 4 };
        assert_eq!(
            synthboard_seal::to_hex(&encode(&event)),
            "050100000000000040000040400400000000000000"
        );
        let step = Step { commands: vec![Command::Commit { text: "é".to_owned() }], deadline: Some(9) };
        assert_eq!(synthboard_seal::to_hex(&encode(&step)), "010000000002000000c3a9010900000000000000");
    }
}
