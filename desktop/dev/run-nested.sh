#!/bin/bash
# Run the OSAIMA desktop in a window on your current desktop, for testing
# without building an ISO. Starts the router, daemon and shell server if they
# are not running, then sway nested inside your session.
#
#   desktop/dev/run-nested.sh          # windows load pages live from :8777 (edit, then reload)
#   desktop/dev/run-nested.sh --built  # use the pages built into the Tauri binary
#
# In the nested window the modifier key is Alt (your desktop keeps Super).
set -eu
REPO="$(cd "$(dirname "$0")/../.." && pwd)"
LOGS="${XDG_RUNTIME_DIR:-/tmp}/osaima-dev"
mkdir -p "$LOGS"

start() { # name, check command, start command...
	local name=$1 check=$2; shift 2
	if ! sh -c "$check" >/dev/null 2>&1; then
		echo "starting $name"
		setsid "$@" >"$LOGS/$name.log" 2>&1 </dev/null &
	fi
}
RUN="${XDG_RUNTIME_DIR:-/tmp}/osaima"
start router "test -S $RUN/router.sock" python3 "$REPO/ai-core/intent-router/router_service.py"
sleep 1
start daemon "test -S $RUN/mcp.sock" "$REPO/ai-core/mcp-daemon/target/release/mcp-daemon"
start server "curl -sf http://127.0.0.1:8777/ -o /dev/null" env PORT=8777 python3 "$REPO/ui/osaima-shell/server.py"

export OSAIMA_DESKTOP="$REPO/desktop"
export OSAIMA_SHELL="$REPO/ui/osaima-shell/src-tauri/target/release/osaima-shell"
export PATH="$REPO/desktop/bin:$PATH"
[ "${1:-}" = "--built" ] || export OSAIMA_DEV_URL=http://127.0.0.1:8777
[ -x "$OSAIMA_SHELL" ] || { echo "build the shell first: (cd ui/osaima-shell/src-tauri && cargo build --release)" >&2; exit 1; }

# The OSAIMA look, for apps in this session only (your own desktop is untouched):
# GTK theme, terminal colours, the system fonts.
export XDG_DATA_DIRS="$REPO/desktop/share:${XDG_DATA_DIRS:-/usr/local/share:/usr/share}"
export XDG_CONFIG_DIRS="$REPO/desktop/xdg:${XDG_CONFIG_DIRS:-/etc/xdg}"
export GTK_THEME=OSAIMA
cat > "$LOGS/fonts.conf" <<FONTS
<?xml version="1.0"?><!DOCTYPE fontconfig SYSTEM "fonts.dtd">
<fontconfig><include ignore_missing="yes">/etc/fonts/fonts.conf</include><dir>$REPO/desktop/fonts</dir></fontconfig>
FONTS
export FONTCONFIG_FILE="$LOGS/fonts.conf"

# Same config with Alt as the modifier and a fixed-size output window. No idle
# lock here: logind lock requests would come from your own desktop's session.
CONF="$LOGS/sway.config"
sed -e 's/^set \$mod Mod4/set $mod Mod1/' -e '/^exec swayidle/d' -e '/^exec osaima-idle/d' "$REPO/desktop/sway/config" > "$CONF"
echo 'output WL-1 resolution 1600x900' >> "$CONF"
echo "sway log: $LOGS/sway.log"
cd "$HOME"   # apps start in the home folder, as in a real session
WLR_BACKENDS=wayland exec sway -c "$CONF" >"$LOGS/sway.log" 2>&1
