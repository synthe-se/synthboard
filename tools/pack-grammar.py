"""Builds the French verb agreement asset from Lexique 3.83.

Each line is `form TAB mask`, where the bits are 1s 2s 3s 1p 2p 3p, then 64 for an
infinitive and 128 for a past participle, and 0 means a known word that is no verb form. Run with `python3 -I`.
"""
import csv
import pathlib
import sys

ROOT = pathlib.Path(sys.argv[0]).resolve().parent.parent
BITS = {"1s": 1, "2s": 2, "3s": 4, "1p": 8, "2p": 16, "3p": 32}
INFINITIVE = 64
PARTICIPLE = 128

masks = {}
with open(ROOT / "data/raw/lexique/Lexique383.tsv", encoding="utf-8", newline="") as source:
    for row in csv.DictReader(source, delimiter="\t", quoting=csv.QUOTE_NONE):
        form = row["ortho"].strip().lower()
        if not form or " " in form:
            continue
        mask = masks.setdefault(form, 0)
        if row["cgram"] in ("VER", "AUX"):
            for reading in row["infover"].split(";"):
                person = reading.split(":")[-1]
                if reading.startswith(("ind:", "sub:", "cnd:")):
                    mask |= BITS.get(person, 0)
                elif reading == "inf":
                    mask |= INFINITIVE
                elif reading.startswith("par:pas"):
                    mask |= PARTICIPLE
        masks[form] = mask

out = ROOT / "android/app/src/main/assets/forms.tsv"
out.write_text("".join(f"{form}\t{mask}\n" for form, mask in sorted(masks.items())), encoding="utf-8")
print(len(masks), "forms,", sum(1 for mask in masks.values() if mask), "conjugated")
