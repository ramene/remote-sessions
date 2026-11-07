#!/bin/sh
# Package a built remote-sessions binary: release/out/remote-sessions-<version>-<label>.tar.gz plus a .sha256.
# usage: [OUT_DIR=dir] scripts/package.sh <label> <path to remote-sessions binary>
# The archive holds remote-sessions, LICENSE, README.md and the plugin folder (claude-mod/remote-sessions).
set -eu
label="$1"; binary="$2"
root="$(cd "$(dirname "$0")/.." && pwd)"
version="$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/crates/remote-sessions-cli/Cargo.toml" | head -1)"
name="remote-sessions-$version-$label"
out="${OUT_DIR:-$root/release/out}"; mkdir -p "$out"
stage="$out/$name"
rm -rf "$stage"; mkdir -p "$stage"
cp "$binary" "$stage/remote-sessions"; chmod 755 "$stage/remote-sessions"
cp "$root/LICENSE" "$stage/LICENSE"
cp "$root/README.md" "$stage/README.md"
cp -R "$root/claude-mod/remote-sessions" "$stage/plugin"
tar -C "$out" -czf "$out/$name.tar.gz" "$name"
( cd "$out" && if command -v sha256sum >/dev/null; then sha256sum "$name.tar.gz"; else shasum -a 256 "$name.tar.gz"; fi > "$name.tar.gz.sha256" )
rm -rf "$stage"
echo "$out/$name.tar.gz"
