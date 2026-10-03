# Copyright 2025 Interstellar OS / OSAIMA project
# Distributed under the terms of the GNU General Public License v3

EAPI=8

inherit git-r3

DESCRIPTION="OSAIMA agentic desktop shell — Tauri UI with a Lua-driven tiling window manager"
HOMEPAGE="https://github.com/akashmanjunath2505/osaima"
EGIT_REPO_URI="https://github.com/akashmanjunath2505/osaima.git"
EGIT_CLONE_TYPE="shallow"
# Only our code is needed — never fetch the kernel / portage submodules.
EGIT_SUBMODULES=()

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
	media-fonts/inter
	media-fonts/jetbrains-mono
"
DEPEND="${RDEPEND}"
# Build: Rust/Cargo for the Tauri host, Node for the frontend assets.
BDEPEND="
	|| ( dev-lang/rust-bin dev-lang/rust )
	net-libs/nodejs[npm]
"

S="${WORKDIR}/${P}/ui/osaima-shell"

src_compile() {
	# NOTE: npm/cargo fetch dependencies at build time, so this package needs
	# Gentoo's build-time network sandbox disabled. Enable it per-package via:
	#   /etc/portage/env/allow-net.conf:  FEATURES="-network-sandbox"
	#   /etc/portage/package.env:         gui-apps/osaima-shell allow-net.conf
	# Frontend is bundler-less static ES modules — no npm build step required,
	# but install JS deps so `tauri build` can bundle the webview.
	npm ci --no-audit --no-fund || npm install
	# Build the release Tauri binary only. --no-bundle skips .deb/.AppImage
	# packaging (which would need extra host tooling and network access);
	# the ebuild installs the raw release binary directly.
	npx --yes @tauri-apps/cli build --no-bundle || die "tauri build failed"
}

src_install() {
	dobin src-tauri/target/release/osaima-shell
	# Ship the frontend assets + the Lua window-manager config.
	# (Web assets live in their own folder so `tauri build` won't try to
	# bundle node_modules / src-tauri into the app.)
	insinto /usr/share/osaima-shell
	doins -r frontend/src frontend/index.html
	# A desktop session entry so a display manager can launch the shell.
	insinto /usr/share/wayland-sessions
	newins "${FILESDIR}"/osaima-shell.desktop osaima-shell.desktop
}
