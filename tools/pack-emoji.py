"""Builds the emoji asset from Unicode's emoji-test.txt and CLDR annotations.

Each line is: group, emoji, skin tone variants, then `|`-joined keywords with
the French and English names first. Run with `python3 -I tools/pack-emoji.py`.
"""
import json
import pathlib
import re
import sys

ROOT = pathlib.Path(sys.argv[0]).resolve().parent.parent
RAW = ROOT / "data/raw/emoji"
OUT = ROOT / "android/app/src/main/assets/emoji.tsv"
GENDERS = ("\u200d\u2642\ufe0f", "\u200d\u2640\ufe0f")
TONES = {"\U0001F3FB", "\U0001F3FC", "\U0001F3FD", "\U0001F3FE", "\U0001F3FF"}
SKIPPED_GROUP = "Component"


def annotations(language):
    merged = {}
    base = json.loads((RAW / f"ann-{language}.json").read_text("utf-8"))
    derived = json.loads((RAW / f"der-{language}.json").read_text("utf-8"))
    merged.update(derived["annotationsDerived"]["annotations"])
    merged.update(base["annotations"]["annotations"])
    return merged


def lookup(table, emoji):
    return table.get(emoji) or table.get(emoji.replace("️", "")) or {}


def keywords(tables, emoji):
    names, words = [], []
    for table in tables:
        entry = lookup(table, emoji)
        names += entry.get("tts", [])
        words += entry.get("default", [])
    seen = []
    for word in names + words:
        cleaned = word.replace("|", " ").replace("\t", " ").strip().lower()
        if cleaned and cleaned not in seen:
            seen.append(cleaned)
    return seen


def main():
    tables = [annotations("fr"), annotations("en")]
    groups, order, tones, genders = [], [], {}, {}
    group = None
    line_pattern = re.compile(r"^([0-9A-F ]+?)\s*; fully-qualified")
    for line in (RAW / "emoji-test.txt").read_text("utf-8").splitlines():
        if line.startswith("# group: "):
            group = line[len("# group: "):].strip()
            if group != SKIPPED_GROUP:
                groups.append(group)
            continue
        match = line_pattern.match(line)
        if not match or group == SKIPPED_GROUP:
            continue
        emoji = "".join(chr(int(code, 16)) for code in match.group(1).split())
        modifiers = [letter for letter in emoji if letter in TONES]
        gendered = next((sign for sign in GENDERS if emoji.endswith(sign)), None)
        if gendered and not modifiers:
            # Offered from the neutral emoji's long press, after its skin tones.
            genders.setdefault(emoji[: -len(gendered)], []).append(emoji)
        elif not modifiers:
            order.append((group, emoji))
        elif len(modifiers) == 1:
            base = "".join(letter for letter in emoji if letter not in TONES)
            for key in {base, base.replace("\ufe0f", "")}:
                tones.setdefault(key, []).append(emoji)

    OUT.parent.mkdir(parents=True, exist_ok=True)
    with OUT.open("w", encoding="utf-8") as out:
        known = {emoji for _, emoji in order} | {emoji.replace("\ufe0f", "") for _, emoji in order}
        orphans = [emoji for base, items in genders.items() if base not in known for emoji in items]
        order.extend(("People & Body", emoji) for emoji in orphans)
        for group, emoji in order:
            plain = emoji.replace("\ufe0f", "")
            variants = (tones.get(emoji) or tones.get(plain) or []) + genders.get(emoji, genders.get(plain, []))
            words = keywords(tables, emoji)
            out.write(f"{group}\t{emoji}\t{' '.join(variants)}\t{'|'.join(words)}\n")
    print(len(order), "emoji,", sum(1 for _, e in order if tones.get(e)), "with skin tones")


main()
