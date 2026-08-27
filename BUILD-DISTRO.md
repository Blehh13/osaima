# Building Interstellar OS — the Gentoo fork

Interstellar OS (OSAIMA) is **not** Gentoo with a new wallpaper — it is a
Gentoo-family derivative distribution. We keep upstream **Gentoo as our upstream**
and maintain only our own layer: an overlay, a profile, branding, and the
packages that make up the agentic desktop.

```
              Gentoo (upstream)
                     │
                     ▼
        ┌──────────────────────────┐
        │  Interstellar overlay     │   gentoo/overlay/
        │   profiles/ (our profile) │
        │   app-misc/interstellar-* │   release + meta packages
        │   gui-apps/osaima-shell   │   our UI shell (live ebuild)
        │   sys-apps/osaima-ai-core │   our MCP daemon (live ebuild)
        └────────────┬─────────────┘
                     ▼
              build_distro.sh  →  rootfs
                     ▼
               create_iso.sh   →  interstellar-os.iso
                     ▼
                  Bootable OS
```

## What lives where (this repo)

| Roadmap stage | In this repo |
|---|---|
| **3. Overlay** (Portage repo) | `gentoo/overlay/` (`metadata/layout.conf`, `profiles/repo_name`) |
| **4. Profile** | `gentoo/overlay/profiles/interstellar/{base,agentic}` |
| **5. First package** | `app-misc/interstellar-release` → installs `/etc/os-release` |
| **6. Distro packages** | `gui-apps/osaima-shell`, `sys-apps/osaima-ai-core`, `app-misc/interstellar-meta` |
| **7. Branding** | `app-misc/interstellar-release/files/{os-release,issue}` |
| **8. ISO** | `build-tools/scripts/build_distro.sh` → `create_iso.sh` |
| **9. Installer** | `installer/` (Calamares + `install.sh`) |

## Hands-on (do this in a Gentoo VM / WSL — not Windows)

Gentoo can't run on Windows, so the *build* runs on Linux. The fork itself is the
files above, which are complete and committable now.

**1. Get a Gentoo base** (VM: 4-8 GB RAM, 40-60 GB disk, UEFI). Boot the Gentoo
minimal ISO and install a normal stage3, or just unpack a stage3 into a chroot.

**2. Enable the overlay.** Copy this repo's overlay in and register it:
```bash
sudo cp -a gentoo/overlay /var/db/repos/interstellar
sudo cp gentoo/config/repos.conf/interstellar.conf /etc/portage/repos.conf/
sudo cp gentoo/config/make.conf.example /etc/portage/make.conf
```

**3. Select our profile:**
```bash
eselect profile list          # shows interstellar/base and interstellar/agentic
eselect profile set interstellar:interstellar/agentic
```

**4. Install the distro identity + desktop:**
```bash
emerge app-misc/interstellar-release     # branding: /etc/os-release
emerge app-misc/interstellar-meta        # shell + AI core + desktop + devtools
cat /etc/os-release                       # now reports "Interstellar OS"
```

**5. Build a full rootfs + ISO** (automated):
```bash
sudo STAGE3_URL="<gentoo amd64 openrc stage3 url>" build-tools/scripts/build_distro.sh
sudo build-tools/scripts/create_iso.sh
```

## The clean-fork rule

We **never** copy or diverge from the whole Gentoo tree. `masters = gentoo` in
`overlay/metadata/layout.conf` means Portage uses Gentoo packages by default and
*our* package only when we provide one. To take upstream updates: `emerge --sync`.

## Naming / trademark note

We ship `ID_LIKE=gentoo` (Gentoo-family) but our own `NAME`/`ID`. Before public
release, check Gentoo's trademark policy before using Gentoo branding directly —
licensing (GPL) and trademarks are separate things.
