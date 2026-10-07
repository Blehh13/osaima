# Copyright 2025 Interstellar OS / OSAIMA project
# Distributed under the terms of the GNU General Public License v3

EAPI=8

inherit git-r3

DESCRIPTION="OSAIMA agentic desktop shell: Tauri UI with a Lua-driven tiling window manager"
HOMEPAGE="https://github.com/Blehh13/osaima"
EGIT_REPO_URI="https://github.com/Blehh13/osaima.git"
EGIT_CLONE_TYPE="shallow"
# Only our code is needed: never fetch the kernel / portage submodules.
EGIT_SUBMODULES=()

LICENSE="GPL-3"
SLOT="0"
KEYWORDS=""  # live ebuild: keyword empty on purpose
IUSE=""

# npm and cargo download dependencies at build time, which Portage's sandbox
# blocks by default.
RESTRICT="network-sandbox"

# Runtime: the Tauri WebKit webview, the compositor the session runs on, and
# the services the shell talks to.
RDEPEND="
	net-libs/webkit-gtk:4.1
	dev-libs/glib
	x11-libs/gtk+:3
	gui-wm/sway
	sys-apps/osaima-ai-core
	sys-apps/osaima-agent
	media-fonts/noto
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
	# The frontend is bundler-less static ES modules, so there is no npm build
	# step, but the JS dependencies are needed for `tauri build`.
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

	# The session: starts the AI core, the assistant and Ollama, then sway
	# running the shell. Display managers launch it from the session entry.
	dobin "${FILESDIR}"/osaima-session
	insinto /usr/share/osaima
	doins "${FILESDIR}"/sway.conf
	insinto /usr/share/wayland-sessions
	newins "${FILESDIR}"/osaima-shell.desktop osaima-shell.desktop
}
