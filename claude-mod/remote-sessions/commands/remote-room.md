---
description: Open the control room beside Claude Code — Remote Control + every cloud session of your account and the Routines, drop into any session (live, two-way), approve/deny, visible close
argument-hint: "[--host <ssh-host>] [--tracked] [--no-routines]"
allowed-tools: Bash(*/scripts/open-room:*)
---
Run exactly this one command, nothing else first:

```bash
"${CLAUDE_PLUGIN_ROOT}/scripts/open-room" $ARGUMENTS
```

No flag is needed: the room shows every session of the account and the Routines by default (`--tracked`: only the sessions tracked on this machine; `--no-routines`: hide the Routines).

- `split: true`: the control room is open on the right (`mux` says whether cmux or tmux made the split). Tell the user in one line: ↑↓ select, Enter opens the chat with the session (live, two-way: type and press Enter to send, Esc goes back), `a`/`d` approve/deny, Tab switches panel, `r` refreshes, `R` shows or hides the Routines, q/Esc/✕ closes. If a panel shows a line with a command (for example `cloud-sessions auth use-claude-login`), that command is for the user to run at a terminal.
- `split: false`: show the user the `run` command to paste into any terminal or a new cmux / tmux pane.
