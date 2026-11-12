# remote-sessions

[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE) [![Release](https://img.shields.io/github/v/release/ramene/remote-sessions?sort=semver&label=release)](https://github.com/ramene/remote-sessions/releases) [![Homebrew](https://img.shields.io/badge/homebrew-ramene%2Ftap-orange?logo=homebrew)](https://github.com/ramene/homebrew-tap) [![Claude Code plugin](https://img.shields.io/badge/Claude%20Code-plugin-d97757)](#install) [![Rust](https://img.shields.io/badge/rust-2021-dea584?logo=rust)](Cargo.toml) [![Platforms](https://img.shields.io/badge/platforms-macOS%20%7C%20Linux-lightgrey)](#install)

[English](README.md) · [中文 (README.zh-CN.md)](README.zh-CN.md)

## Contents

- [What it is, and how it differs from cloud-sessions](#what-it-is-and-how-it-differs-from-cloud-sessions)
- [Install](#install)
- [Setup: serve a Remote Control session](#setup-serve-a-remote-control-session)
- [The verbs, with examples](#the-verbs-with-examples)
- [Attach from another machine](#attach-from-another-machine)
- [Stop semantics](#stop-semantics)
- [Recording](#recording)
- [Limits](#limits)
- [Security model](#security-model)
- [License](#license)
- [Contributing](#contributing)

## What it is, and how it differs from cloud-sessions

Claude Code has two ways to run a session somewhere other than your terminal:

| | cloud-sessions | remote-sessions (this repo) |
|---|---|---|
| Where the session runs | Anthropic's cloud (claude.ai/code, routines) | Your own machine: a Mac, a Linux box, a VM |
| How it starts | cloud-sessions starts it for a repo | You run `claude remote-control --name <n>` on that machine |
| Who holds the process | Anthropic | You: it is an ordinary local worker process |
| Find a session | the tracked list | the worker processes on a host (`list`, over `ssh -n` for another host) |
| Stop | a Stop-button link | one SIGINT to the local worker |
| Starting sessions | yes | no: `claude remote-control` starts them |

The two mods share the event decoder and the cast renderer (copied, same authors). Anything that only exists in the cloud (starting cloud sessions, Stop-button links, batch collect) stays in cloud-sessions.

Out of scope here: approving permission prompts from outside (impossible by design, see [Stop semantics](#stop-semantics)) and any `--connect` TUI.

## Install

Three ways, no Rust needed for the first two.

1. **Claude Code plugin from GitHub** (the program is fetched and checksum-verified on first use):

```bash
claude plugin marketplace add ramene/remote-sessions
claude plugin install remote-sessions@remote-sessions
```

2. **Homebrew**: `brew install ramene/tap/remote-sessions`
3. **From source** (needs a Rust toolchain): `cargo install --path crates/remote-sessions-cli`

Requirements: `claude` (Claude Code CLI) on PATH, and, for `list --host`, `stop --host` and `cast --host`, key-based `ssh` to the other machine.

The plugin (slash commands `/remote-serve`, `/remote-list`, `/remote-send`, `/remote-read`, `/remote-stop`, plus an MCP server) lives in `claude-mod/remote-sessions`. Its launcher runs, in order: `$REMOTE_SESSIONS_BIN`; a copy bundled in the plugin; an earlier download in the plugin data folder; a `remote-sessions` already on PATH; else it downloads `remote-sessions-<version>-<macos-universal|linux-x86_64|linux-aarch64>.tar.gz` from the GitHub release `remote-sessions-v<version>` and checks its `.sha256` before using it. On failure it prints one plain sentence.

Environment variables:

| variable | used by | meaning |
|---|---|---|
| `REMOTE_SESSIONS_BIN` | plugin launcher | path of the binary to run |
| `REMOTE_SESSIONS_TOKEN` | `events` | API token; otherwise read from `~/.config/remote-sessions/token` (0600). Inside Claude Code `/remote-read` needs no token |
| `REMOTE_SESSIONS_CLAUDE` | `send`, `serve` | path of the `claude` binary |
| `REMOTE_SESSIONS_API` | `events` | API base URL: `https://api.anthropic.com` only (plain http only to a loopback test server) |
| `REMOTE_SESSIONS_MCP_APPROVE` | `mcp` | `1` lets the MCP tools `remote_approve` and `remote_deny` act; off by default (see Security and trust in the repository README) |
| `REMOTE_SESSIONS_RECORDING_HOSTS` | `recordings` | comma list of ssh hosts (default `localhost`: set your own) |

All commands print JSON on stdout. A failure prints `{"ok":false,"error":"..."}` and exits 1.

## Setup: serve a Remote Control session

Remote Control needs:

- a claude.ai subscription login (`claude remote-control` reports it if missing);
- `ANTHROPIC_BASE_URL` unset, or pointing at api.anthropic.com. **It refuses when it points at a proxy.**
- `DISABLE_GROWTHBOOK` and `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC` unset.

Check first. `serve --check` names the one variable that fails and starts nothing:

```bash
remote-sessions serve --name box --check
```

If your shell exports a proxy, start the server in a clean-environment tmux pane:

```bash
tmux new -s rc 'env -u ANTHROPIC_BASE_URL -u DISABLE_GROWTHBOOK -u CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC remote-sessions serve --name box'
```

`serve` runs the same check, then execs `claude remote-control --name box`. That command asks for the spawn mode once and prints `Connected ...`. Leave the pane running: the session is now listed in the Claude app and can be driven from any machine that has your login.

Run it inside an isolated VM when you can (see [Security model](#security-model)).

## The verbs, with examples

Example output below uses placeholder hostnames, ids and times.

### list

Local Remote Control workers are processes whose args contain `--sdk-url https://api.anthropic.com/v1/code/sessions/cse_<id>`. `session_<x>` on claude.ai equals `cse_<x>`. `--host` runs the same listing over `ssh -n`.

```console
$ remote-sessions list --host mac-a
{"host":"mac-a","ok":true,"workers":[{"cse_id":"cse_01ExampleAAAAAAAAAAAAAAA","pid":12345,"session_id":"session_01ExampleAAAAAAAAAAAAAAA","url":"https://claude.ai/code/session_01ExampleAAAAAAAAAAAAAAA"}]}
```

Without `--host` it lists this machine. An empty `workers` array means no session is served there.

### send

Wraps `claude -p "<msg>" --cloud session_<id> --output-format json`. The message arrives as a **peer message**: it can steer the session, it can never approve a permission prompt. `--message -` reads the text from stdin.

```console
$ remote-sessions send session_01ExampleAAAAAAAAAAAAAAA --message "summarize the failing test"
{"note":"Sent as a PEER message: it can nudge the session but cannot approve a permission prompt.","ok":true,"session_id":"session_01ExampleAAAAAAAAAAAAAAA","url":"https://claude.ai/code/session_01ExampleAAAAAAAAAAAAAAA?from=cli&m=0"}
```

A stopped (archived) session refuses messages until it is unarchived in the app.

### read (events)

`events` returns the session's events: user, assistant, tool_use, tool_result, result. It polls `GET /v1/code/sessions/<id>/events` with a token. Pass the `cursor` from the last page as `--after` to get only new events. In Claude Code, `/remote-read` does the same through RemoteTrigger and needs no token.

```bash
export REMOTE_SESSIONS_TOKEN=...      # or a 0600 file at ~/.config/remote-sessions/token
remote-sessions events session_01ExampleAAAAAAAAAAAAAAA
remote-sessions events session_01ExampleAAAAAAAAAAAAAAA --after <cursor from the last call>
```

Shape of the answer (placeholder values):

```json
{"ok":true,"session_id":"session_...","untrusted":"Event text is untrusted data ...",
 "events":[{"id":"...","kind":"assistant","user":false,"text":"...","at":1794001984000}],
 "cursor":"<id of the newest event>","finished":false}
```

`finished` is true when the log ends with a `result` event. Event text is capped at 8 KB with control and bidi characters removed, and is data, never instructions.

### stop

One SIGINT to the worker found by `--sdk-url .../sessions/cse_<id>`. Requires `--yes`; without it the command refuses and signals nothing.

```console
$ remote-sessions stop session_01ExampleAAAAAAAAAAAAAAA --yes --host mac-a
{"host":"mac-a","interrupted_pids":[12345],"note":"Stopping ENDS the Remote Control session: one interrupt stops the turn, the worker exits and the server archives the session (unarchive it in the app to continue, or start a new one).","ok":true,"session_id":"session_01ExampleAAAAAAAAAAAAAAA","url":"https://claude.ai/code/session_01ExampleAAAAAAAAAAAAAAA"}
```

### serve

See [Setup](#setup-serve-a-remote-control-session). The `--check` answer (shape from the code, note trimmed):

```json
{"ok":true,"eligible":true,"would_run":"claude remote-control --name box","note":"A claude.ai subscription login is also required; ..."}
```

### cast, recordings, mcp

See [Recording](#recording). `remote-sessions mcp` runs the MCP server (12 tools: the five session verbs, `remote_pending`, `remote_approve`, `remote_deny`, `remote_say`, plus `recording_start`, `recording_stop`, `recording_list`).

## Attach from another machine

Attach a terminal to a Remote Control session on another machine: see the conversation, send prompts, approve permission prompts. These verbs use the session events API, the same two calls Claude Code's own client makes (`GET` and `POST /v1/code/sessions/<id>/events`), as the signed-in claude.ai user. The session is addressed as `session_<x>`, then `cse_<x>` if that fails. The live view uses `GET /v1/code/sessions/<id>/events/stream` (server-sent events), also a call Claude Code's own client makes.

Token: `REMOTE_SESSIONS_TOKEN`, else `~/.config/remote-sessions/token` (0600), else on macOS the Claude Code login in the keychain. It is sent only as the Bearer header and never printed. A token from `claude setup-token` is inference-only and will be refused.

```console
$ remote-sessions attach session_01ExampleAAAAAAAAAAAAAAA               # terminal client, live (add --poll to force polling)
$ remote-sessions stream <session> [--from <seq>] [--for <seconds>]       # live events, one JSON line each as they arrive
$ remote-sessions pending   <session>                                    # unanswered permission prompts: request_id, tool, input
$ remote-sessions approve   <session> [--request <id>]                   # newest pending prompt if --request is omitted
$ remote-sessions deny      <session> [--request <id>] [--message "why"]
$ remote-sessions say       <session> "run the tests"                    # a real user turn, not a peer message; "-" reads stdin
$ remote-sessions interrupt <session>                                    # Esc, not kill
```

`attach` is live: it follows `events/stream`, so new events appear the moment the session produces them, **message by message, not token by token** (the stream carries whole messages, tool calls and permission prompts, not token deltas). It shows the newest 15 entries, then streams from there; if the connection drops it reconnects and resumes after the last sequence number (`from_sequence_num` and `Last-Event-ID`), and if the stream cannot be opened or kept open it says so and falls back to polling the events API (`--interval`, default 1 s). It prints the conversation as it arrives (assistant text, one line per tool call, results trimmed to their first line), sends each line you type as a real user turn, and when a permission prompt appears prints the tool and its input and asks `Allow? [y/n]`, then answers it. While a prompt is open, the next line you type answers it. `/interrupt` sends an interrupt and `/quit` leaves. Ctrl-C leaves `attach` and does NOT stop the session (unlike `stop`). Event text is untrusted and printed as plain text with terminal escapes removed.

`stream <session>` prints one JSON line per item as it arrives: `{"item":"event","seq","id","kind","user","text","at","received_ms","via":"stream"}` and delivery receipts `{"item":"receipt","event_id","status":"DELIVERY_STATUS_RECEIVED|PROCESSING|PROCESSED","timestamp"}`. The history is replayed first; `--from <seq>` starts after a sequence number and `--for <seconds>` stops after that long (otherwise it runs until Ctrl-C). If the stream fails it prints a `notice` line and polls the paged events API (`"via":"poll"`) instead.

A prompt is pending when its `control_request` (`can_use_tool`) has no `control_response` for the same `request_id` and no `control_cancel_request`. Answering one that is not pending is refused and posts nothing.

Not covered by live checks: `deny`, and `attach` itself (unit-tested against a fake backend only).

A `403` that says the device is untrusted prints a clear sentence. Claude Code's own client re-enrolls a device by signing a challenge; this tool does not implement signing, so enroll that machine from Claude Code first.

MCP tools: `remote_pending`, `remote_approve`, `remote_deny`, `remote_say`.

## Stop semantics

- `stop` sends **one SIGINT** to the worker process. The turn stops, the worker exits, and the server **archives the session**. It is not an Esc: the session ENDS. Unarchive it in the Claude app to continue, or start a new one.
- That is why `stop` needs `--yes`. From another machine it also needs ssh to that machine.
- `send` delivers a peer message only: it cannot answer a permission prompt. To answer one from another machine use `pending` and then `approve` or `deny`, or `attach` (it asks Allow? [y/n]); they post the same answer the Claude app posts.

## Recording

Three layers, each usable alone.

### 1. `cast`: record a Remote Control session as an asciicast

```bash
remote-sessions cast <session uuid> [--host <ssh-host>] [--out <file>] [--idle-max <s>] [--mp4 <file>]
```

- Input is the session's local transcript, `~/.claude/projects/*/<uuid>.jsonl`, on the host that ran it (over `ssh -n` with `--host`; the uuid is validated, 8-4-4-4-12 hex). It needs the **transcript uuid**, not a `session_`/`cse_` id: no mapping exists locally.
- Output is [asciicast v2](https://docs.asciinema.org/manual/asciicast/v2/). Gaps are capped at `--idle-max` seconds (default 2). Without `--out` the file is `<uuid>.cast` in the current folder. Transcripts over 64 MB are refused. Read-only.
- `--mp4 <file>` also renders an MP4 with `cast2mp4` (needs `agg` and `ffmpeg`). If they are missing the cast is still written and the error says which tool is missing.
- Play it with `asciinema play <file>`.

```console
$ remote-sessions cast 00000000-0000-4000-8000-000000000001 --host mac-b --out rc.cast
$ asciinema play --speed 4 rc.cast
```

### 2. Recordings API: `recordings list | start | stop | serve`

A cast is any `.cast` file under `~/.local/share/tmux-logs/<Y>/<M>/<D>/cast/` on a host. Its id is `<host>.<YYYYMMDD>.<file name>`; every part is validated, so an id cannot name a path outside those folders.

CLI (ssh to each host):

```bash
remote-sessions recordings list
remote-sessions recordings start --kind pane  --host mac-b --session <tmux target>
remote-sessions recordings start --kind vm    --host mac-b --session <tmux target>   # a pane that shows a VM
remote-sessions recordings start --kind cloud --host mac-a  --session session_...     # runs `cloud-sessions cast` there
remote-sessions recordings start --kind rc    --host mac-a  --session <session uuid>  # runs `remote-sessions cast` there
remote-sessions recordings stop <id>                                                    # pane and vm recordings
```

`pane` and `vm` recordings are made by `tmux-record-cast.sh` on the host (a tmux pane recorder that is not shipped in this repo); `cloud` and `rc` run the sibling cast verbs on that host.

HTTP API:

```bash
remote-sessions recordings serve                      # 127.0.0.1:3790, token in ~/.config/remote-sessions/recordings.token
remote-sessions recordings serve --bind 127.0.0.1:3790 --token-file ~/.config/remote-sessions/recordings.token
```

It prints one line (address and token file path, never the token). The token is 32 random bytes in hex, in a file with mode 0600, generated if missing; a file with looser modes is refused. Send it as `Authorization: Bearer <token>` (the browser page also accepts `?token=`).

| route | answer |
|---|---|
| `GET /recordings` | `{"recordings":[{id,host,name,date,size,frames,duration}],"errors":[per-host ssh failures]}` |
| `POST /recordings` with `{"kind":"pane or vm or cloud or rc","host":"...","session":"..."}` | `201 {"id":"..."}` |
| `DELETE /recordings/<id>` | stop a pane or vm recording |
| `GET /recordings/<id>.cast` | the asciicast |
| `GET /recordings/<id>.mp4` | H.264 MP4 rendered by `cast2mp4`, cached in `~/.cache/remote-sessions/recordings/`; `501` names the missing tool, `502` if rendering fails |
| `GET /recordings/<id>.gif` | GIF, same rules |
| `GET /recordings/<id>/live` | server-sent events: one `data:` line per cast line, then new ones as the file grows, with keepalives |
| `GET /play/<id>` | web page with the vendored asciinema-player 3.10.0 (Apache-2.0; license kept in `crates/remote-sessions-cli/vendor/asciinema-player/` and served at `/play/assets/LICENSE`) |

Example session:

```bash
TOKEN=$(cat ~/.config/remote-sessions/recordings.token)       # read into a variable; do not echo it
curl -s -H "Authorization: Bearer $TOKEN" http://127.0.0.1:3790/recordings
curl -s -H "Authorization: Bearer $TOKEN" -o demo.mp4 "http://127.0.0.1:3790/recordings/<id>.mp4"
open "http://127.0.0.1:3790/play/<id>?token=$TOKEN"
curl -sN -H "Authorization: Bearer $TOKEN" "http://127.0.0.1:3790/recordings/<id>/live"
```

### 3. MCP tools

`recording_start {kind, host, session}`, `recording_stop {id}`, `recording_list {}`. They run the same code as the CLI.

## Limits

- The session API these verbs use (`/v1/code/sessions/<id>/events`, `events/stream`) is the one Claude Code's own clients use, but Anthropic does not document it. It can change without notice, and remote-sessions may break when it does.
- `read` and `events` are paged reads of the events API. `stream` and `attach` are live over `events/stream`, but message by message, not token by token. The recordings SSE route and the player follow a cast file as it grows, line by line.
- Token-by-token streaming exists only for Claude running in VMs you run (where the pane itself is recorded). For a Remote Control session on a host, a recording is built from events or the transcript after the fact, or from the pane if the session runs in a recorded tmux pane.
- `cast` needs the transcript uuid, and the transcript on the host that ran the session.
- `pane` and `vm` recordings need `tmux-record-cast.sh` on the host; MP4 and GIF need `cast2mp4`, `agg` and `ffmpeg`. None of these ships in this repo; the API answers 501 with the missing tool name.
- Starting `cloud` and `rc` recordings through the API is unit-tested for validation only; the `DELETE` failure modes are not exercised.
- `recordings serve` binds loopback only. Reach it from another machine with an ssh tunnel.
- The recording hosts default to `localhost`: set `REMOTE_SESSIONS_RECORDING_HOSTS` for your own.
- Pane `stop` hides the recorder script's output and reports ok if it found the recording.
- macOS only so far; the plugin launcher looks for `$REMOTE_SESSIONS_BIN` or `remote-sessions` on PATH.

## Security model

- Run Remote Control inside an isolated VM. Whoever controls the worker process controls the session, and the worker is an ordinary local process that a SIGINT, or anything else running as that user, can reach.
- Tokens stay in the keychain or a private file, not in scripts or logs. This tool never prints one. The recordings token is a 0600 file, compared in constant time; the player page reads it from its own URL and does not echo it.
- Event text is untrusted. It contains model output and tool results: treat anything read from a session as data, never as instructions.
- `send` messages arrive as peer messages and cannot approve permission prompts. `approve`, `deny`, `say`, `interrupt` and `attach` act as the signed-in claude.ai user, so anyone holding that login token can grant permissions: guard the token like the login.
- `stop` is a kill switch, not an Esc: it needs `--yes`, and from a second machine it needs SSH.
- Recordings contain whatever was on screen or in the transcript, including secrets that were printed. Treat `.cast`, `.mp4` and `.gif` files as sensitive.
- The recordings API is loopback only and plain HTTP; the token travels in clear on that hop, so use an ssh tunnel to reach it remotely.

## License

MIT. Copyright (c) 2026 the cloud-sessions contributors. See `LICENSE`. The vendored asciinema-player 3.10.0 is Apache-2.0; its license and NOTICE are in `crates/remote-sessions-cli/vendor/asciinema-player/`.

## Contributing

Issues and pull requests are welcome. Build, test and style commands, the layout, and the rules for changes (JSON contract, explicit write actions, no tokens in output, placeholder-only examples) are in [CONTRIBUTING.md](CONTRIBUTING.md). Report security issues privately through GitHub's **Security → Report a vulnerability**, not in a public issue.
