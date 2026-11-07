---
description: Check Remote Control eligibility on this machine and give the command that serves it
argument-hint: "[name]"
disable-model-invocation: true
---
Check whether this machine can serve a Remote Control session. This only checks; it never starts the server (the server is interactive and runs in the user's own terminal).

```bash
RS="${CLAUDE_PLUGIN_ROOT}/scripts/remote-sessions"
"$RS" serve --check ${ARGUMENTS:+--name "$ARGUMENTS"}
```

- `ok: true`: eligible. Tell the user to run `remote-sessions serve --name <n>` (or `claude remote-control --name <n>`) in a terminal. It asks the spawn mode once, then prints `Connected ...`.
- `ok: false`: the error names the one environment variable that blocks Remote Control (`ANTHROPIC_BASE_URL` must be unset or api.anthropic.com; `DISABLE_GROWTHBOOK` and `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC` must be unset). Report it as is. A claude.ai subscription login is also required.

Run Remote Control inside an isolated VM: whoever controls the worker process controls the session.
