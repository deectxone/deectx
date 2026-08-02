#!/usr/bin/env bash
# Builds the release binary and tarball, prints the SHA-256 for Homebrew.
# Run from the repo root:  ./scripts/release.sh
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

cargo build --release
bin="$root/target/release/deectx"
dist="$root/dist"
mkdir -p "$dist"
tar="$dist/deectx-x86_64-unknown-linux-gnu.tar.gz"
tar -C "$root/target/release" -czf "$tar" deectx
hash="$(shasum -a 256 "$tar" | awk '{print $1}')"
echo "Built $tar"
echo "SHA-256: $hash"
echo "Paste into install/brew/deectx.rb (sha256) after publishing the tag tarball."