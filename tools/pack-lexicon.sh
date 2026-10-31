#!/bin/sh
# Builds the keyboard's word list from the benchmark dictionaries: `word,weight`
# lines, French and English merged, with words below weight 20 dropped: they cost memory and win no gesture.
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
lists="$root/data/raw/heliboard-word-list-csvs/compressed/unsorted"
french=17965a492f20ab1ba06f95704f9754a1f82e54be6581c50bb1d11b37e12a85a0
english=bd950ef4b57655120eee65cee62a5d216a63f721d9a8bb759ce2022437840443
out="$root/android/app/src/main/assets/lexicon.csv"

mkdir -p "$(dirname "$out")"
# The third column is the language: 1 French, 2 English. A word only counts
# for a language where it is reasonably common, so that `il` in the English
# list does not make it English. Below that it stays usable, without language.
language() {
    zstd -dc "$1" | tail -n +2 | awk -F, -v bit="$2" 'NF == 2 && $2 >= 20 { print $1 "," $2 "," ($2 >= 50 ? bit : 0) }'
}
{
    language "$lists/$french.lex.csv.zst" 1
    language "$lists/$english.lex.csv.zst" 2
} > "$out"
wc -lc "$out"

# Context, from the subtitle counts in data/raw/ngrams.
ngrams="$root/data/raw/ngrams"
assets="$root/android/app/src/main/assets"
bin="$root/target/release"
"$bin/compile_affinity" "$assets/affinity.bin" 10 0.5 "$ngrams/fr.txt" "$ngrams/en.txt"
"$bin/next_words" "$assets/next.tsv" 200 "$ngrams/fr.txt" "$ngrams/en.txt"
