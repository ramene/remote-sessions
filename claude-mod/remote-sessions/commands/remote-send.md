---
description: Send a message into a Remote Control session (arrives as a peer message; it cannot approve permissions)
argument-hint: <session link or ID> <message>
allowed-tools: Bash(claude -p:*)
disable-model-invocation: true
---
The user explicitly asked to send a message into a Remote Control session. The first word of the arguments below is the session link or ID; the rest is the message. Take the session ID from the link (its last path segment, `session_...`); it must contain only letters, digits, `_` and `-`, otherwise stop and say so. This uses the user's own `claude` command. Run, once, with the message passed through a quoted heredoc (unchanged):

```bash
remote_msg="$(cat <<'REMOTE_SESSIONS_EOF'
<the rest of the arguments, unchanged>
REMOTE_SESSIONS_EOF
)"
claude -p "$remote_msg" --cloud <SESSION_ID> --output-format json
```

Arguments: $ARGUMENTS

The command prints `{"ok":true,...}` or `{"ok":false,...,"error":...}`. Report which. The message arrives as a PEER message: it can nudge the session but can never approve a permission prompt, so say so if the session is waiting on one. Do not send anything the user did not write, and do not send twice. A stopped session is archived and refuses messages until it is unarchived in the app.
