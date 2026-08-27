# Interstellar OS — Build & Boot in a VM (beginner guide)

Goal: go from a Windows PC → a virtual machine → a running **Interstellar OS**
(our Gentoo fork). Follow the milestones **in order**. Each milestone is a real,
demoable win, so you don't have to finish everything to have something to show.

> ⏱️ **Reality check.** Gentoo is source-based, so parts of this take **hours** of
> compiling and need a **stable internet connection**. Budget most of a day. Take a
> VM **snapshot** after each milestone so a mistake never costs you the whole thing.

---

## The milestone ladder

| Milestone | What you get | Difficulty |
|---|---|---|
| **A** | Plain Gentoo Linux boots in the VM | Medium (long) |
| **B** | System renames itself to **"Interstellar OS"** — *the fork is proven* | Easy ✅ |
| **C** | Our AI shell + AI core installed *(optional, heavy compile)* | Hard (long) |
| **D** | A bootable `interstellar-os.iso` *(advanced)* | Hard |

**Milestone B is the real trophy** — `cat /etc/os-release` printing *Interstellar OS*
proves you forked Linux. It's light. Aim for A → B first.

---

## Step 0 — Turn on virtualization (one-time)

Your CPU can run VMs, but it's often off by default.
1. Restart the PC → enter **BIOS/UEFI** (tap `Del`, `F2`, `F10`, or `Esc` at boot — varies).
2. Find **Intel VT-x** / **AMD-V** / **SVM Mode** / "Virtualization" → set **Enabled**.
3. Save & exit.

(If VirtualBox later complains "VT-x not available", this is why.)

---

## Step 1 — Install VirtualBox

1. Go to **virtualbox.org → Downloads → Windows hosts**. (Oracle VirtualBox is fine — your example works great. VMware Workstation Player also works if you prefer.)
2. Run the installer, click Next through it, Finish.

---

## Step 2 — Download the Gentoo ISO

1. Go to **gentoo.org → Downloads**.
2. Under **amd64**, download the **"LiveGUI USB Image" (.iso)** — it has a graphical
   environment and all tools, easiest for beginners. (The "Minimal Installation CD"
   also works if you're comfortable with a text-only screen.)
3. Remember where the `.iso` file saved.

---

## Step 3 — Create the VM

In VirtualBox:
1. **New** → Name: `Interstellar OS` → Type: **Linux**, Version: **Gentoo (64-bit)** → Next.
2. **Memory:** 4096 MB minimum, **8192 MB** if you can spare it (compiling loves RAM).
3. **Create a virtual hard disk now** → VDI → Dynamically allocated → **60 GB**.
4. Select the VM → **Settings**:
   - **System → Processor:** give it **4 CPUs** (compiling is much faster).
   - **System → Motherboard:** tick **Enable EFI**. (Important — matches our installer.)
   - **Storage:** click the empty CD → pick your Gentoo **.iso**.
   - **Network:** Adapter 1 = **NAT** (gives internet).
5. **OK.**

📸 *Take a snapshot now: right-click VM → Snapshots → Take → name it "fresh VM".*

---

## Step 4 — Boot the VM & open a terminal

1. Select the VM → **Start**. It boots the Gentoo live environment.
2. Open a **Terminal** (on LiveGUI it's in the menu; on minimal you're already at a shell).
3. Test internet: `ping -c3 gentoo.org` → you should see replies. (If not, check the
   Network setting = NAT and restart the VM.)

---

## Milestone A — Install base Gentoo

> This is condensed from the official **Gentoo Handbook (amd64)**. Keep the handbook
> open too: `wiki.gentoo.org/wiki/Handbook:AMD64`. Commands below assume the whole
> disk `/dev/sda` is the VM disk (safe — it's a virtual disk, nothing of yours).

### A1. Partition the disk (UEFI layout)
```bash
sudo -i                       # become root for the whole install
parted -a optimal /dev/sda
  mklabel gpt
  mkpart ESP fat32 1MiB 513MiB
  set 1 esp on
  mkpart root ext4 513MiB 100%
  quit
mkfs.vfat -F32 /dev/sda1
mkfs.ext4 /dev/sda2
mount /dev/sda2 /mnt/gentoo
mkdir -p /mnt/gentoo/efi
mount /dev/sda1 /mnt/gentoo/efi
```

### A2. Download & unpack the stage3 (the base system)
```bash
cd /mnt/gentoo
# On LiveGUI you can use a browser, or grab it with a helper. Easiest:
links https://www.gentoo.org/downloads/    # pick "Stage 3 openrc" amd64, save here
# ...or download the stage3-*-openrc-*.tar.xz however you like into /mnt/gentoo
tar xpvf stage3-*.tar.xz --xattrs-include='*.*' --numeric-owner
```

### A3. Basic Portage config
```bash
nano /mnt/gentoo/etc/portage/make.conf
# add these lines (tune -j to your CPU count):
#   MAKEOPTS="-j4"
#   ACCEPT_KEYWORDS="~amd64"
#   GRUB_PLATFORMS="efi-64"
```

### A4. Enter the new system (chroot)
```bash
cp -L /etc/resolv.conf /mnt/gentoo/etc/
mount --types proc /proc /mnt/gentoo/proc
mount --rbind /sys /mnt/gentoo/sys
mount --rbind /dev /mnt/gentoo/dev
mount --bind /run /mnt/gentoo/run
chroot /mnt/gentoo /bin/bash
source /etc/profile
emerge-webrsync            # download the Gentoo package tree
```

### A5. Timezone, locale, profile
```bash
echo "Asia/Kolkata" > /etc/timezone && emerge --config sys-libs/timezone-data
echo "en_US.UTF-8 UTF-8" >> /etc/locale.gen && locale-gen
eselect profile list       # pick a default/linux/amd64 desktop profile for now
eselect profile set <number-for-a-desktop-profile>
```

### A6. Kernel — use the **prebuilt** one (saves hours!)
```bash
echo "sys-kernel/installkernel dracut grub" >> /etc/portage/package.use/kernel
emerge sys-kernel/gentoo-kernel-bin        # binary kernel — no long compile
emerge sys-kernel/linux-firmware
```

### A7. Fstab, hostname, root password
```bash
emerge sys-apps/genfstab || true
# Simple fstab:
cat > /etc/fstab <<EOF
/dev/sda2  /     ext4  defaults,noatime  0 1
/dev/sda1  /efi  vfat  defaults          0 2
EOF
echo "interstellar" > /etc/hostname
passwd            # set a root password you'll remember
```

### A8. Bootloader (GRUB, UEFI)
```bash
emerge sys-boot/grub
grub-install --target=x86_64-efi --efi-directory=/efi --removable
grub-mkconfig -o /boot/grub/grub.cfg
```

### A9. Reboot into Gentoo
```bash
exit                      # leave chroot
umount -R /mnt/gentoo
reboot
```
In VirtualBox, remove the ISO first (Devices → Optical Drives → Remove disk) so it
boots the disk, not the installer. You should land at a login — log in as `root`.

🎉 **Milestone A done: Gentoo boots.** 📸 *Take a snapshot: "gentoo base".*

---

## Milestone B — Make it **Interstellar OS** (the fork!)

Now we layer *our* overlay on top. This part is light.

```bash
# 1. Get our project
emerge dev-vcs/git
git clone https://github.com/akashmanjunath2505/osaima.git /root/osaima
cd /root/osaima

# 2. Install our overlay into the system
cp -a gentoo/overlay /var/db/repos/interstellar
mkdir -p /etc/portage/repos.conf
cp gentoo/config/repos.conf/interstellar.conf /etc/portage/repos.conf/
# (edit that file so location = /var/db/repos/interstellar and auto-sync = no)
nano /etc/portage/repos.conf/interstellar.conf

# 3. Switch to OUR profile
eselect profile list                         # you'll now see interstellar/base + interstellar/agentic
eselect profile set interstellar:interstellar/base

# 4. Install our identity/branding package
emerge app-misc/interstellar-release

# 5. THE MOMENT OF TRUTH
cat /etc/os-release
```
You should see:
```
NAME="Interstellar OS"
ID=interstellar
ID_LIKE=gentoo
PRETTY_NAME="Interstellar OS (OSAIMA) 0.1"
```

🏆 **Milestone B done — you forked Linux.** Your OS now identifies as Interstellar OS.
This is the screenshot to put in your report. 📸 *Snapshot: "interstellar branded".*

---

## Milestone C — Install our AI shell + core *(optional, long)*

⚠️ This compiles Rust, a WebKit webview and Node — it can take **a long time** and
needs plenty of RAM/disk. Only do it if you have time. Switch to the full flavor:

```bash
eselect profile set interstellar:interstellar/agentic
emerge sys-apps/osaima-ai-core          # builds the MCP daemon (Rust)
emerge gui-apps/osaima-shell            # builds the Tauri + Lua shell (long!)
# or everything at once:
emerge app-misc/interstellar-meta
```

**Faster alternative to still "show the UI on Linux":** the desktop shell is just
web files. Inside the VM you can run it in a browser without the heavy compile:
```bash
emerge www-client/firefox dev-lang/python   # (or use an existing browser)
cd /root/osaima/ui/osaima-shell
python -m http.server 8777
# open http://localhost:8777 in the VM's browser → the full shell, running on Interstellar OS
```

📸 *Snapshot when it works.*

---

## Milestone D — Build a shareable ISO *(advanced)*

Once the system is set up, from the repo:
```bash
sudo STAGE3_URL="<a gentoo amd64 openrc stage3 URL>" build-tools/scripts/build_distro.sh
sudo build-tools/scripts/create_iso.sh
```
This produces `interstellar-os-*.iso` you could boot on another machine/VM. This is
the hardest step — attempt it last.

---

## Survival tips

- **Snapshots are your friend.** One after each milestone. If anything breaks:
  right-click VM → Snapshots → restore.
- **Compiling looks frozen but isn't** — it just takes long. Leave it.
- **Out of disk?** You made the disk too small; recreate the VM with 60–80 GB.
- **"VT-x not available"** → redo Step 0 (BIOS virtualization).
- **No internet in VM** → Settings → Network → Adapter 1 = NAT → restart VM.
- **Stuck on a Gentoo step** → the official Handbook is the source of truth:
  `wiki.gentoo.org/wiki/Handbook:AMD64`.

## What to aim for
For your project, **Milestone A + B is a complete, impressive result**: "we forked
Gentoo into Interstellar OS, here it is booting and identifying itself." C and D are
bonus. Don't stress if you stop at B.
