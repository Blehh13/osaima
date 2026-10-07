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
CONFIGS=(A-stock A2-desktop B-ours-stable C-ours-testing)

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

# Packages that stable Gentoo doesn't have (only ~amd64 does) make emerge stop at
# the first one. Unmask them as they turn up and record which ones, because each
# is something the image can't take from the stable binary host.
KEYWORDS_FILE=/etc/portage/package.accept_keywords/zz-spike

# Prints the atoms of packages masked only by the ~amd64 keyword.
masked_atoms() {
	grep -E '^- [^ ]+::[^ ]+ \(masked by: ~amd64 keyword\)' "$1" |
		sed -E 's/^- ([^ ]+)-[0-9][^ ]*::.*$/\1/' | sort -u
}

# Select a profile: Gentoo's with eselect, ours by linking the overlay's directory.
select_profile() {
	local profile=$1
	if [[ $profile == interstellar:* ]]; then
		ln -sfn "/var/db/repos/interstellar/profiles/${profile#interstellar:}" /etc/portage/make.profile
	else
		eselect profile set "$profile"
	fi
	ls -l /etc/portage/make.profile
	# Show the profile's real parse error, if it has one.
	portageq envvar ARCH > /dev/null 2> "$OUT/profile-error.txt" || cat "$OUT/profile-error.txt"
}

# name | profile | ACCEPT_KEYWORDS
run() {
	local name=$1 profile=$2 keywords=$3 attempt atom
	log "Experiment $name: profile=$profile keywords=$keywords"
	if ! select_profile "$profile"; then
		echo "could not select the profile $profile" > "$OUT/$name.txt"
		echo 1 > "$OUT/$name.rc"
		: > "$OUT/$name.unmasked"
		return
	fi
	: > "$KEYWORDS_FILE"
	: > "$OUT/$name.unmasked"
	for attempt in $(seq 1 15); do
		ACCEPT_KEYWORDS="$keywords" emerge --pretend --verbose --getbinpkg --usepkg \
			--with-bdeps=n --color=n --quiet-build=y "${PACKAGES[@]}" > "$OUT/$name.txt" 2>&1
		echo $? > "$OUT/$name.rc"
		new=$(masked_atoms "$OUT/$name.txt")
		[ -z "$new" ] && break
		for atom in $new; do
			echo "$atom ~amd64" >> "$KEYWORDS_FILE"
			echo "$atom" >> "$OUT/$name.unmasked"
		done
	done
	echo "emerge exit code: $(cat "$OUT/$name.rc")"
	echo "needed testing keywords: $(sort -u "$OUT/$name.unmasked" | paste -sd' ' -)"
	echo "binary packages: $(grep -c '^\[binary' "$OUT/$name.txt")"
	echo "to compile:      $(grep -c '^\[ebuild' "$OUT/$name.txt")"
}

run A-stock "$ORIGINAL_PROFILE" amd64
run A2-desktop "${ORIGINAL_PROFILE}/desktop" amd64
run B-ours-stable interstellar:interstellar/agentic amd64
run C-ours-testing interstellar:interstellar/agentic '~amd64'

log "Why are some packages not taken from the binary host?"
# For every package A2 would compile, look it up in the host's index: is it
# there at all, in which versions, and which USE flags differ from what we want?
index=$(find /var/cache -path '*binhost*' -name Packages 2>/dev/null | head -1)
echo "index: ${index:-not found}"
if [ -n "$index" ]; then
	python3 - "$index" "$OUT/A2-desktop.txt" > "$OUT/binhost-diagnosis.txt" <<'PY'
import re
import sys

index, listing = sys.argv[1:3]
by_pkg = {}
for block in open(index, encoding="utf-8", errors="replace").read().split("\n\n"):
    fields = dict(line.split(": ", 1) for line in block.splitlines() if ": " in line)
    cpv = fields.get("CPV")
    if cpv:
        by_pkg.setdefault(re.sub(r"-[0-9][^/]*$", "", cpv), []).append(fields)

print("| Package | Version we would build | On the binary host | USE flags that differ |")
print("|---|---|---|---|")
for line in open(listing, encoding="utf-8", errors="replace"):
    m = re.match(r"\[ebuild[^\]]*\] (\S+)(.*)", line)
    if not m:
        continue
    cpv = m.group(1).split("::")[0].split(":")[0]
    pkg = re.sub(r"-[0-9][^/]*$", "", cpv)
    version = cpv[len(pkg) + 1:]
    if pkg.endswith("-9999") or "/osaima-" in pkg or "/interstellar-" in pkg:
        continue
    offered = by_pkg.get(pkg, [])
    if not offered:
        print(f"| {pkg} | {version} | not on the host | |")
        continue
    versions = sorted({o["CPV"][len(pkg) + 1:] for o in offered})
    use = re.search(r'USE="([^"]*)"', m.group(2))
    wanted = {t.strip("()%*") for t in (use.group(1).split() if use else []) if not t.startswith("-")}
    last = offered[-1]
    have = set(last.get("USE", "").split())
    known = {f.lstrip("+-") for f in last.get("IUSE", "").split()}
    diff = sorted(f"+{f}" for f in (wanted - have) & known) + sorted(f"-{f}" for f in (have - wanted) & known)
    print(f"| {pkg} | {version} | {', '.join(versions)} | {' '.join(diff) or 'none (version differs)'} |")
PY
	cat "$OUT/binhost-diagnosis.txt"
fi

log "Writing the summary"
{
	echo "# Binary host check"
	echo
	echo "Packages asked for: ${PACKAGES[*]}"
	echo
	echo "| Configuration | emerge exit | From binary packages | Must be compiled | Download |"
	echo "|---|---|---|---|---|"
	for name in "${CONFIGS[@]}"; do
		bin=$(grep -c '^\[binary' "$OUT/$name.txt")
		src=$(grep -c '^\[ebuild' "$OUT/$name.txt")
		rc=$(cat "$OUT/$name.rc" 2>/dev/null || echo "-")
		dl=$(grep -o 'Size of downloads: .*' "$OUT/$name.txt" | head -1)
		echo "| $name | $rc | $bin | $src | ${dl:-n/a} |"
	done
	echo
	echo "- A-stock: Gentoo's default amd64 profile, stable keywords (what the binary host is built for)"
	echo "- A2-desktop: Gentoo's desktop profile, stable keywords"
	echo "- B-ours-stable: our agentic profile, stable keywords"
	echo "- C-ours-testing: our agentic profile with ~amd64, as gentoo/config/make.conf.example does today"
	echo
	echo "Packages that only exist in the testing branch (~amd64), so they can never come from the stable binary host:"
	for name in "${CONFIGS[@]}"; do
		echo "- $name: $(sort -u "$OUT/$name.unmasked" | paste -sd' ' -)"
	done
	if [ -f "$OUT/binhost-diagnosis.txt" ]; then
		echo
		echo "## A2-desktop: why these were not taken from the binary host"
		echo
		cat "$OUT/binhost-diagnosis.txt"
	fi
	for name in "${CONFIGS[@]}"; do
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
