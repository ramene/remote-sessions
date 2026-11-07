# Contributing to remote-sessions

Thanks for helping. remote-sessions is a small Rust workspace plus a Claude Code plugin, MIT licensed. Changes are welcome as issues and pull requests.

## Before you start

- **Bugs:** open an issue with the command you ran, what you expected, what happened, and the output of `remote-sessions --version` and `claude --version`.
- **Features:** open an issue first so we can agree on the shape before you write code. remote-sessions talks to Claude Code's session API, which Anthropic does not document; a change that depends on new or undocumented behavior needs a note on how it was checked.
- **Security issues:** do not open a public issue. Use **Security → Report a vulnerability** on GitHub so the report stays private.

## Layout

| Path | What it is |
|---|---|
| `crates/remote-sessions-core` | the library: session ids, the events API, the live stream, permission answers, transcripts, casts |
| `crates/remote-sessions-cli` | the `remote-sessions` program: verbs, `attach`, the recordings API, the MCP server |
| `claude-mod/remote-sessions` | the Claude Code plugin: slash commands, launcher, MCP config |
| `release/` | `build-all.sh` (macOS universal, Linux x86_64 and aarch64) and `RELEASE.md` |
| `CHANGELOG.md` | one entry per version |

## Build and test

You need a stable Rust toolchain (edition 2021) and `claude` on your PATH for the end-to-end verbs.

```bash
cargo build --locked
cargo test --workspace --locked
cargo fmt --all                            # format the files you touched
cargo clippy --workspace --all-targets      # add no new warnings in code you touch
claude plugin validate claude-mod/remote-sessions
```

Every pull request must pass `cargo test --workspace --locked`. The tree does not pass `cargo fmt --check` or `clippy -D warnings` yet; cleanup pull requests for that are welcome, kept separate from feature changes. Add tests next to the code you change; network calls are tested against the in-process fake server the existing tests use, never against a real account.

## Rules for changes

- **Keep the JSON contract.** Every verb prints one JSON object on stdout; failures print `{"ok":false,"error":"..."}` and exit 1. Do not add fields that change the meaning of existing ones.
- **Write actions stay explicit.** Anything that answers a permission prompt, sends a turn, interrupts or stops a session must be a separate verb the user runs on purpose. `stop` keeps its `--yes`.
- **Never print a token.** Tokens are read from the environment or a 0600 file and never echoed, logged or put in an error message.
- **Examples use placeholders only.** In docs, tests and fixtures use fake ids such as `session_01ExampleAAAAAAAAAAAAAAA` and neutral host names such as `mac-a`. Never paste real session ids, transcript content, machine names or account details.
- **Docs follow behavior.** If a verb, flag or default changes, update `README.md`, the plugin's command files and `CHANGELOG.md` in the same pull request.

## Pull requests

1. Fork, branch from `main`, keep the change focused.
2. Run the commands above.
3. Add a `CHANGELOG.md` line under the next version.
4. Open the pull request with what changed and how you checked it.

Signed commits are appreciated. By contributing you agree that your contribution is licensed under the MIT license in `LICENSE`.

## Releases

Maintainers cut releases with `release/build-all.sh` and the steps in `release/RELEASE.md`; the Homebrew formula lives in [ramene/homebrew-tap](https://github.com/ramene/homebrew-tap).
