#!/bin/bash
# The macOS runner image installs rustup with Homebrew, so its proxies live in
# /opt/homebrew/opt/rustup/bin while CARGO_HOME stays ~/.cargo. Tests marked
# `requires_rustup_stable` run `$CARGO_HOME/bin/cargo +stable`, so link the
# proxies there when they are missing.
set -euo pipefail
bin="${CARGO_HOME:-$HOME/.cargo}/bin"
mkdir -p "$bin"
for tool in cargo rustc rustup; do
    if [ ! -e "$bin/$tool" ]; then
        ln -s "$(command -v "$tool")" "$bin/$tool"
    fi
done
ls -l "$bin"
