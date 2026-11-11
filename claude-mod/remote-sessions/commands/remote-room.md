---
description: Open the control room beside Claude Code — Remote Control + cloud sessions, drop into any session (live, two-way), approve/deny, visible close
argument-hint: "[--host <ssh-host>]"
allowed-tools: Bash(*/scripts/open-room:*)
---
Run exactly this one command, nothing else first:

```bash
"${CLAUDE_PLUGIN_ROOT}/scripts/open-room" $ARGUMENTS
```

- `split: true`: the control room is open on the right (`mux` says whether cmux or tmux made the split). Tell the user in one line: ↑↓ select, Enter drops into a session (two-way: type and press Enter to send, Esc goes back), `a`/`d` approve/deny, Tab switches panel, `r` refreshes, q/Esc/✕ closes.
- `split: false`: show the user the `run` command to paste into any terminal or a new cmux / tmux pane.
