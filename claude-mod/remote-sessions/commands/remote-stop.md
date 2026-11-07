---
description: Stop a Remote Control session (ENDS it; the server archives it)
argument-hint: "<session link or ID> [--host <ssh-host>]"
disable-model-invocation: true
---
The user asked to stop the Remote Control session in `$ARGUMENTS`. First tell the user, in plain words: **stopping ENDS the session**. One interrupt stops the turn, the worker exits and the server archives the session; a later send is refused until it is unarchived in the app. Only after the user has confirmed, run:

```bash
"${CLAUDE_PLUGIN_ROOT}/scripts/remote-sessions" stop $ARGUMENTS --yes
```

- `ok: true` + `interrupted_pids`: the session was stopped. Confirm with /remote-read: the events end with `[Request interrupted by user ...]` and result `error_during_execution`.
- `ok: false`: no worker for that session on that machine. Use `--host <ssh-host>` for another machine (needs ssh). A session running in Anthropic's cloud cannot be stopped this way (that is cloud-sessions: use the Stop button on its page).
