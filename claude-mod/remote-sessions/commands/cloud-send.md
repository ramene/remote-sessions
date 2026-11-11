---
description: Send a follow-up message into a cloud session (wraps cloud-sessions send)
argument-hint: <session link or ID> <message>
allowed-tools: Bash(cloud-sessions send:*)
disable-model-invocation: true
---
The user explicitly asked to send a follow-up message into a cloud session. The first word of the arguments below is the session link or ID; the rest is the message. Take the session ID from the link (its last path segment, `session_...`); it must contain only letters, digits, `_` and `-`, otherwise stop and say so. This needs the separate `cloud-sessions` tool: if `cloud-sessions` is not on PATH, say "Install cloud-sessions to send into cloud sessions" and stop. Run, once, with the message passed on stdin through a quoted heredoc (unchanged):

```bash
cloud-sessions send <SESSION_ID> --message - <<'CLOUD_SEND_EOF'
<the rest of the arguments, unchanged>
CLOUD_SEND_EOF
```

Arguments: $ARGUMENTS

Report `ok` or the `error`. Do not send anything the user did not write, and do not send twice. To follow the reply, the user runs `cloud-sessions watch <SESSION_ID>`. Cloud sessions raise no permission prompts, so there is nothing to approve; this is the whole drop-in for a cloud session.
