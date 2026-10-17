#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
# usage: run_all.sh DYNARMIC_SRC OUT_DIR APKLIBDIR...   (each APKLIBDIR holds the lib*.so of one APK)
# Runs census.py once per unique library (by sha256); writes OUT_DIR/<sha12>.json and OUT_DIR/index.tsv
set -euo pipefail
dyn=$1; out=$2; shift 2
here=$(cd "$(dirname "$0")" && pwd)
mkdir -p "$out"
: > "$out/index.tsv"
for d in "$@"; do
  for f in "$d"/*.so; do
    h=$(sha256sum "$f" | cut -c1-12)
    printf '%s\t%s\t%s\n' "$d" "$(basename "$f")" "$h" >> "$out/index.tsv"
    echo "$h $f"
  done
done | sort -u -k1,1 | xargs -P 16 -L 1 bash -c 'python3 -I '"$here"'/census.py "$1" '"$out"'/$0.json '"$dyn"' || echo FAIL $1' 
