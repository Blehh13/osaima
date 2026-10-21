#!/bin/bash
echo "sun" | sudo -S apt-get update
echo "sun" | sudo -S apt-get install -y curl wget build-essential libwebkit2gtk-4.1-dev libssl-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev nodejs npm docker.io
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
source "$HOME/.cargo/env"
