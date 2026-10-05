# Copyright 2025 Interstellar OS / OSAIMA project
# Distributed under the terms of the GNU General Public License v2

EAPI=8

DESCRIPTION="Interstellar OS (OSAIMA) release identification and branding files"
HOMEPAGE="https://github.com/Blehh13/osaima"

LICENSE="GPL-2"
SLOT="0"
KEYWORDS="~amd64"
IUSE=""

S="${WORKDIR}"

src_install() {
	# Distro identity. /etc/os-release is the real file; /usr/lib/os-release is a
	# relative symlink to it (matches the os-release(5) search order).
	insinto /etc
	doins "${FILESDIR}"/os-release
	dosym ../../etc/os-release /usr/lib/os-release

	# Console login banner.
	doins "${FILESDIR}"/issue
}

pkg_postinst() {
	elog "Interstellar OS branding installed."
	elog "This package ships /etc/os-release, which sys-apps/baselayout also owns."
	elog "If the merge is blocked by a file collision, allow it once with:"
	elog "  COLLISION_IGNORE=\"/etc/os-release /usr/lib/os-release\" emerge app-misc/interstellar-release"
}
