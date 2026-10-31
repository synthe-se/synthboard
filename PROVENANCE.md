# Provenance

Every external item that enters this repository is logged here, so that the licence can be chosen later with full knowledge.

## Data

| Source | Licence | Local path | Revision | Use |
|---|---|---|---|---|
| HeliBoard gesture data, codeberg.org/eclexic/heliboard-gesture-data | CC BY-SA 4.0 | `data/raw/heliboard-gesture-data` | `7d39bc9831d088192e133dae9ea6cd2a6127bae4` | Train and test |
| swipe.futo.org, huggingface.co/datasets/futo-org/swipe.futo.org | MIT | `data/raw/swipe.futo.org` | `d71bf5fd7f45b3e7c2ed2d76a21b0dbd3b4ba566` | Train and test |
| HeliBoard word lists, codeberg.org/eclexic/heliboard-word-list-csvs | GPL-3.0 | `data/raw/heliboard-word-list-csvs` | `76213fd8d4e6da7fec59e61b2d81574b1431ed13` | Benchmark lexicons only |
| OPUS OpenSubtitles v2018, object.pouta.csc.fi/OPUS-OpenSubtitles/v2018/mono | No open licence found; rights holders may ask for removal | `data/raw/ngrams` (aggregate counts only) | v2018, first 300M tokens per language | Next words and context table. Private use only: must be replaced before any publication |
| Lexique 3.83, lexique.org | CC BY-SA 4.0 | `data/raw/lexique` | 3.83 | French verb agreement |
| Unicode emoji-test.txt | Unicode License V3 | `data/raw/emoji` | 18.0 | Emoji catalogue |
| CLDR annotations, cldr-annotations-full | Unicode License V3 | `data/raw/emoji` | 49.0.0 | Emoji search keywords |

The assets in `android/app/src/main/assets` are generated from these sources by `tools/` and are not committed. Personal traces and learned words stay in `data/raw/personal`, which is ignored.

## Code

| Source | Licence | Files here | What was taken |
|---|---|---|---|

Code from a GPL source is allowed for private use, but it forces the GPL on any later publication.
