# Copyright 2025 Interstellar OS / OSAIMA project
# Distributed under the terms of the GNU General Public License v2

EAPI=8

DESCRIPTION="Interstellar OS meta package — pulls the full agentic desktop"
HOMEPAGE="https://github.com/Blehh13/osaima"

LICENSE="metapackage"
SLOT="0"
KEYWORDS="~amd64"
IUSE="+shell +ai desktop devtools"

# A meta package has no source — it only expresses dependencies.
RDEPEND="
	app-misc/interstellar-release
	shell? ( gui-apps/osaima-shell )
	ai? ( sys-apps/osaima-ai-core )
	desktop? (
		gui-wm/sway
		x11-terms/foot
		gui-libs/xdg-desktop-portal-wlr
	)
	devtools? (
		sys-devel/gcc
		dev-lang/rust
		dev-lang/python
		net-libs/nodejs
		dev-vcs/git
	)
"

# Nothing to compile or install for a meta package.
