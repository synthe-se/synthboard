//! Writes the Kotlin side of the wire to the path given, from the one
//! definition of each type in Rust, so that the two sides cannot disagree.

use std::fmt::Write as _;

use synthboard_core::codec::{Described, Shape, Slot, Variant};
use synthboard_core::{Command, DrawnKey, EmojiGroup, EmojiItem, Event, Field, Haptic, Icon, LETTERS_LABEL, POPUP_HEIGHT_DP, SEARCH_CELLS, SLOT_CENTERS, Step};
use synthboard_ffi::{Call, KEPT_BYTES, Reply};
use synthboard_seal::KEY_BYTES;

fn camel(name: &str) -> String {
    let mut parts = name.split('_');
    let first = parts.next().unwrap_or_default().to_owned();
    parts.fold(first, |text, part| {
        let mut letters = part.chars();
        let head = letters.next().map(|letter| letter.to_ascii_uppercase());
        text + &head.into_iter().chain(letters).collect::<String>()
    })
}

fn screaming(name: &str) -> String {
    let mut out = String::new();
    for (index, letter) in name.chars().enumerate() {
        if letter.is_uppercase() && index > 0 {
            out.push('_');
        }
        out.push(letter.to_ascii_uppercase());
    }
    out
}

fn inner<'a>(ty: &'a str, wrapper: &str) -> Option<&'a str> {
    ty.strip_prefix(wrapper)?.strip_prefix('<')?.strip_suffix('>')
}

/// The Kotlin type for a Rust type.
fn kotlin(ty: &str) -> String {
    match ty {
        "u8" => "UByte".to_owned(),
        "u32" => "UInt".to_owned(),
        "i32" => "Int".to_owned(),
        "u64" => "ULong".to_owned(),
        "f32" => "Float".to_owned(),
        "bool" => "Boolean".to_owned(),
        "Vec<u8>" => "ByteArray".to_owned(),
        _ => {
            if let Some(item) = inner(ty, "Vec") {
                format!("List<{}>", kotlin(item))
            } else if let Some(item) = inner(ty, "Option") {
                format!("{}?", kotlin(item))
            } else {
                ty.to_owned()
            }
        }
    }
}

/// The Kotlin expression that reads a value of this type from `r`.
fn read(ty: &str) -> String {
    match ty {
        "u8" | "u32" | "i32" | "u64" | "f32" => format!("r.{ty}()"),
        "bool" => "r.bool()".to_owned(),
        "String" => "r.string()".to_owned(),
        "Vec<u8>" => "r.bytes()".to_owned(),
        _ => {
            if let Some(item) = inner(ty, "Vec") {
                format!("r.list {{ {} }}", read(item))
            } else if let Some(item) = inner(ty, "Option") {
                format!("r.optional {{ {} }}", read(item))
            } else {
                format!("{ty}.read(r)")
            }
        }
    }
}

/// The Kotlin statement that writes `value`, of this type, to `w`.
fn write(ty: &str, value: &str) -> String {
    match ty {
        "u8" | "u32" | "i32" | "u64" | "f32" => format!("w.{ty}({value})"),
        "bool" => format!("w.bool({value})"),
        "String" => format!("w.string({value})"),
        "Vec<u8>" => format!("w.bytes({value})"),
        _ => {
            if let Some(item) = inner(ty, "Vec") {
                format!("w.list({value}) {{ {} }}", write(item, "it"))
            } else if let Some(item) = inner(ty, "Option") {
                format!("w.optional({value}) {{ {} }}", write(item, "it"))
            } else {
                format!("{value}.write(w)")
            }
        }
    }
}

fn spaceless(ty: &str) -> String {
    ty.split_whitespace().collect()
}

fn parameters(slots: &[Slot]) -> String {
    slots
        .iter()
        .map(|slot| format!("val {}: {}", camel(slot.name), kotlin(&spaceless(slot.ty))))
        .collect::<Vec<_>>()
        .join(", ")
}

fn reads(slots: &[Slot]) -> String {
    slots.iter().map(|slot| read(&spaceless(slot.ty))).collect::<Vec<_>>().join(", ")
}

fn writes(slots: &[Slot]) -> String {
    let mut out = String::new();
    for slot in slots {
        let _ = writeln!(out, "        {}", write(&spaceless(slot.ty), &camel(slot.name)));
    }
    out
}

fn structure(out: &mut String, name: &str, slots: &[Slot]) {
    let _ = writeln!(out, "data class {name}({}) {{", parameters(slots));
    let _ = writeln!(out, "    fun write(w: WireWriter) {{\n{}    }}\n", writes(slots));
    let _ = writeln!(out, "    companion object {{\n        fun read(r: WireReader) = {name}({})\n    }}\n}}\n", reads(slots));
}

fn plain_enum(out: &mut String, name: &str, variants: &[Variant]) {
    let names: Vec<String> = variants.iter().map(|variant| screaming(variant.name)).collect();
    let _ = writeln!(out, "enum class {name} {{\n    {};\n", names.join(", "));
    let _ = writeln!(out, "    fun write(w: WireWriter) = w.u8(ordinal.toUByte())\n");
    let _ = writeln!(out, "    companion object {{\n        fun read(r: WireReader) = entries[r.u8().toInt()]\n    }}\n}}\n");
}

fn sealed_enum(out: &mut String, name: &str, variants: &[Variant]) {
    let _ = writeln!(out, "sealed class {name} {{\n    abstract fun write(w: WireWriter)\n");
    for (tag, variant) in variants.iter().enumerate() {
        let head = if variant.slots.is_empty() {
            format!("data object {}", variant.name)
        } else {
            format!("data class {}({})", variant.name, parameters(variant.slots))
        };
        let _ = writeln!(out, "    {head} : {name}() {{");
        let _ = writeln!(out, "        override fun write(w: WireWriter) {{\n            w.u8({tag}u)");
        out.push_str(&writes(variant.slots).replace("        ", "            "));
        let _ = writeln!(out, "        }}\n    }}\n");
    }
    let _ = writeln!(out, "    companion object {{\n        fun read(r: WireReader): {name} = when (r.u8().toInt()) {{");
    for (tag, variant) in variants.iter().enumerate() {
        let made = if variant.slots.is_empty() {
            variant.name.to_owned()
        } else {
            format!("{}({})", variant.name, reads(variant.slots))
        };
        let _ = writeln!(out, "            {tag} -> {made}");
    }
    let _ = writeln!(out, "            else -> throw IllegalArgumentException(\"{name}\")\n        }}\n    }}\n}}\n");
}

fn generate(out: &mut String, shape: &Shape) {
    match shape {
        Shape::Struct { name, slots } => structure(out, name, slots),
        Shape::Enum { name, variants } if variants.iter().all(|variant| variant.slots.is_empty()) => {
            plain_enum(out, name, variants);
        }
        Shape::Enum { name, variants } => sealed_enum(out, name, variants),
    }
}

fn main() -> std::io::Result<()> {
    let path = std::env::args().nth(1).ok_or(std::io::ErrorKind::InvalidInput)?;
    let mut out = String::from("// Generated by kotlin_wire. Change the Rust types instead.\npackage app.synthboard.core\n\n");
    for shape in [
        &Field::SHAPE,
        &Event::SHAPE,
        &Command::SHAPE,
        &Step::SHAPE,
        &DrawnKey::SHAPE,
        &Icon::SHAPE,
        &Haptic::SHAPE,
        &EmojiItem::SHAPE,
        &EmojiGroup::SHAPE,
        &Call::SHAPE,
        &Reply::SHAPE,
    ] {
        generate(&mut out, shape);
    }
    let [left, center, right] = SLOT_CENTERS;
    let _ = writeln!(out, "val SLOT_CENTERS = listOf({left}f, {center}f, {right}f)");
    let _ = writeln!(out, "const val KEY_BYTES = {KEY_BYTES}");
    let _ = writeln!(out, "const val SEARCH_CELLS = {SEARCH_CELLS}");
    let _ = writeln!(out, "const val LETTERS_LABEL = \"{LETTERS_LABEL}\"");
    let _ = writeln!(out, "const val POPUP_HEIGHT_DP = {POPUP_HEIGHT_DP}f");
    let _ = writeln!(out, "const val KEPT_BYTES = {KEPT_BYTES}");
    std::fs::write(path, out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn generated(shape: &Shape) -> String {
        let mut out = String::new();
        generate(&mut out, shape);
        out
    }

    #[test]
    fn names_follow_kotlin() {
        assert_eq!(camel("bar_height"), "barHeight");
        assert_eq!(camel("x"), "x");
        assert_eq!(screaming("GestureStart"), "GESTURE_START");
        assert_eq!(screaming("Tap"), "TAP");
    }

    #[test]
    fn types_map_to_kotlin() {
        let pairs = [
            ("u8", "UByte"),
            ("u32", "UInt"),
            ("i32", "Int"),
            ("u64", "ULong"),
            ("f32", "Float"),
            ("bool", "Boolean"),
            ("String", "String"),
            ("Vec<u8>", "ByteArray"),
            ("Vec<String>", "List<String>"),
            ("Option<u64>", "ULong?"),
            ("DrawnKey", "DrawnKey"),
        ];
        for (rust, expected) in pairs {
            assert_eq!(kotlin(rust), expected);
        }
        assert_eq!(spaceless("Vec < u8 >"), "Vec<u8>");
    }

    #[test]
    fn reads_and_writes_mirror_each_other() {
        let pairs = [
            ("u32", "r.u32()", "w.u32(x)"),
            ("bool", "r.bool()", "w.bool(x)"),
            ("String", "r.string()", "w.string(x)"),
            ("Vec<u8>", "r.bytes()", "w.bytes(x)"),
            ("Vec<Icon>", "r.list { Icon.read(r) }", "w.list(x) { it.write(w) }"),
            ("Option<u64>", "r.optional { r.u64() }", "w.optional(x) { w.u64(it) }"),
        ];
        for (ty, reading, writing) in pairs {
            assert_eq!(read(ty), reading);
            assert_eq!(write(ty, "x"), writing);
        }
    }

    #[test]
    fn a_struct_is_a_data_class() {
        let code = generated(&Field::SHAPE);
        assert!(code.contains("data class Field(val inputType: UInt, val imeOptions: UInt) {"));
        assert!(code.contains("w.u32(inputType)"));
        assert!(code.contains("Field(r.u32(), r.u32())"));
    }

    #[test]
    fn an_enum_of_names_is_an_enum_class() {
        let code = generated(&Icon::SHAPE);
        assert!(code.contains("enum class Icon {"));
        assert!(code.contains("NONE, SHIFT, DELETE, ENTER, EMOJI;"));
    }

    #[test]
    fn an_enum_with_values_is_a_sealed_class_tagged_by_position() {
        let code = generated(&Event::SHAPE);
        assert!(code.contains("sealed class Event {"));
        assert!(code.contains("data object Hidden : Event()"));
        assert!(code.contains("data class Selection(val start: UInt, val end: UInt) : Event()"));
        assert!(code.contains("5 -> Down(r.i32(), r.f32(), r.f32(), r.u64())"));
    }
}
