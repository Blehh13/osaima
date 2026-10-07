#!/usr/bin/env bash
#
# ISO feasibility spike, experiment 1: how much of the OS can come from Gentoo's
# official binary package host instead of being compiled?
#
# Runs INSIDE a Gentoo stage3 container (see .github/workflows/iso-spike.yml).
# Nothing is installed: `emerge --pretend` only resolves dependencies, so this
# takes minutes, not hours. For each configuration it counts the packages that
# would be installed from a binary package and those that would be compiled.
#
# Writes the raw emerge output and summary.md to $OUT.

set -uo pipefail

REPO=${REPO:-/work}
OUT=${OUT:-$REPO/spike-out}
mkdir -p "$OUT"

# What a bootable Interstellar image needs: our meta package (shell, AI core,
# agent, Ollama, sway, terminal) plus the boot and live-ISO tooling.
PACKAGES=(
	app-misc/interstellar-meta
	net-misc/networkmanager
	sys-kernel/gentoo-kernel-bin
	sys-kernel/dracut
	sys-boot/grub
	sys-fs/squashfs-tools
	dev-libs/libisoburn
	sys-fs/mtools
)

log() { printf '\n==> %s\n' "$*"; }

log "Container: $(. /etc/os-release && echo "$PRETTY_NAME"), $(nproc) CPUs, $(free -g | awk '/Mem:/ {print $2}') GB RAM"

log "Syncing the Portage tree (emerge-webrsync)"
start=$(date +%s)
emerge-webrsync -q > "$OUT/webrsync.txt" 2>&1 || { tail -20 "$OUT/webrsync.txt"; exit 1; }
echo "took $(( $(date +%s) - start )) s"

log "Binary package host configuration"
getuto > "$OUT/getuto.txt" 2>&1 || true
cat /etc/portage/binrepos.conf/* 2>/dev/null | tee "$OUT/binrepos.txt"
portageq envvar FEATURES | tr ' ' '\n' | grep -E 'getbinpkg|binpkg' || true
ORIGINAL_PROFILE=$(eselect profile show | tail -1 | xargs)
echo "stage3 profile: $ORIGINAL_PROFILE"

log "Registering the Interstellar overlay"
mkdir -p /var/db/repos /etc/portage/repos.conf /etc/portage/package.accept_keywords
cp -a "$REPO/gentoo/overlay" /var/db/repos/interstellar
cp "$REPO/gentoo/config/repos.conf/interstellar.conf" /etc/portage/repos.conf/
cat > /etc/portage/package.accept_keywords/osaima <<'EOF'
app-misc/interstellar-meta ~amd64
app-misc/interstellar-release ~amd64
gui-apps/osaima-shell **
sys-apps/osaima-ai-core **
sys-apps/osaima-agent **
sci-ml/ollama ~amd64
EOF
eselect profile list | grep -i interstellar || echo "(the overlay's profiles are not listed)"

# name | profile | ACCEPT_KEYWORDS
run() {
	local name=$1 profile=$2 keywords=$3
	log "Experiment $name: profile=$profile keywords=$keywords"
	eselect profile set "$profile" || { echo "could not select $profile" > "$OUT/$name.txt"; return; }
	ACCEPT_KEYWORDS="$keywords" emerge --pretend --verbose --getbinpkg --usepkg \
		--with-bdeps=n --color=n --quiet-build=y "${PACKAGES[@]}" > "$OUT/$name.txt" 2>&1
	echo $? > "$OUT/$name.rc"
	echo "emerge exit code: $(cat "$OUT/$name.rc")"
	grep -c '^\[binary' "$OUT/$name.txt" | sed 's/^/binary packages: /'
	grep -c '^\[ebuild' "$OUT/$name.txt" | sed 's/^/to compile:     /'
}

run A-stock "$ORIGINAL_PROFILE" amd64
run B-ours-stable interstellar:interstellar/agentic amd64
run C-ours-testing interstellar:interstellar/agentic '~amd64'

log "Writing the summary"
{
	echo "# Binary host check"
	echo
	echo "Packages asked for: ${PACKAGES[*]}"
	echo
	echo "| Configuration | emerge exit | From binary packages | Must be compiled | Download |"
	echo "|---|---|---|---|---|"
	for name in A-stock B-ours-stable C-ours-testing; do
		bin=$(grep -c '^\[binary' "$OUT/$name.txt")
		src=$(grep -c '^\[ebuild' "$OUT/$name.txt")
		rc=$(cat "$OUT/$name.rc" 2>/dev/null || echo "-")
		dl=$(grep -o 'Size of downloads: .*' "$OUT/$name.txt" | head -1)
		echo "| $name | $rc | $bin | $src | ${dl:-n/a} |"
	done
	echo
	echo "- A-stock: Gentoo's default amd64 profile, stable keywords (what the binary host is built for)"
	echo "- B-ours-stable: our agentic profile, stable keywords"
	echo "- C-ours-testing: our agentic profile with ~amd64, as gentoo/config/make.conf.example does today"
	for name in A-stock B-ours-stable C-ours-testing; do
		echo
		echo "## $name: packages that would be compiled"
		echo
		echo '```'
		grep '^\[ebuild' "$OUT/$name.txt" | sed -E 's/^\[ebuild[^]]*\] //' | head -80
		echo '```'
		if [ "$(cat "$OUT/$name.rc" 2>/dev/null)" != 0 ]; then
			echo
			echo "emerge did not succeed; the end of its output:"
			echo
			echo '```'
			tail -40 "$OUT/$name.txt"
			echo '```'
		fi
	done
} > "$OUT/summary.md"
cat "$OUT/summary.md"
