# Runnable entry for MCP registries and scanners: builds the remote-sessions CLI and runs `remote-sessions mcp` on stdio.
# No login, no token and no network are needed to start it or to list its tools; auth is checked only when a tool is called.
FROM rust:1-slim AS build
WORKDIR /src
COPY . .
RUN cargo build --release --locked -p remote-sessions-cli

FROM debian:stable-slim
COPY --from=build /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt
COPY --from=build /src/target/release/remote-sessions /usr/local/bin/remote-sessions
LABEL io.modelcontextprotocol.server.name="io.github.ramene/remote-sessions"
LABEL org.opencontainers.image.source="https://github.com/ramene/remote-sessions"
ENTRYPOINT ["remote-sessions", "mcp"]
