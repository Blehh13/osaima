# Copyright 2025 Interstellar OS / OSAIMA project
# Distributed under the terms of the GNU General Public License v3

EAPI=8

inherit git-r3 systemd

DESCRIPTION="OSAIMA scheduler: learns what each program is doing and sets its CPU priority to match"
HOMEPAGE="https://github.com/Blehh13/osaima"
EGIT_REPO_URI="https://github.com/Blehh13/osaima.git"
EGIT_CLONE_TYPE="shallow"
# Only our code is needed: never fetch the kernel / portage submodules.
EGIT_SUBMODULES=()

LICENSE="GPL-3+"
SLOT="0"
KEYWORDS=""  # live ebuild
IUSE="+ebpf"

# cargo downloads the crates at build time, which Portage's sandbox blocks by default.
RESTRICT="network-sandbox"

# The eBPF telemetry is optional: without it the daemon reads the kernel's /proc accounting.
RDEPEND="
	ebpf? ( dev-util/bpftool )
"
BDEPEND="
	|| ( dev-lang/rust-bin dev-lang/rust )
	ebpf? (
		llvm-core/clang
		dev-libs/libbpf
		dev-util/bpftool
	)
"

S="${WORKDIR}/${P}/ai-core/schedd"

pkg_pretend() {
	if use ebpf && ! [[ -e /sys/kernel/btf/vmlinux ]]; then
		ewarn "The eBPF telemetry is built against the running kernel's types, which are"
		ewarn "exposed in /sys/kernel/btf/vmlinux. Run a kernel built with CONFIG_DEBUG_INFO_BTF"
		ewarn "(Gentoo's distribution kernels are), or build with USE=-ebpf."
	fi
}

src_compile() {
	cargo build --release --locked || die "cargo build failed"
	if use ebpf; then
		emake -C bpf || die "building the eBPF telemetry failed"
	fi
}

src_install() {
	dobin target/release/osaima-schedd

	# the trained classifier
	insinto /usr/share/osaima/schedd
	doins model/model.json

	if use ebpf; then
		insinto /usr/lib/osaima/schedd
		doins bpf/telemetry.bpf.o
	fi

	# a system service: raising priorities and loading eBPF both need privileges
	systemd_dounit "${FILESDIR}"/osaima-schedd.service
	dodoc README.md
}

pkg_postinst() {
	elog "osaima-schedd sets CPU priorities for ordinary users' programs and puts them back"
	elog "when it stops. Enable it with:"
	elog "  systemctl enable --now osaima-schedd"
	elog "Try it without changing anything: osaima-schedd run --dry-run --source proc -v"
}
