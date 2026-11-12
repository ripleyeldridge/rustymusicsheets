#!/usr/bin/env bash
set -e
project_dir="$(CDPATH='' cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
binary="$project_dir/target/release/rustymusicsheets"
if [[ ! -x "$binary" ]]; then
    cargo build --locked --release --manifest-path "$project_dir/Cargo.toml"
fi
exec "$binary" "$@"
