# Changelog

## 0.2.3 (unreleased)

- Linux: use the Claude Code login from ~/.claude/.credentials.json. Token order for `events`, `read`, `pending`, `approve`, `deny`, `say`, `interrupt`, `mode`, `stream` and `attach`: `REMOTE_SESSIONS_TOKEN`, then `~/.config/remote-sessions/token`, then the macOS keychain login (macOS only), then `claudeAiOauth.accessToken` from `$CLAUDE_CONFIG_DIR/.credentials.json` (when `CLAUDE_CONFIG_DIR` is set) or `~/.claude/.credentials.json` (all platforms). Before, Linux found no token although `claude auth status` said `loggedIn: true` (s461). The file is only read: an expired login (`expiresAt` in the past) is an error ("the Claude Code login has expired: run claude once to refresh it"), not a stale request. Bad JSON or a missing `accessToken` key gives a clear error. No error text carries a token.
- Tests: `tests/token.rs` (valid, expired, missing key, bad JSON, missing file, file mode 0644 accepted, `CLAUDE_CONFIG_DIR`, and the precedence order).
- MCP: every tool now declares a `title` and all four tool annotation hints as booleans (`readOnlyHint`, `destructiveHint`, `idempotentHint`, `openWorldHint`; MCP spec 2025-06-18). Changed values: `remote_approve` and `remote_deny` are destructive (an approved tool can change or delete files; a denied tool call cannot be redone); `remote_list` and `remote_stop` are open-world (with `host` they run over ssh).
- Tests: `tests/mcp_tools.rs` has one test per MCP tool, by name: its hints and its argument checks (no network, no ssh, no signal).

## 0.2.2

- 0.2.2: privacy: examples and defaults no longer name the authors machines or sessions; default recording host is localhost

## 0.2.1

- New: `stream <session> [--from <seq>] [--for <seconds>]` follows a session live over `GET /v1/code/sessions/<id>/events/stream` (server-sent events, the call Claude Code's own client makes) and prints one JSON line per event as it arrives, plus delivery receipts (`DELIVERY_STATUS_RECEIVED|PROCESSING|PROCESSED`). Message by message, not token by token. It reconnects on a dropped connection with `from_sequence_num` and `Last-Event-ID`, skips what it already delivered, and falls back to polling the paged events API (with a notice) when the stream fails.
- Changed: `attach` is live by default (events/stream); it keeps its input loop and `Allow? [y/n]` answers, and says so when it falls back to polling. `--poll` forces polling.
- New: `read <session>` (the conversation as text, every page) and `mode <session> <default|acceptEdits|auto|plan|dontAsk|bypassPermissions>` (the `set_permission_mode` request the Claude app sends), already on main since 0.2.0.
- Core: new `remote_sessions_core::stream` module (`Client::stream`, `StreamItem`, `StreamEnd`) and `Client::open_stream`.

## 0.2.0

Since 0.1.0: attach, pending, approve, deny, say and interrupt (a terminal client for Remote Control sessions from another machine), read (the conversation as text), cast and recordings (asciicast v2, web player, MP4 and a recordings HTTP API), an end card for MP4s, and prebuilt archives for macOS (universal), Linux x86_64 and Linux arm64.

- New: attach from another machine. `attach <session> [--interval 1]` is a terminal client (polled): prints the conversation, sends typed lines as real user turns, asks `Allow? [y/n]` for permission prompts; Ctrl-C leaves without stopping the session. Also `pending`, `approve [--request <id>]`, `deny [--request <id>] [--message m]`, `say <session> <text|->` and `interrupt`, over the session events API. Falls back from `session_<x>` to `cse_<x>`; an untrusted-device 403 prints a clear message (no signing). The token may now also come from the macOS Claude Code keychain login.
- New MCP tools: remote_pending, remote_approve, remote_deny, remote_say.

- New: cast <session uuid> [--host <ssh-host>] [--out <file>] [--idle-max <s>] [--mp4 <file>] records a session from its local transcript (the Claude projects folder, uuid.jsonl, on the host that ran it) as an asciicast v2 file for asciinema play. Read-only. The event decoder and cast renderer are copied from cloud-sessions (same authors).
- New: recordings list | start | stop (ssh to each host; pane and vm recordings use tmux-record-cast.sh, cloud and rc make a cast with the sibling cast verbs) and recordings serve [--bind 127.0.0.1:3790] [--token-file <path>]: an HTTP API (Bearer token in a 0600 file, generated if missing, never printed; loopback bind only) with GET/POST /recordings, DELETE /recordings/<id>, GET /recordings/<id>.cast, .mp4, .gif (cast2mp4; 501 when agg or ffmpeg are missing), GET /recordings/<id>/live (server-sent events) and GET /play/<id> (vendored asciinema-player 3.10.0, Apache-2.0, license served and kept in vendor/asciinema-player/).
- New MCP tools: recording_start, recording_stop, recording_list.
- cast --mp4 <file> renders the cast with cast2mp4.

## 0.1.0

- New mod, sibling of cloud-sessions (same authors): `remote-sessions` for Remote Control sessions on your own machines.
- CLI `serve|list|send|events|stop|mcp`; Claude Code plugin with `/remote-serve`, `/remote-list`, `/remote-send`, `/remote-read`, `/remote-stop`, and an MCP server.
- `stop` requires `--yes` (it ends the session and the server archives it).
