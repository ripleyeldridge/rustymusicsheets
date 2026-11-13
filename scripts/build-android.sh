#!/usr/bin/env bash
set -euo pipefail
project_dir="$(CDPATH='' cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
source "$project_dir/scripts/android-env.sh"
if ! command -v cargo-apk >/dev/null; then
    printf '%s\n' "Install cargo-apk 0.10.0: cargo install cargo-apk --version 0.10.0 --locked --root \"\$HOME/.local\"" >&2
    exit 1
fi
# cargo-apk 0.10 does not forward target-specific rustflags; it constructs its own
# encoded flags. Supply page alignment in its supported input environment instead.
if [[ -n "${CARGO_ENCODED_RUSTFLAGS:-}" && -n "${RUSTFLAGS:-}" ]]; then
    printf '%s\n' "Set only one of RUSTFLAGS and CARGO_ENCODED_RUSTFLAGS before building." >&2
    exit 1
fi
if [[ -n "${CARGO_ENCODED_RUSTFLAGS:-}" ]]; then
    export CARGO_ENCODED_RUSTFLAGS="${CARGO_ENCODED_RUSTFLAGS}"$'\x1f'"-Clink-arg=-Wl,-z,max-page-size=16384"$'\x1f'"-Clink-arg=-Wl,-z,common-page-size=16384"
else
    export RUSTFLAGS="${RUSTFLAGS:-} -Clink-arg=-Wl,-z,max-page-size=16384 -Clink-arg=-Wl,-z,common-page-size=16384"
fi
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}"
# Keep Android's debuggable flag, but omit bulky DWARF data by default.
# Set CARGO_PROFILE_DEV_DEBUG=2 explicitly when native source-level debugging.
export CARGO_PROFILE_DEV_DEBUG="${CARGO_PROFILE_DEV_DEBUG:-0}"
lock_before="$(sha256sum "$project_dir/Cargo.lock")"
cargo metadata --manifest-path "$project_dir/Cargo.toml" --locked --format-version 1 >/dev/null
cargo apk build --manifest-path "$project_dir/Cargo.toml" --package rustymusicsheets --lib --target aarch64-linux-android "$@"
if [[ "$(sha256sum "$project_dir/Cargo.lock")" != "$lock_before" ]]; then
    printf '%s\n' "Cargo.lock changed during packaging; review it before distributing the APK." >&2
    exit 1
fi
