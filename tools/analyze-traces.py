"""Reports how swipe typing went, from the keyboard's local trace.

The main figure is the share of gestures kept without any correction, which is
the measure that counts: dataset scores only catch regressions. It also lists
the confusions to fix and how far touches land from key centres, per key.
Run with `python3 -I tools/analyze-traces.py data/raw/personal/traces.tsv`.
"""
import collections
import sys
import time

KEEP = "kept"
PICKED = "picked"
DELETED = "deleted"
MIN_KEY_SAMPLES = 3
SHOWN_CONFUSIONS = 15


def parse(path):
    """Yields one dict per gesture, with its fate and the word really meant."""
    day = "unknown"
    layout = {}
    gesture = None
    with open(path, encoding="utf-8") as source:
        for line in source:
            fields = line.rstrip("\n").split("\t")
            kind = fields[0]
            if kind == "S" and len(fields) > 1:
                day = time.strftime("%Y-%m-%d", time.localtime(int(fields[1])))
            elif kind == "K" and len(fields) > 1 and fields[1]:
                keys = {}
                for key in fields[1].split(" "):
                    parts = key.split(":")
                    if len(parts) == 5 and parts[0]:
                        keys[parts[0].lower()] = tuple(float(value) for value in parts[1:])
                if keys:
                    layout = keys
            elif kind == "G" and len(fields) >= 4:
                if gesture:
                    yield gesture
                points = [tuple(float(v) for v in p.split(",")) for p in fields[3].split(" ") if p]
                gesture = dict(day=day, word=fields[1], truth=fields[1], fate=KEEP, points=points, layout=layout)
            elif gesture and kind == "C" and len(fields) > 1:
                gesture.update(truth=fields[1], fate=PICKED)
            elif gesture and kind == "D":
                gesture.update(truth=None, fate=DELETED)
            elif gesture and kind == "F" and len(fields) > 1 and gesture["fate"] == DELETED:
                gesture["truth"] = fields[1]
    if gesture:
        yield gesture


def main():
    gestures = list(parse(sys.argv[1]))
    if not gestures:
        print("No gestures yet.")
        return

    print(f"\n{'Day':<12}{'Gestures':>9}{'Kept':>8}{'Picked':>8}{'Deleted':>9}")
    by_day = collections.defaultdict(collections.Counter)
    for gesture in gestures:
        by_day[gesture["day"]][gesture["fate"]] += 1
    for day, fates in sorted(by_day.items()):
        total = sum(fates.values())
        print(f"{day:<12}{total:>9}{100 * fates[KEEP] / total:>7.0f}%{fates[PICKED]:>8}{fates[DELETED]:>9}")

    confusions = collections.Counter()
    for gesture in gestures:
        if gesture["fate"] != KEEP:
            meant = gesture["truth"] or "?"
            confusions[f"{meant} <- {gesture['word']}"] += 1
    if confusions:
        print("\nMost frequent confusions (meant <- written):")
        for pair, count in confusions.most_common(SHOWN_CONFUSIONS):
            print(f"  {count:>3}  {pair}")

    # The touch-down and lift-off show aim best, since each is one known key.
    offsets = collections.defaultdict(list)
    for gesture in gestures:
        word = (gesture["truth"] or "").lower().replace("'", "")
        keys = gesture["layout"]
        if not word or not gesture["points"]:
            continue
        for letter, point in ((word[0], gesture["points"][0]), (word[-1], gesture["points"][-1])):
            if letter in keys:
                x, y, width, height = keys[letter]
                offsets[letter].append(((point[0] - x) / width, (point[1] - y) / height))
    shown = {k: v for k, v in offsets.items() if len(v) >= MIN_KEY_SAMPLES}
    if shown:
        print("\nAim per key, in key sizes (+x right, +y down):")
        for letter, values in sorted(shown.items()):
            dx = sum(v[0] for v in values) / len(values)
            dy = sum(v[1] for v in values) / len(values)
            print(f"  {letter}  {dx:+.2f} {dy:+.2f}  ({len(values)} touches)")


main()
