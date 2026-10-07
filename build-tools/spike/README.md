# ISO feasibility spike

**Question:** can a bootable Interstellar OS image be built on a free GitHub
runner? Building Gentoo from source (what `BUILD-DISTRO.md` describes) takes many
hours, so the answer depends on how much can come from Gentoo's official binary
package host.

Run it from the Actions tab: **ISO feasibility spike** (it also runs when this
folder or `gentoo/` changes on a `spike/**` branch).

## Experiment 1: binary packages (done)

`binhost-check.sh` runs in a Gentoo stage3 container and asks Portage, with
`--pretend` (nothing is installed), what it would do to install
`app-misc/interstellar-meta` plus the kernel, bootloader and ISO tools. It counts
packages that come from the binary host (`https://distfiles.gentoo.org/releases/amd64/binpackages/23.0/x86-64`)
and packages that would be compiled.

| Configuration | Resolves | From binary packages | Compiled | Download |
|---|---|---|---|---|
| A: Gentoo default profile | no (WebKitGTK needs `wayland`, `X` or `aqua`) | | | |
| A2: Gentoo `desktop` profile | yes | 315 | 20 | 1.4 GB |
| B: our `agentic` profile | yes | 312 | 24 | 1.4 GB |
| C: our profile with `~amd64` (what `make.conf.example` used to set) | **no** | 43 | 32 | |
| D: B plus `USE=keyring` for `webkit-gtk` | yes | 271 | 23 | 1.4 GB |

(B and D are not directly comparable: the two resolve slightly different package
sets, 336 against 294 in total. I did not investigate why.)

What stays compiled in B and D, and why:

- **WebKitGTK** (the heaviest by far) is compiled in B because we build it with
  `keyring` off and the official package has it on. In D, setting that one flag
  turns it into a binary package. This is the difference between minutes and hours.
- **ggml and Ollama** are not on the binary host at all, and are testing-only
  (`~amd64`) packages; they must be compiled.
- **PipeWire and xdg-desktop-portal** differ in USE flags: the host builds them
  for systemd, we use OpenRC and elogind.
- The rest is small: our own live ebuilds, a few Python libraries, and binary
  tarballs (`rust-bin`, the kernel).

Runner: 4 CPUs, 15 GB RAM, 86 GB free disk, and `/dev/kvm` exists (so a VM can
be booted with hardware acceleration, to be confirmed).

### Bugs this found in our packaging (fixed on this branch)

1. The overlay did not declare the `portage-2` profile format, so the `agentic`
   profile's parent `gentoo:targets/desktop` could not be resolved. The profile
   had never worked.
2. The `agentic` profile's parent was `..` (the folder holding both profiles),
   not `../base`.
3. The profile and `make.conf.example` pinned Python 3.12, which conflicts with
   Gentoo's default 3.14 (exactly one target is allowed) and makes the binary
   packages not match. The pins are removed.
4. `osaima-agent` only allowed Python 3.12 and 3.13, so it could not be
   installed on a current Gentoo. It now allows 3.12 to 3.14.
5. `osaima-shell` depended on `media-fonts/inter`, which is not in Gentoo. The
   shell falls back to system fonts, so it now depends on `media-fonts/noto`.

### Also done here

- `gentoo/config/make.conf.example` no longer sets `ACCEPT_KEYWORDS="~amd64"`
  (configuration C). The packages that need testing or live keywords are listed
  in `gentoo/config/package.accept_keywords/osaima`.
- `gentoo/config/package.use/osaima` sets `net-libs/webkit-gtk keyring`.

## Not yet measured

These are the experiments that would finish answering the question. Times are
guesses until measured.

2. **Real install:** install the set above into a rootfs with `emerge --getbinpkg`
   and measure wall time and disk use, including the compiles (ggml, Ollama,
   PipeWire, our own cargo builds).
3. **ISO and boot:** turn that rootfs into a live ISO (squashfs, dracut,
   grub-mkrescue, as `build-tools/scripts/build_live_iso.sh` does) and boot it in
   QEMU on the runner, checking for a login prompt or the shell starting.
