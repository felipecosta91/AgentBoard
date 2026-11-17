#!/bin/bash
# Builds AgentBoard and installs ~/Applications/AgentBoard.app (Spotlight / Dock).
set -euo pipefail
cd "$(dirname "$0")/.."

(cd web && pnpm install --frozen-lockfile && pnpm build)
cargo build --release

pkill -f "AgentBoard.app/Contents/MacOS/AgentBoard" 2>/dev/null || true
mkdir -p "$HOME/Applications"
scripts/make-app.sh target/release/agentboard "$HOME/Applications" >/dev/null
touch "$HOME/Applications/AgentBoard.app"
echo "Instalado em $HOME/Applications/AgentBoard.app"
