"""Adds received trace files to the archive, keeping each gesture only once.

The phone's file is cumulative and may arrive several times, by Taildrop or by
cable. A block is a gesture line with the correction lines after it, and a
gesture seen before is dropped, unless the new copy knows what happened next.
Usage: `python3 -I tools/ingest-traces.py ARCHIVE RECEIVED...`.
"""
import os
import pathlib
import sys


def blocks(text):
    """Splits a trace into `(header lines, gesture block)` pairs, in order."""
    current = []
    for line in text.splitlines():
        if line.startswith("G\t"):
            if current:
                yield current
            current = [line]
        elif current and line[:2] in ("C\t", "F\t") or current and line == "D":
            current.append(line)
        else:
            if current:
                yield current
                current = []
            yield [line]
    if current:
        yield current


def main():
    os.umask(0o077)
    archive = pathlib.Path(sys.argv[1])
    archive.parent.mkdir(parents=True, exist_ok=True)
    known = archive.read_text("utf-8") if archive.exists() else ""
    kept = list(blocks(known))
    position = {block[0]: index for index, block in enumerate(kept) if block[0].startswith("G\t")}

    added = 0
    for name in sys.argv[2:]:
        for block in blocks(pathlib.Path(name).read_text("utf-8")):
            head = block[0]
            if not head.startswith("G\t"):
                if not kept or kept[-1] != block:
                    kept.append(block)
                continue
            if head not in position:
                position[head] = len(kept)
                kept.append(block)
                added += 1
            elif len(block) > len(kept[position[head]]):
                kept[position[head]] = block

    archive.write_text("".join(line + "\n" for block in kept for line in block), "utf-8")
    print(f"{added} new gestures.")


main()
