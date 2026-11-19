#!/usr/bin/env bash
# Bygger Linux-udgaven af smtp-graph (statisk, musl) til x86_64 og aarch64 og pakker den i
# udgivelse/linux/smtp-graph-<version>-linux-<arkitektur>.tar.gz.
#
# Køres i WSL (Ubuntu) fra Windows:  wsl -d Ubuntu -- bash /mnt/c/Arbejdsmappen/SMTP-Graph-rs/linux/byg-linux.sh
# Kræver: rustup-målene x86_64-unknown-linux-musl og aarch64-unknown-linux-musl, cargo-zigbuild
# og zig (her i ~/.zig-venv: uv venv ~/.zig-venv && VIRTUAL_ENV=~/.zig-venv uv pip install ziglang).
set -euo pipefail

KILDE="$(cd "$(dirname "$0")/.." && pwd)"
BYG="$HOME/byg/smtp-graph"
export PATH="$HOME/.zig-venv/bin:$HOME/.cargo/bin:$PATH"

# Byg uden for /mnt/c (meget hurtigere) – og uden testindstillinger med hemmeligheder.
mkdir -p "$BYG"
rsync -a --delete --exclude target --exclude test-irvold --exclude udgivelse --exclude vejledning "$KILDE/" "$BYG/"
cd "$BYG"

VER="$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)"
UD="$KILDE/udgivelse/linux"
mkdir -p "$UD"

for MAAL in x86_64-unknown-linux-musl aarch64-unknown-linux-musl; do
    ARK="${MAAL%%-*}"
    cargo zigbuild --release --no-default-features --bin smtp-graph --target "$MAAL"
    PAKKE="smtp-graph-$VER-linux-$ARK"
    rm -rf "/tmp/$PAKKE" && mkdir -p "/tmp/$PAKKE"
    cp "target/$MAAL/release/smtp-graph" "/tmp/$PAKKE/"
    cp linux/LAES-MIG.txt "/tmp/$PAKKE/"
    cp README.md "/tmp/$PAKKE/"
    chmod 755 "/tmp/$PAKKE/smtp-graph"
    chmod 644 "/tmp/$PAKKE/LAES-MIG.txt" "/tmp/$PAKKE/README.md"
    tar -C /tmp -czf "$UD/$PAKKE.tar.gz" "$PAKKE"
    echo "Pakket: $UD/$PAKKE.tar.gz"
done
