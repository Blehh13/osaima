# Copyright 2025 Interstellar OS / OSAIMA project
# Distributed under the terms of the GNU General Public License v3

EAPI=8

inherit git-r3

DESCRIPTION="OSAIMA agentic desktop shell — Tauri UI with a Lua-driven tiling window manager"
HOMEPAGE="https://github.com/akashmanjunath2505/osaima"
EGIT_REPO_URI="https://github.com/akashmanjunath2505/osaima.git"
EGIT_CLONE_TYPE="shallow"

LICENSE="GPL-3"
SLOT="0"
KEYWORDS=""  # live ebuild — keyword empty on purpose
IUSE=""

# Runtime: the Tauri WebKit webview + a portal for Wayland.
RDEPEND="
	net-libs/webkit-gtk:4.1
	dev-libs/glib
	x11-libs/gtk+:3
	sys-apps/osaima-ai-core
"
DEPEND="${RDEPEND}"
# Build: Rust/Cargo for the Tauri host, Node for the frontend assets.
BDEPEND="
	virtual/rust
	net-libs/nodejs[npm]
"

S="${WORKDIR}/${P}/ui/osaima-shell"

src_compile() {
	# Frontend is bundler-less static ES modules — no npm build step required,
	# but install JS deps so `tauri build` can bundle the webview.
	npm ci --no-audit --no-fund || npm install
	# Build the release Tauri binary.
	npx --yes @tauri-apps/cli build || die "tauri build failed"
}

src_install() {
	dobin src-tauri/target/release/osaima-shell
	# Ship the frontend assets + the Lua window-manager config.
	insinto /usr/share/osaima-shell
	doins -r src index.html
	# A desktop session entry so a display manager can launch the shell.
	insinto /usr/share/wayland-sessions
	newins "${FILESDIR}"/osaima-shell.desktop osaima-shell.desktop
}
