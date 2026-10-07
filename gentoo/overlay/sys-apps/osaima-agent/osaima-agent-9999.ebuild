# Copyright 2025 Interstellar OS / OSAIMA project
# Distributed under the terms of the GNU General Public License v3

EAPI=8

PYTHON_COMPAT=( python3_{12,13} )

inherit git-r3 python-single-r1 systemd

DESCRIPTION="OSAIMA assistant: turns requests into safe system actions through the AI core"
HOMEPAGE="https://github.com/Blehh13/osaima"
EGIT_REPO_URI="https://github.com/Blehh13/osaima.git"
EGIT_CLONE_TYPE="shallow"
# Only our code is needed: never fetch the kernel / portage submodules.
EGIT_SUBMODULES=()

LICENSE="GPL-3+"
SLOT="0"
KEYWORDS=""  # live ebuild
IUSE="voice"
REQUIRED_USE="${PYTHON_REQUIRED_USE}"

# The Anthropic SDK is not packaged in Gentoo, so its Python dependencies are
# installed from PyPI into a private directory at build time. That needs the
# network, which Portage's sandbox blocks by default.
RESTRICT="network-sandbox"
# Wheels from PyPI contain prebuilt libraries (pydantic-core).
QA_PREBUILT="usr/lib/${PN}/*"

RDEPEND="
	${PYTHON_DEPS}
	sys-apps/osaima-ai-core
	voice? ( || ( media-video/pipewire media-sound/alsa-utils ) )
"
BDEPEND="
	${PYTHON_DEPS}
	$(python_gen_cond_dep 'dev-python/pip[${PYTHON_USEDEP}]')
"

S="${WORKDIR}/${P}/ai-core/agent"

src_install() {
	local site="/usr/lib/${PN}/site"
	# faster-whisper and Piper are not packaged in Gentoo either.
	local target="."
	use voice && target=".[voice]"

	"${EPYTHON}" -m pip install \
		--no-cache-dir --disable-pip-version-check --no-compile \
		--target "${ED}${site}" "${target}" || die "pip install failed"
	python_optimize "${ED}${site}"

	# Run from the private directory; nothing is installed system-wide.
	cat > "${T}/osaima-agent" <<-EOF || die
	#!/bin/sh
	export PYTHONPATH="${site}\${PYTHONPATH:+:\$PYTHONPATH}"
	exec ${EPYTHON} -m osaima_agent "\$@"
	EOF
	dobin "${T}/osaima-agent"

	systemd_newuserunit "${FILESDIR}"/osaima-agent.service osaima-agent.service
	dodoc README.md
}

pkg_postinst() {
	elog "Check the setup, and download the local model, with:"
	elog "  osaima-agent doctor --pull"
	elog "The Interstellar desktop session starts the assistant automatically."
	elog "Settings (optional): ~/.config/osaima/agent.toml"
	if use voice; then
		elog "Voice: download a speaking voice once (needs the network):"
		elog "  python -m piper.download_voices en_US-lessac-medium --data-dir ~/.local/share/osaima/voice"
		elog "Speech recognition downloads its model the first time you use the microphone."
	fi
}
