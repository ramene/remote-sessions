# sessions-tui

Terminal UIs for coding-agent sessions, used by [remote-sessions](https://github.com/ramene/remote-sessions):

- the session view: transcript, prompt box, permission modal (`remote-sessions attach <session> --tui`);
- the control room: every session in two panels, with keyboard and mouse (`remote-sessions control`).

The crate does not know where a session runs. The host program implements `model::Backend` (session view) or `room::RoomBackend` (control room). It uses threads and `std::sync::mpsc`, with no async runtime. Messages arrive whole, not token by token. All text from a session is untrusted: the crate cleans it and draws it as plain text.

`cargo run -p sessions-tui --example mock_frames` prints the README frames of the control room, drawn by this code with placeholder data, as JSON.

See the [remote-sessions README](https://github.com/ramene/remote-sessions#control-room) and the [getting-started guide](https://github.com/ramene/remote-sessions/blob/main/docs/getting-started.md).

## License

MIT. See `LICENSE`.
