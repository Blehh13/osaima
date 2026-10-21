# Copyright 2025 Interstellar OS / OSAIMA project
# Distributed under the terms of the GNU General Public License v2

EAPI=8

DESCRIPTION="Interstellar OS (OSAIMA) release identification and branding files"
HOMEPAGE="https://github.com/akashmanjunath2505/osaima"

LICENSE="GPL-2"
SLOT="0"
KEYWORDS="~amd64"
IUSE=""

S="${WORKDIR}"

src_install() {
	# Distro identity, laid out as os-release(5) recommends for vendors: the real
	# file in /usr/lib and /etc/os-release a symlink to it. (Shipping the file in
	# /etc would be config-protected next to baselayout's symlink, leaving
	# /etc/os-release -> /usr/lib/os-release -> /etc/os-release: a loop.)
	insinto /usr/lib
	doins "${FILESDIR}"/os-release
	dosym ../usr/lib/os-release /etc/os-release

	# Console login banner.
	insinto /etc
	doins "${FILESDIR}"/issue
}

pkg_postinst() {
	elog "Interstellar OS branding installed."
	elog "The interstellar/* profiles set COLLISION_IGNORE for the os-release files"
	elog "baselayout also ships. To stop a baselayout update from restoring Gentoo's,"
	elog "mask them for baselayout only:"
	elog "  echo 'INSTALL_MASK=\"/etc/os-release /usr/lib/os-release\"' > /etc/portage/env/no-os-release.conf"
	elog "  echo 'sys-apps/baselayout no-os-release.conf' >> /etc/portage/package.env"
}
