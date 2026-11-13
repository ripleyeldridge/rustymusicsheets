#!/usr/bin/env bash
set -euo pipefail
root="$(CDPATH='' cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
command -v inkscape >/dev/null || {
    printf '%s\n' 'Inkscape is required to regenerate the PNG icons.' >&2
    exit 1
}
for size in 16 24 32 48 64 128 256 512; do
    output="$root/assets/icons/${size}x${size}/rustymusicsheets.png"
    mkdir -p "$(dirname "$output")"
    inkscape "$root/assets/rustymusicsheets.svg" \
        --export-width="$size" --export-height="$size" \
        --export-filename="$output"
done
