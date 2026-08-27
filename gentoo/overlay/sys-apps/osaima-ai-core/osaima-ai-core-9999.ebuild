# Copyright 2025 Interstellar OS / OSAIMA project
# Distributed under the terms of the GNU General Public License v3

EAPI=8

inherit git-r3 systemd

DESCRIPTION="OSAIMA AI core — MCP daemon exposing system context over a Unix socket"
HOMEPAGE="https://github.com/akashmanjunath2505/osaima"
EGIT_REPO_URI="https://github.com/akashmanjunath2505/osaima.git"
EGIT_CLONE_TYPE="shallow"
# Only our code is needed — never fetch the kernel / portage submodules.
EGIT_SUBMODULES=()

LICENSE="GPL-3"
SLOT="0"
KEYWORDS=""  # live ebuild
IUSE=""

RDEPEND=""
DEPEND="${RDEPEND}"
# A Rust toolchain providing cargo (source or binary).
BDEPEND="|| ( dev-lang/rust-bin dev-lang/rust )"

S="${WORKDIR}/${P}/ai-core/mcp-daemon"

src_compile() {
	cargo build --release || die "cargo build failed"
}

src_install() {
	newbin target/release/mcp-daemon osaima-mcp-daemon
	# systemd user service that starts the daemon (PDR: osaima-*.service).
	systemd_newuserunit "${FILESDIR}"/osaima-mcp-daemon.service osaima-mcp-daemon.service
}
