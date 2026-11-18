# Publishing to the MCP registry (operator step, not done by CI)

The image and `server.json` are the registry metadata. The publish is a separate step, run by the operator after the tag.

1. Tag the release: `git tag remote-sessions-v0.3.4` (operator names the commit). The `ghcr image` workflow builds the image, runs the stranger test (stdin `initialize` + `tools/list`, hints checked), and pushes `ghcr.io/ramene/remote-sessions:0.3.4` and `:latest`.
2. Check the pushed image answers as a stranger: `docker run --rm -i ghcr.io/ramene/remote-sessions:0.3.4 < request.txt` where `request.txt` holds `initialize`, `notifications/initialized`, `tools/list` (one JSON per line).
3. Publish `server.json` to the MCP registry with the registry's own publisher tool, as documented at the MCP registry site (the tool name and its login step are not checked in this repo), signed in as the GitHub account that owns `io.github.ramene/remote-sessions`. The name in `server.json` must equal the one in the image label `io.modelcontextprotocol.server.name`.
4. Confirm the registry entry shows version 0.3.4, transport stdio, and the image identifier above.

Not done here: no tag, no push, no publish.
