#!/usr/bin/env bash
#
# build_distro.sh — assemble an Interstellar OS root filesystem from a Gentoo
# stage3 plus our overlay, profile and meta package.
#
# This is the "fork" build (Stages 3-8 of the roadmap): it does NOT rebuild the
# world from scratch — it layers Interstellar's overlay on top of upstream Gentoo,
# selects our profile, and emerges the meta package that pulls in the shell + AI
# core. Run on a Linux host with root (ideally in a VM or CI), not on Windows.
#
# Output: a rootfs tree at $ROOTFS you can pack into an ISO with create_iso.sh.

set -euo pipefail

REPO_ROOT="$(realpath "$(dirname "$0")/../..")"
WORK="${WORK:-/var/tmp/interstellar-build}"
ROOTFS="${ROOTFS:-$WORK/rootfs}"
STAGE3_URL="${STAGE3_URL:-}"   # e.g. a Gentoo amd64 openrc stage3 tarball URL
PROFILE="${PROFILE:-interstellar:interstellar/agentic}"

need_root() { [ "$(id -u)" = 0 ] || { echo "Run as root." >&2; exit 1; }; }
log() { echo -e "\n==> $*"; }

need_root
mkdir -p "$WORK" "$ROOTFS"

# ── Stage 1: unpack a Gentoo stage3 as the base ──────────────────────────────
if [ ! -e "$ROOTFS/etc/gentoo-release" ]; then
	[ -n "$STAGE3_URL" ] || { echo "Set STAGE3_URL to a Gentoo stage3 tarball." >&2; exit 1; }
	log "Fetching + unpacking stage3"
	curl -fSL "$STAGE3_URL" -o "$WORK/stage3.tar.xz"
	tar xpf "$WORK/stage3.tar.xz" -C "$ROOTFS" --xattrs-include='*.*' --numeric-owner
fi

# ── Stage 2: install our overlay + Portage config into the rootfs ────────────
log "Installing Interstellar overlay + config"
install -d "$ROOTFS/var/db/repos/interstellar"
cp -a "$REPO_ROOT/gentoo/overlay/." "$ROOTFS/var/db/repos/interstellar/"

install -d "$ROOTFS/etc/portage/repos.conf"
# Local build: point the overlay at the copy we just placed.
cat > "$ROOTFS/etc/portage/repos.conf/interstellar.conf" <<'EOF'
[interstellar]
location = /var/db/repos/interstellar
masters = gentoo
auto-sync = no
priority = 100
EOF

cp "$REPO_ROOT/gentoo/config/make.conf.example" "$ROOTFS/etc/portage/make.conf"

# Copy DNS so the chroot can fetch distfiles.
cp -L /etc/resolv.conf "$ROOTFS/etc/resolv.conf"

# ── Stage 3: chroot and build the distro ─────────────────────────────────────
log "Entering chroot to sync, set profile and emerge the meta package"
for m in proc sys dev; do mount --rbind "/$m" "$ROOTFS/$m"; done

chroot "$ROOTFS" /bin/bash -eux <<CHROOT
	source /etc/profile
	emerge-webrsync
	eselect profile set "$PROFILE"
	# Pull the whole agentic desktop (shell + AI core + desktop + devtools).
	emerge --autounmask=y --autounmask-write=y app-misc/interstellar-meta || true
	etc-update --automode -5 || true
	emerge app-misc/interstellar-meta
	# Enable the AI core service for the default session.
	systemctl --global enable osaima-mcp-daemon.service 2>/dev/null || true
CHROOT

for m in dev sys proc; do umount -R "$ROOTFS/$m" 2>/dev/null || true; done

log "Rootfs ready at: $ROOTFS"
log "Next: pack it with build-tools/scripts/create_iso.sh"
