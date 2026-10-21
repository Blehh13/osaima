#!/usr/bin/env bash
#
# build_vm_image.sh — fast Interstellar OS test image for QEMU/KVM.
#
# Unlike build_distro.sh (source build from the git ebuilds), this uses
# Gentoo's official binary packages and installs the OSAIMA components from
# THIS working tree, so a VM can test uncommitted work in under an hour.
#
#   sudo build-tools/scripts/build_vm_image.sh          # build
#   build-tools/scripts/run_vm.sh                       # boot it
#
# Output ($OUT, default out/vm): interstellar.qcow2, vmlinuz, initramfs.img,
# and id_osaima (SSH key for root@localhost:2222).
#
# Prerequisites on the host: cargo build --release in ai-core/mcp-daemon, a
# llama-server build and a .gguf model (ai-core/llm/start-llm.sh finds both).

set -euo pipefail

REPO_ROOT="$(realpath "$(dirname "$0")/../..")"
WORK="${WORK:-/var/tmp/osaima-vm}"
ROOTFS="$WORK/rootfs"
OUT="${OUT:-$REPO_ROOT/out/vm}"
IMG_SIZE="${IMG_SIZE:-24G}"
# Pick a fast mirror for your region (https://www.gentoo.org/downloads/mirrors/);
# the default CDN can be very slow from some networks.
MIRROR="${GENTOO_MIRROR:-https://distfiles.gentoo.org}"
STAGE3_INDEX="$MIRROR/releases/amd64/autobuilds/latest-stage3-amd64-desktop-systemd.txt"
PROFILE="interstellar:interstellar/systemd"
# The user who ran sudo owns the dev artifacts (daemon build, model, llama-server).
DEV_HOME="$(getent passwd "${SUDO_USER:-root}" | cut -d: -f6)"
# the CPU build (a Vulkan build may sit next to it for GPU tests)
LLAMA_DIR="${LLAMA_DIR:-$(dirname "$(find "$DEV_HOME/.local/share/osaima/llama" -name llama-server -type f -not -path '*vulkan*' | head -1)")}"
MODELS_DIR="${MODELS_DIR:-$DEV_HOME/.local/share/osaima/models}"
DAEMON_BIN="$REPO_ROOT/ai-core/mcp-daemon/target/release/mcp-daemon"
JAIL_BIN="$REPO_ROOT/ai-core/mcp-daemon/target/release/osaima-jail"
SHELL_BIN="$REPO_ROOT/ui/osaima-shell/src-tauri/target/release/osaima-shell"

log() { echo -e "\n==> $(date +%H:%M:%S) $*"; }
[ "$(id -u)" = 0 ] || { echo "Run as root (sudo)." >&2; exit 1; }
[ -x "$DAEMON_BIN" ] && [ -x "$JAIL_BIN" ] || { echo "Build the daemon first: cargo build --release" >&2; exit 1; }
[ -x "$SHELL_BIN" ] || { echo "Build the shell first: (cd ui/osaima-shell/src-tauri && cargo build --release)" >&2; exit 1; }
[ -x "$LLAMA_DIR/llama-server" ] || { echo "llama-server not found (set LLAMA_DIR)" >&2; exit 1; }
ls "$MODELS_DIR"/*.gguf >/dev/null || { echo "No .gguf in $MODELS_DIR" >&2; exit 1; }
mkdir -p "$WORK" "$ROOTFS" "$OUT"

# The host's /dev, /proc, /sys and /run are mounted into the chroot as SLAVES:
# unmounting a shared copy would also unmount the host's own (and take the
# user's desktop session down with /run/user). Mount in one step so there is
# no shared moment, never stack a second copy, and refuse to unmount anything
# that is not a slave.
mount_host() {
	for m in proc sys dev run; do
		mountpoint -q "$ROOTFS/$m" || mount --rbind --make-rslave "/$m" "$ROOTFS/$m"
	done
}
cleanup() {
	for m in run proc sys dev; do
		mountpoint -q "$ROOTFS/$m" || continue
		if findmnt -rn -o PROPAGATION "$ROOTFS/$m" | grep -q shared; then
			echo "NOT unmounting $ROOTFS/$m: it is shared with the host; reboot or unmount by hand" >&2
			continue
		fi
		umount -R -l "$ROOTFS/$m" 2>/dev/null || true
	done
}
trap cleanup EXIT

# ── 1. Gentoo stage3 (desktop + systemd) ─────────────────────────────────────
if [ ! -e "$ROOTFS/etc/gentoo-release" ]; then
	# the index is PGP clearsigned; take the tarball path, not the signature lines
	path="$(curl -fsSL "$STAGE3_INDEX" | grep -o '^[0-9TZ]*/stage3-[^ ]*\.tar\.xz' | head -1)"
	log "Fetching stage3: $path"
	curl -fSL --retry 3 -C - -o "$WORK/stage3.tar.xz" "$MIRROR/releases/amd64/autobuilds/$path"
	tar xpf "$WORK/stage3.tar.xz" -C "$ROOTFS" --xattrs-include='*.*' --numeric-owner
fi

# ── 2. Portage: our overlay + the official binhost ──────────────────────────
log "Configuring Portage"
rm -rf "$ROOTFS/var/db/repos/interstellar"
install -d "$ROOTFS/var/db/repos/interstellar" "$ROOTFS/etc/portage/repos.conf" \
	"$ROOTFS/etc/portage/package.use" "$ROOTFS/etc/dracut.conf.d"
cp -a "$REPO_ROOT/gentoo/overlay/." "$ROOTFS/var/db/repos/interstellar/"
cat > "$ROOTFS/etc/portage/repos.conf/interstellar.conf" <<'EOF'
[interstellar]
location = /var/db/repos/interstellar
masters = gentoo
auto-sync = no
priority = 100
EOF
cat > "$ROOTFS/etc/portage/make.conf" <<EOF
# Generic flags so binary packages from the Gentoo binhost match.
COMMON_FLAGS="-O2 -pipe"
CFLAGS="\${COMMON_FLAGS}"
CXXFLAGS="\${COMMON_FLAGS}"
MAKEOPTS="-j$(nproc)"
FEATURES="getbinpkg binpkg-request-signature"
EMERGE_DEFAULT_OPTS="--getbinpkg --jobs=4 --quiet-build"
ACCEPT_LICENSE="*"
LC_MESSAGES=C.utf8
GENTOO_MIRRORS="$MIRROR"
EOF
# Binary packages from the same mirror.
for f in "$ROOTFS"/etc/portage/binrepos.conf/*.conf; do
	[ -e "$f" ] && sed -i "s|^sync-uri = .*/releases/|sync-uri = $MIRROR/releases/|" "$f"
done
echo "sys-kernel/installkernel dracut" > "$ROOTFS/etc/portage/package.use/kernel"
# night light: gammastep's Wayland (wlr gamma) backend, without the GTK tray or location service
echo "x11-misc/gammastep wayland -gtk -geoclue" > "$ROOTFS/etc/portage/package.use/gammastep"
# Our own overlay is ~amd64; everything else stays on stable (binhost) packages.
install -d "$ROOTFS/etc/portage/package.accept_keywords"
echo "*/*::interstellar ~amd64" > "$ROOTFS/etc/portage/package.accept_keywords/interstellar"
# Keep Interstellar's os-release when baselayout is updated (see interstellar-release).
install -d "$ROOTFS/etc/portage/env"
echo 'INSTALL_MASK="/etc/os-release /usr/lib/os-release"' > "$ROOTFS/etc/portage/env/no-os-release.conf"
echo 'sys-apps/baselayout no-os-release.conf' > "$ROOTFS/etc/portage/package.env"
# Generic initramfs: the image must boot on any VM, not just the build host.
echo 'hostonly="no"' > "$ROOTFS/etc/dracut.conf.d/osaima.conf"
cp -L /etc/resolv.conf "$ROOTFS/etc/resolv.conf"

# ── 3. Packages (binary where the binhost has them) ──────────────────────────
log "Installing packages in chroot (binary packages; this is the long step)"
mount_host
# not -u: Gentoo's /etc/profile.d scripts read unset variables
chroot "$ROOTFS" /bin/bash -e <<CHROOT
	source /etc/profile
	[ -d /var/db/repos/gentoo/profiles ] || GENTOO_MIRRORS="$MIRROR" emerge-webrsync -q
	getuto
	eselect profile set "$PROFILE"
	emerge --noreplace --update \
		sys-kernel/gentoo-kernel-bin \
		sys-apps/bubblewrap 		dev-util/bpftool \
		net-misc/openssh \
		gui-wm/sway gui-apps/swaybg gui-apps/foot media-fonts/dejavu \
		gui-apps/waybar gui-apps/mako gui-apps/wl-clipboard gui-apps/grim x11-libs/libnotify \
		gui-apps/swaylock gui-apps/swayidle x11-misc/gammastep sys-apps/xdg-desktop-portal-gtk net-wireless/wireless-tools \
		net-libs/webkit-gtk:4.1 \
		xfce-base/thunar app-editors/mousepad media-gfx/imv \
		app-text/zathura app-text/zathura-pdf-poppler app-text/poppler x11-misc/xdg-utils sys-process/htop \
		app-office/libreoffice-bin media-video/mpv app-arch/xarchiver \
		www-client/firefox-bin \
		app-misc/interstellar-release
	# A previous interrupted run can leave the kernel merged but not deployed.
	ls /boot/vmlinuz-* >/dev/null 2>&1 || emerge --config sys-kernel/gentoo-kernel-bin
CHROOT

# ── 4. OSAIMA components from this working tree ──────────────────────────────
log "Installing OSAIMA from $REPO_ROOT"
install -Dm755 "$DAEMON_BIN" "$ROOTFS/usr/bin/osaima-mcp-daemon"
# next to the daemon: it applies Landlock/seccomp/rlimits to AI-run commands
install -Dm755 "$JAIL_BIN" "$ROOTFS/usr/bin/osaima-jail"
# the OSAIMA apps (assistant, settings, notes, ...), one window each
install -Dm755 "$SHELL_BIN" "$ROOTFS/usr/bin/osaima-shell"
# the desktop session: sway, top bar, notifications, file manager actions
rm -rf "$ROOTFS/usr/share/osaima/desktop"
install -d "$ROOTFS/usr/share/osaima/desktop"
cp -a "$REPO_ROOT/desktop/sway" "$REPO_ROOT/desktop/waybar" "$REPO_ROOT/desktop/mako" "$REPO_ROOT/desktop/thunar" \
	"$REPO_ROOT/desktop/wallpaper" "$ROOTFS/usr/share/osaima/desktop/"
# the OSAIMA look for every app: fonts, GTK theme, terminal colours
install -d "$ROOTFS/usr/share/fonts/osaima" "$ROOTFS/usr/share/themes" "$ROOTFS/etc/xdg/foot"
install -m644 "$REPO_ROOT"/desktop/fonts/* "$ROOTFS/usr/share/fonts/osaima/"
cp -a "$REPO_ROOT/desktop/share/themes/OSAIMA" "$ROOTFS/usr/share/themes/"
install -m644 "$REPO_ROOT/desktop/xdg/foot/foot.ini" "$ROOTFS/etc/xdg/foot/foot.ini"
chroot "$ROOTFS" fc-cache -f >/dev/null 2>&1 || true
install -m755 "$REPO_ROOT"/desktop/bin/osaima-* "$ROOTFS/usr/bin/"
install -Dm644 "$REPO_ROOT/desktop/thunar/uca.xml" "$ROOTFS/etc/xdg/Thunar/uca.xml"
install -d "$ROOTFS/usr/lib/osaima/llama" "$ROOTFS/usr/share/osaima/models" "$ROOTFS/usr/libexec/osaima"
cp -a "$LLAMA_DIR/." "$ROOTFS/usr/lib/osaima/llama/"
ln -sf /usr/lib/osaima/llama/llama-server "$ROOTFS/usr/bin/llama-server"
# Only the models the image should ship (the benchmark picked Phi-4-mini; the
# 1-2B ones were not accurate enough). Extra models also push the live ISO's
# squashfs past ISO-9660's 4 GiB per-file limit.
rm -f "$ROOTFS"/usr/share/osaima/models/*.gguf
for m in ${IMAGE_MODELS:-phi-4-mini*.gguf}; do cp "$MODELS_DIR"/$m "$ROOTFS/usr/share/osaima/models/"; done
install -m755 "$REPO_ROOT/ai-core/llm/start-llm.sh" "$ROOTFS/usr/libexec/osaima/start-llm.sh"
rm -rf "$ROOTFS/usr/share/osaima-shell"
install -d "$ROOTFS/usr/share/osaima-shell"
cp -a "$REPO_ROOT/ui/osaima-shell/frontend" "$REPO_ROOT/ui/osaima-shell/server.py" "$ROOTFS/usr/share/osaima-shell/"

# Intent router (NLU fast path): ONNX MiniLM + head, no PyTorch.
router_src="$REPO_ROOT/ai-core/intent-router"
[ -f "$router_src/intent_embed/encoder.onnx" ] || { echo "Export the router first: python export_onnx.py" >&2; exit 1; }
rm -rf "$ROOTFS/usr/lib/osaima/router"
install -d "$ROOTFS/usr/lib/osaima/router/intent_embed"
install -m644 "$router_src"/{router_service.py,slots.py,taxonomy.py,knowledge.py} "$ROOTFS/usr/lib/osaima/router/"
install -m644 "$router_src"/intent_embed/{encoder.onnx,tokenizer.json,head.npz} "$ROOTFS/usr/lib/osaima/router/intent_embed/"
mount_host
chroot "$ROOTFS" /bin/bash -e <<'CHROOT'
	[ -x /usr/lib/osaima/router-venv/bin/python ] || python3 -m venv /usr/lib/osaima/router-venv
	/usr/lib/osaima/router-venv/bin/pip install -q --disable-pip-version-check onnxruntime tokenizers numpy
CHROOT
cleanup

# Knowledge base for RAG: the OSAIMA docs, indexed on the build host with the
# same encoder (the user's own ~/Documents are added by re-running knowledge.py).
# Only project docs: never the build user's own ~/Documents.
know="$WORK/knowledge"
rm -rf "$know"
install -d -o "${SUDO_USER:-root}" "$know"
sudo -u "${SUDO_USER:-root}" env OSAIMA_KNOWLEDGE_DIR="$know" python3 "$router_src/knowledge.py" build \
	"$REPO_ROOT/ai-core/rag-engine/seed_corpus" "$REPO_ROOT/README.md" "$REPO_ROOT/ARCHITECTURE.md" \
	"$REPO_ROOT/BUILD-DISTRO.md" "$REPO_ROOT/ui/osaima-shell/README.md" >/dev/null
install -d "$ROOTFS/home/osaima/.local/share/osaima/knowledge"
cp "$know"/{vectors.npy,chunks.json} "$ROOTFS/home/osaima/.local/share/osaima/knowledge/"

# Voice (push to talk): faster-whisper listens, Piper speaks, both on the device.
# The service is ai-core/agent's `osaima-voice`, in its own Python environment, with
# the speech models downloaded now so the image needs no network to use them.
voice_models=/usr/share/osaima/voice
rm -rf "$ROOTFS/usr/lib/osaima/agent"
install -d "$ROOTFS/usr/lib/osaima/agent" "$ROOTFS/etc/osaima"
cp -a "$REPO_ROOT/ai-core/agent/pyproject.toml" "$REPO_ROOT/ai-core/agent/README.md" "$REPO_ROOT/ai-core/agent/src" "$ROOTFS/usr/lib/osaima/agent/"
printf '[voice]\nmodels_dir = "%s"\n' "$voice_models" > "$ROOTFS/etc/osaima/agent.toml"
mount_host
chroot "$ROOTFS" /bin/bash -e <<CHROOT
	[ -x /usr/lib/osaima/voice-venv/bin/python ] || python3 -m venv /usr/lib/osaima/voice-venv
	/usr/lib/osaima/voice-venv/bin/pip install -q --disable-pip-version-check --no-cache-dir "/usr/lib/osaima/agent[voice]"
	install -d $voice_models
	/usr/lib/osaima/voice-venv/bin/python -m piper.download_voices en_US-lessac-medium --data-dir $voice_models
	/usr/lib/osaima/voice-venv/bin/python -c "from faster_whisper import WhisperModel; WhisperModel('base.en', device='cpu', compute_type='int8', download_root='$voice_models')"
CHROOT
cleanup

units="$ROOTFS/usr/lib/systemd/user"
install -d "$units"
cat > "$units/osaima-llm.service" <<'EOF'
[Unit]
Description=OSAIMA on-device LLM (llama.cpp)
# The desktop user only: root's user manager (e.g. an SSH login) must not start a second copy.
ConditionUser=!root

[Service]
ExecStart=/usr/libexec/osaima/start-llm.sh
Restart=on-failure

[Install]
WantedBy=default.target
EOF
cat > "$units/osaima-router.service" <<'EOF'
[Unit]
Description=OSAIMA intent router (NLU fast path)
# The desktop user only: root's user manager (e.g. an SSH login) must not start a second copy.
ConditionUser=!root

[Service]
ExecStart=/usr/lib/osaima/router-venv/bin/python /usr/lib/osaima/router/router_service.py
Restart=on-failure

[Install]
WantedBy=default.target
EOF
sed -e 's|^\[Unit\]|[Unit]\nAfter=osaima-llm.service osaima-router.service|' \
	"$REPO_ROOT/gentoo/overlay/sys-apps/osaima-ai-core/files/osaima-mcp-daemon.service" > "$units/osaima-mcp-daemon.service"
cat > "$units/osaima-voice.service" <<'EOF'
[Unit]
Description=OSAIMA voice (push-to-talk speech, on this computer)
# The desktop user only: root's user manager (e.g. an SSH login) must not start a second copy.
ConditionUser=!root

[Service]
Environment=OSAIMA_AGENT_CONFIG=/etc/osaima/agent.toml
ExecStart=/usr/lib/osaima/voice-venv/bin/osaima-voice
Restart=on-failure

[Install]
WantedBy=default.target
EOF
cat > "$units/osaima-shell-web.service" <<'EOF'
[Unit]
Description=OSAIMA desktop shell (web backend)
# The desktop user only: root's user manager (e.g. an SSH login) must not start a second copy.
ConditionUser=!root
After=osaima-mcp-daemon.service

[Service]
Environment=PORT=8777
ExecStart=/usr/bin/python3 /usr/share/osaima-shell/server.py
Restart=on-failure

[Install]
WantedBy=default.target
EOF

# Scheduler daemon (optional): sets CPU priorities from what each program is doing.
# Installed when it has been built (cargo build --release in ai-core/schedd, and make in
# ai-core/schedd/bpf for the eBPF telemetry) and a trained model is committed.
schedd_dir="$REPO_ROOT/ai-core/schedd"
if [ -x "$schedd_dir/target/release/osaima-schedd" ] && [ -f "$schedd_dir/model/model.json" ]; then
	log "Adding the scheduler daemon"
	install -Dm755 "$schedd_dir/target/release/osaima-schedd" "$ROOTFS/usr/bin/osaima-schedd"
	install -Dm644 "$schedd_dir/model/model.json" "$ROOTFS/usr/share/osaima/schedd/model.json"
	if [ -f "$schedd_dir/bpf/telemetry.bpf.o" ]; then
		install -Dm644 "$schedd_dir/bpf/telemetry.bpf.o" "$ROOTFS/usr/lib/osaima/schedd/telemetry.bpf.o"
	fi
	install -Dm644 "$REPO_ROOT/gentoo/overlay/sys-apps/osaima-schedd/files/osaima-schedd.service" 		"$ROOTFS/usr/lib/systemd/system/osaima-schedd.service"
fi

# ── 5. System setup: user, autologin into the shell, SSH for tests ──────────
log "Configuring the system"
hash="$(openssl passwd -6 osaima)"  # test-image password for osaima and root
ssh-keygen -q -t ed25519 -N '' -f "$OUT/id_osaima" <<<y >/dev/null 2>&1 || true
chown "${SUDO_USER:-root}" "$OUT/id_osaima" "$OUT/id_osaima.pub"
chroot "$ROOTFS" /bin/bash -eu <<CHROOT
	echo interstellar > /etc/hostname
	echo '/dev/vda / ext4 defaults,noatime 0 1' > /etc/fstab
	id osaima >/dev/null 2>&1 || useradd -m -G wheel,video,input,audio -s /bin/bash osaima
	# Pre-hashed: Gentoo's passwdqc rejects the short test password otherwise.
	echo 'osaima:$hash' | chpasswd -e
	echo 'root:$hash' | chpasswd -e
	install -d -m700 /root/.ssh
	cat > /root/.ssh/authorized_keys <<KEY
$(cat "$OUT/id_osaima.pub")
KEY
	systemctl enable sshd systemd-networkd systemd-resolved >/dev/null 2>&1
	[ -f /usr/lib/systemd/system/osaima-schedd.service ] && systemctl enable osaima-schedd.service >/dev/null 2>&1 || true
	printf '[Match]\nName=en*\n[Network]\nDHCP=yes\n' > /etc/systemd/network/20-wired.network
	# the web backend is only for browser-based development; the desktop apps talk to the daemon directly
	systemctl --global enable osaima-llm.service osaima-router.service osaima-mcp-daemon.service osaima-voice.service
	systemctl --global disable osaima-shell-web.service >/dev/null 2>&1 || true
	# Autologin on tty1 straight into the OSAIMA desktop.
	install -d /etc/systemd/system/getty@tty1.service.d
	printf '[Service]\nExecStart=\nExecStart=-/sbin/agetty --autologin osaima --noclear %%I \$TERM\n' \
		> /etc/systemd/system/getty@tty1.service.d/autologin.conf
	# What loginctl enable-linger does; logind is not running inside the chroot.
	install -d /var/lib/systemd/linger && touch /var/lib/systemd/linger/osaima
CHROOT
# Firefox is only the shell's kiosk: no first-run/terms dialog over the shell,
# and no telemetry, studies or update checks (nothing should leave the machine).
install -Dm644 "$REPO_ROOT/build-tools/files/firefox-policies.json" "$ROOTFS/opt/firefox/distribution/policies.json"
# Desktop portal with the GTK backend: waybar asks it for the dark/light
# setting and exits if it is masked; without a backend Firefox waits on it.
rm -f "$ROOTFS"/etc/systemd/user/xdg-desktop-portal*.service
install -d "$ROOTFS/etc/xdg/xdg-desktop-portal"
printf '[preferred]\ndefault=gtk\n' > "$ROOTFS/etc/xdg/xdg-desktop-portal/sway-portals.conf"
# The OSAIMA desktop session. Image-only settings go in a sway include so the
# shared config (desktop/sway/config) stays the same for development.
rm -f "$ROOTFS/home/osaima/.config/sway/config"
install -d "$ROOTFS/etc/sway/config.d"
cat > "$ROOTFS/etc/sway/config.d/50-osaima-image.conf" <<'EOF'
# User services (the AI daemon) need the session's display to open windows.
exec systemctl --user import-environment WAYLAND_DISPLAY SWAYSOCK XDG_CURRENT_DESKTOP
exec dbus-update-activation-environment --systemd WAYLAND_DISPLAY SWAYSOCK XDG_CURRENT_DESKTOP
# In a VM the virtual GPU reports a tiny preferred mode (640x480); real
# hardware keeps its native resolution.
# (one quoted argument: swaymsg would otherwise read --custom as its own option)
exec sh -c 'systemd-detect-virt -q && swaymsg "output * mode --custom 1600x900@60Hz"'
EOF
# Dark GTK apps (Thunar, Mousepad) to match OSAIMA.
install -d "$ROOTFS/etc/gtk-3.0"
printf '[Settings]\ngtk-theme-name=OSAIMA\ngtk-application-prefer-dark-theme=1\ngtk-font-name=Atkinson Hyperlegible Next 10\n' > "$ROOTFS/etc/gtk-3.0/settings.ini"
cat > "$ROOTFS/home/osaima/.bash_profile" <<'EOF'
[ -f ~/.bashrc ] && . ~/.bashrc
if [ "$(tty)" = /dev/tty1 ] && [ -z "$WAYLAND_DISPLAY" ]; then
	export OSAIMA_DESKTOP=/usr/share/osaima/desktop XDG_CURRENT_DESKTOP=sway MOZ_ENABLE_WAYLAND=1 GTK_THEME=OSAIMA
	if systemd-detect-virt -q; then
		# Software rendering works on any VM GPU; WebKit's GPU paths do not.
		export WLR_RENDERER=pixman WLR_NO_HARDWARE_CURSORS=1
		export WEBKIT_DISABLE_DMABUF_RENDERER=1 WEBKIT_DISABLE_COMPOSITING_MODE=1
	fi
	cd ~
	exec sway -c "$OSAIMA_DESKTOP/sway/config"
fi
EOF
chroot "$ROOTFS" chown -R osaima:osaima /home/osaima

# ── 6. Disk image + kernel for direct boot ──────────────────────────────────
cleanup
if [ -n "${SKIP_DISK:-}" ]; then
	log "Rootfs updated; skipping the disk image (SKIP_DISK set)"
	chown -R "${SUDO_USER:-root}" "$OUT"
	exit 0
fi
log "Building disk image"
kver="$(ls "$ROOTFS/lib/modules" | sort -V | tail -1)"
# installkernel names it kernel-<ver> (systemd layout) or vmlinuz-<ver>
cp "$(ls "$ROOTFS"/boot/{kernel,vmlinuz}-"$kver" 2>/dev/null | head -1)" "$OUT/vmlinuz"
cp "$ROOTFS/boot/initramfs-$kver.img" "$OUT/initramfs.img"
rm -f "$WORK/root.img"
truncate -s "$IMG_SIZE" "$WORK/root.img"
mkfs.ext4 -q -F -L interstellar -d "$ROOTFS" "$WORK/root.img"
qemu-img convert -O qcow2 -c "$WORK/root.img" "$OUT/interstellar.qcow2"
rm -f "$WORK/root.img"
chown -R "${SUDO_USER:-root}" "$OUT"

log "Done (kernel $kver)"
ls -lh "$OUT"
