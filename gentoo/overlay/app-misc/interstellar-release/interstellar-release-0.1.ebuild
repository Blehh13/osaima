# Copyright 2025 Interstellar OS / OSAIMA project
# Distributed under the terms of the GNU General Public License v2

EAPI=8

DESCRIPTION="Interstellar OS (OSAIMA) release identification and branding files"
HOMEPAGE="https://github.com/akashmanjunath2505/osaima"

LICENSE="GPL-2"
SLOT="0"
KEYWORDS="~amd64"
IUSE=""

# This package owns /etc/os-release for the distro, so it must replace the
# baselayout-provided one.
RDEPEND="!!sys-apps/baselayout[-build]"

S="${WORKDIR}"

src_install() {
	# Distro identity — what `cat /etc/os-release` reports.
	insinto /etc
	doins "${FILESDIR}"/os-release
	dosym ../etc/os-release /usr/lib/os-release

	# Legacy identity + login banner.
	newins "${FILESDIR}"/os-release interstellar-release
	insinto /etc
	doins "${FILESDIR}"/issue
}
