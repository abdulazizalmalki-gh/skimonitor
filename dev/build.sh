#!/usr/bin/env bash
# Release build with local paths remapped: panic messages and embedded strings
# carry /home/user instead of this machine's real home. Use this to build any
# binary you might share or commit.
set -euo pipefail
cd "$(dirname "$0")/.."
export RUSTFLAGS="--remap-path-prefix=$HOME=/home/user"
cargo build --release "$@"
echo "built target/release/sshscope (paths remapped)"
