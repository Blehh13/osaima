# Copyright 2025 Interstellar OS / OSAIMA project
# Distributed under the terms of the GNU General Public License v3

EAPI=8

inherit git-r3 systemd udev

DESCRIPTION="OSAIMA AI core: MCP server exposing system context and safe actions"
HOMEPAGE="https://github.com/Blehh13/osaima"
EGIT_REPO_URI="https://github.com/Blehh13/osaima.git"
EGIT_CLONE_TYPE="shallow"
# Only our code is needed: never fetch the kernel / portage submodules.
EGIT_SUBMODULES=()

LICENSE="GPL-3+"
SLOT="0"
KEYWORDS=""  # live ebuild
IUSE=""

# Cargo downloads crates at build time, which Portage's sandbox blocks by default.
RESTRICT="network-sandbox"

# Runtime tools used by individual MCP tools: PipeWire's wpctl (volume),
# elogind's loginctl (power actions), sway's IPC (window control).
RDEPEND="
	virtual/udev
	media-video/wireplumber
	sys-auth/elogind
"
DEPEND="${RDEPEND}"
# A Rust toolchain providing cargo (source or binary).
BDEPEND="|| ( dev-lang/rust-bin dev-lang/rust )"

S="${WORKDIR}/${P}/ai-core/mcp-daemon"

src_compile() {
	cargo build --release --locked || die "cargo build failed"
}

src_install() {
	newbin target/release/mcp-daemon osaima-mcp-daemon
	# systemd user service that starts the daemon (PDR: osaima-*.service).
	systemd_newuserunit "${FILESDIR}"/osaima-mcp-daemon.service osaima-mcp-daemon.service
	# Lets the video group change screen brightness without root.
	udev_dorules "${FILESDIR}"/90-osaima-backlight.rules
	dodoc README.md
}

pkg_postinst() {
	udev_reload
	elog "Screen brightness control needs your user in the 'video' group:"
	elog "  gpasswd -a <user> video"
	elog "Then log out and back in."
}

pkg_postrm() {
	udev_reload
}
