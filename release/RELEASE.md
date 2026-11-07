# Releasing remote-sessions

A release is three archives plus checksums, attached to the GitHub release `remote-sessions-vX.Y.Z`:

| Asset | For |
| --- | --- |
| `remote-sessions-X.Y.Z-macos-universal.tar.gz` (+ `.sha256`) | any Mac (arm64 and Intel) |
| `remote-sessions-X.Y.Z-linux-x86_64.tar.gz` (+ `.sha256`) | Linux x86_64 |
| `remote-sessions-X.Y.Z-linux-aarch64.tar.gz` (+ `.sha256`) | Linux arm64 |
| `SHA256SUMS` | the Homebrew tap's `update-formula.sh`, and people verifying by hand |

Each archive is `remote-sessions-X.Y.Z-<label>/` holding `remote-sessions`, `LICENSE`, `README.md` and `plugin/` (the Claude Code plugin).

## Build

```
release/build-all.sh     # builds what this machine can, writes release/out/SHA256SUMS
```

* On a Mac: `macos-universal` (arm64 + x86_64 merged with `lipo`). On Linux: its own CPU; with `cargo zigbuild` installed, the other Linux target too.
* A target it cannot build is reported as SKIPPED and the script exits 2. Build the rest elsewhere, copy those archives into `release/out/`, run the script again: it rewrites `SHA256SUMS` over everything there. Exit 0 means all three are present.
* `scripts/package.sh <label> <binary>` makes one archive and its `.sha256` (the build script calls it).
* Set `CARGO_TARGET_DIR` to keep build output off a network disk.

## Publish

1. Bump `version` in `crates/remote-sessions-cli/Cargo.toml` (and `claude-mod/remote-sessions/VERSION`), update `CHANGELOG.md`, run `cargo test --workspace --locked`.
2. `release/build-all.sh` until it exits 0.
3. Create the GitHub release `remote-sessions-vX.Y.Z` and attach everything in `release/out/`. The assets must be public for `brew install`.
4. Tag `remote-sessions-vX.Y.Z` and push it.
5. In the Homebrew tap (`ramene/homebrew-tap`): `scripts/update-formula.sh remote-sessions X.Y.Z <path>/SHA256SUMS`, commit, push. Then `brew install ramene/tap/remote-sessions`.

Licence: MIT (`LICENSE`).
