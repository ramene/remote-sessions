#!/bin/sh
# Build the remote-sessions release archives for every target this machine can build, then write checksums.
#
#   release/build-all.sh
#
# Targets (labels used in the asset names): macos-universal (arm64 + x86_64 merged with lipo), linux-x86_64,
# linux-aarch64. macOS builds the first; Linux builds its own CPU natively; `cargo zigbuild`, when installed,
# cross-builds the other Linux target. A target this machine cannot build is reported as SKIPPED, not failed: build it
# elsewhere and copy its archive into release/out/, then run this script again (it rewrites release/out/SHA256SUMS
# over everything there). Exit 0 only when all three archives are present; exit 2 when some are missing.
# Set CARGO_TARGET_DIR to build somewhere other than ./target (for example an internal disk).
set -eu
root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"
version="$(sed -n 's/^version = "\(.*\)"/\1/p' crates/remote-sessions-cli/Cargo.toml | head -1)"
tdir="${CARGO_TARGET_DIR:-$root/target}"
out=release/out
mkdir -p "$out"
built=""; skipped=""

host() { rustc -vV | sed -n 's/^host: //p'; }
keep() { # label, binary
  scripts/package.sh "$1" "$2" >/dev/null
  built="$built $1"
  echo "built $1"
}
build() { # rust target [zig]
  if [ -n "${2:-}" ] && command -v cargo-zigbuild >/dev/null 2>&1 && [ "$(host)" != "$1" ]; then
    cargo zigbuild --release --locked -p remote-sessions-cli --target "$1.${LINUX_GLIBC:-2.28}"
  else
    cargo build --release --locked -p remote-sessions-cli --target "$1"
  fi
}

case "$(uname -s)" in
  Darwin)
    rustup target add aarch64-apple-darwin x86_64-apple-darwin >/dev/null 2>&1 || true
    build aarch64-apple-darwin; build x86_64-apple-darwin
    mkdir -p "$tdir/universal"
    lipo -create -output "$tdir/universal/remote-sessions" "$tdir/aarch64-apple-darwin/release/remote-sessions" "$tdir/x86_64-apple-darwin/release/remote-sessions"
    lipo -info "$tdir/universal/remote-sessions"
    keep macos-universal "$tdir/universal/remote-sessions"
    ;;
  *) skipped="$skipped macos-universal(needs a Mac)" ;;
esac
for pair in x86_64-unknown-linux-gnu:linux-x86_64 aarch64-unknown-linux-gnu:linux-aarch64; do
  triple="${pair%%:*}"; label="${pair##*:}"
  if [ "$(uname -s)" = Linux ] && [ "$(host)" = "$triple" ]; then
    build "$triple"; keep "$label" "$tdir/$triple/release/remote-sessions"
  elif command -v cargo-zigbuild >/dev/null 2>&1; then
    rustup target add "$triple" >/dev/null 2>&1 || true
    build "$triple" zig; keep "$label" "$tdir/$triple/release/remote-sessions"
  else
    skipped="$skipped $label"
  fi
done

( cd "$out" && rm -f SHA256SUMS && for f in remote-sessions-"$version"-*.tar.gz; do [ -f "$f" ] && cat "$f.sha256"; done > SHA256SUMS; cat SHA256SUMS )
echo "built:$built"
[ -z "$skipped" ] || echo "SKIPPED:$skipped"
missing=""
for label in macos-universal linux-x86_64 linux-aarch64; do [ -f "$out/remote-sessions-$version-$label.tar.gz" ] || missing="$missing $label"; done
[ -z "$missing" ] && { echo "all targets present: ready to tag remote-sessions-v$version (see release/RELEASE.md)"; exit 0; }
echo "MISSING archives:$missing (build them on the right machine, copy into release/out/, run this script again)"; exit 2
