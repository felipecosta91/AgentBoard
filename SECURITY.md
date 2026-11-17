# Security

## Reporting a vulnerability

Please **do not open a public issue**. Use GitHub's private vulnerability reporting
(Security tab → *Report a vulnerability*) with steps to reproduce. You will get a reply as soon as possible.

## Threat model (read before exposing the app)

- AgentBoard runs AI agents (Claude / Codex CLIs) that can **edit files and run shell commands** inside your project folders.
- The server is protected by a single random token (128 bits, generated on first run, stored only in `~/.agentboard/`).
  **Anyone holding the token (or the pairing QR/code) can control your agents.** Treat it like an SSH key.
- With remote access on, the server is reachable from the internet through a tunnel (ngrok / Cloudflare). The token is the only gate.
  Turn the tunnel off with `AGENTBOARD_TUNNEL=off` if you do not use the phone app.
- Push notifications use the public `ntfy.sh` server by default (random 64-bit topic). Card titles and short messages pass through it.
  Self-host ntfy and set `NTFY_SERVER` to avoid that. The tunnel URL announcement is HMAC-signed with your token, so a third party
  who learns the topic cannot redirect the phone app.

## What the project never contains

No credentials, tokens or keys are committed. Per-install secrets (access token, ntfy topic) are generated at first run.
The ngrok authtoken is stored by ngrok itself, never by AgentBoard. Release signing keys for the APK are optional CI secrets.
