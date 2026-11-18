# Security policy

## Supported versions
The latest release (`0.3.x`) gets fixes. Older releases do not.

## Reporting a vulnerability
Please report privately through GitHub's private vulnerability reporting on this repository (Security tab, "Report a vulnerability"). Do not open a public issue for a vulnerability.

Include: the version, what you ran, what you expected, and what happened. We will acknowledge the report and agree a fix and a disclosure date with you.

## What this project handles
remote-sessions starts and reads Claude Code Remote Control sessions on your machine. It can approve or deny permission prompts only when the environment variable `REMOTE_SESSIONS_MCP_APPROVE=1` is set in the MCP server's own environment. Keep that variable unset unless you intend it. Never put a token, a session URL or a host credential in an issue or a report.
