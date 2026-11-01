"""Builds the French verb agreement asset from Lexique 3.83.

Each line is `form TAB mask`, with the bits that `grammar.tsv` names, and 0 means a
known word that is no verb form. Run with `python3 -I`.
"""
import csv
import pathlib
import sys

ROOT = pathlib.Path(sys.argv[0]).resolve().parent.parent
ASSETS = ROOT / "android/app/src/main/assets"
RULES = [line.split("\t") for line in (ASSETS / "grammar.tsv").read_text("utf-8").splitlines()]
BITS = {rule[1]: int(rule[2]) for rule in RULES if rule[0] == "@bit"}
INFINITIVE = BITS.pop("infinitive")
PARTICIPLE = BITS.pop("participle")

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

out = ASSETS / "forms.tsv"
out.write_text("".join(f"{form}\t{mask}\n" for form, mask in sorted(masks.items())), encoding="utf-8")
print(len(masks), "forms,", sum(1 for mask in masks.values() if mask), "conjugated")
