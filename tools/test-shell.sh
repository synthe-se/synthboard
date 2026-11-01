#!/bin/sh
# Runs the Android shell tests on SDK 35, the newest Robolectric runs on Java 17.
# Pass "record" to accept new screenshots.
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)

task=verifyRoborazziDebug
if [ "${1:-}" = record ]; then
    task=recordRoborazziDebug
fi
cd "$root/android"
./gradlew "$task" --console=plain -q
