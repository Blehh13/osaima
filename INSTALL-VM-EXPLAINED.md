# Interstellar OS install — every command explained

Companion to [INSTALL-VM-GUIDE.md](INSTALL-VM-GUIDE.md). Same steps, but here each
command is explained in plain English: **what it does** and **why**. Read this
side-by-side while you type the commands from the guide.

---

## First, 8 words you'll keep seeing

- **Disk** — the VM's storage (here it's a 60 GB virtual file, `/dev/sda`).
- **Partition** — a slice of the disk. We make two: a small boot slice and a big main slice.
- **Filesystem** — the "formatting" that lets a partition hold files (like NTFS on Windows; on Linux we use `ext4` and `fat32`).
- **Mount** — attaching a partition to a folder so you can read/write it. Linux has no "C:" — everything hangs off `/` (the root).
- **stage3** — a tarball (zip-like file) containing a minimal working Linux system. The seed we grow from.
- **Portage** — Gentoo's app store / package manager (the `emerge` command).
- **Profile** — a preset bundle of default settings for the whole OS. **Ours renames it to Interstellar.**
- **chroot** — "change root": you step *inside* the new system-in-progress and run commands as if you'd already booted it.

---

## Step 0–4 (VM setup) — nothing to explain command-wise

That's all clicking in VirtualBox. The one command:
```bash
ping -c3 gentoo.org
```
- `ping` = "are you there?" to a server. `-c3` = do it 3 times then stop.
- **Why:** confirms the VM has internet. Gentoo downloads everything, so no internet = no install.

---

## Milestone A — base Gentoo, line by line

### A0. Become the administrator
```bash
sudo -i
```
- `sudo` = "do this as the superuser (root)", `-i` = "and stay logged in as root".
- **Why:** installing an OS touches protected system areas. Root = full permission.
  (You'll see the prompt end in `#` instead of `$` — that means you're root.)

---

### A1. Partition the disk

```bash
parted -a optimal /dev/sda
```
- `parted` = a disk-partitioning tool. `-a optimal` = align slices efficiently.
  `/dev/sda` = the whole VM disk. This opens `parted`'s own mini-prompt; the next
  lines are typed *inside* it.

```
mklabel gpt
```
- Puts a fresh **GPT** partition table on the disk — the modern map that says "this
  disk is divided like so." Wipes any old layout (fine — it's an empty virtual disk).

```
mkpart ESP fat32 1MiB 513MiB
set 1 esp on
```
- `mkpart ESP fat32 1MiB 513MiB` = make partition #1, ~512 MB, for booting. It's
  called the **ESP** (EFI System Partition) — modern PCs read the boot files from here.
- `set 1 esp on` = flag partition #1 as "this is the boot one."

```
mkpart root ext4 513MiB 100%
```
- Make partition #2 using **all the rest** of the disk. This is where the whole OS
  and your files live ("root").

```
quit
```
- Leave `parted`. Now the disk has two empty slices: a boot slice and a main slice.

```bash
mkfs.vfat -F32 /dev/sda1
mkfs.ext4 /dev/sda2
```
- `mkfs` = "make filesystem" (format). `mkfs.vfat -F32` formats the boot slice as
  FAT32 (what EFI requires). `mkfs.ext4` formats the main slice as ext4 (standard
  Linux filesystem). **Now they can hold files.**

```bash
mount /dev/sda2 /mnt/gentoo
mkdir -p /mnt/gentoo/efi
mount /dev/sda1 /mnt/gentoo/efi
```
- `mount /dev/sda2 /mnt/gentoo` = attach the main slice to the folder `/mnt/gentoo`.
  From now on, everything we put in `/mnt/gentoo` is really going onto that partition.
- `mkdir -p /mnt/gentoo/efi` = make an `efi` folder inside it.
- `mount /dev/sda1 /mnt/gentoo/efi` = attach the boot slice there.
- **Why:** we're building the new OS *inside* `/mnt/gentoo` before we boot it.

---

### A2. Unpack the stage3 (the seed system)

```bash
cd /mnt/gentoo
```
- `cd` = "change directory" — move into the folder we just mounted.

```bash
tar xpvf stage3-*.tar.xz --xattrs-include='*.*' --numeric-owner
```
- `tar` = the unzip tool for `.tar.xz` files. Flags: `x`=extract, `p`=keep
  permissions, `v`=show progress, `f`=from this file.
- `stage3-*.tar.xz` = the minimal-Linux tarball you downloaded (the `*` matches
  the version).
- `--xattrs-include` / `--numeric-owner` = keep special file attributes and owner
  IDs exactly. **Why:** a system won't work right if file permissions are scrambled.
- **Result:** `/mnt/gentoo` now contains a tiny but complete Linux system.

---

### A3. Basic Portage config

```bash
nano /mnt/gentoo/etc/portage/make.conf
```
- `nano` = a simple text editor in the terminal. This opens Gentoo's main settings
  file. Add the lines from the guide, then save with **Ctrl+O, Enter**, exit with **Ctrl+X**.
- What the lines mean:
  - `MAKEOPTS="-j4"` = compile using 4 CPU cores at once (faster). Match your VM's CPU count.
  - `ACCEPT_KEYWORDS="~amd64"` = allow newer "testing" packages (our live ebuilds need this).
  - `GRUB_PLATFORMS="efi-64"` = build the bootloader for modern EFI booting.

---

### A4. Enter the new system (chroot)

```bash
cp -L /etc/resolv.conf /mnt/gentoo/etc/
```
- `cp` = copy; `-L` = follow links. This copies the live system's **internet/DNS
  settings** into the new system. **Why:** so the new system can also reach the internet.

```bash
mount --types proc /proc /mnt/gentoo/proc
mount --rbind /sys /mnt/gentoo/sys
mount --rbind /dev /mnt/gentoo/dev
mount --bind /run /mnt/gentoo/run
```
- These attach special system folders (`/proc`, `/sys`, `/dev`, `/run`) from the
  live environment into the new one. They're not normal files — they're live views
  of hardware, processes, and devices.
- **Why:** once we step inside (chroot), the new system needs these to function
  (talk to the disk, CPU, etc.).

```bash
chroot /mnt/gentoo /bin/bash
source /etc/profile
```
- `chroot /mnt/gentoo /bin/bash` = **step inside** the new system. From now on `/`
  means the new Interstellar-to-be, not the live installer. This is the magic move.
- `source /etc/profile` = load the new system's settings into your current shell
  (updates your prompt, paths, etc.).

```bash
emerge-webrsync
```
- Downloads the **Portage tree** — the catalog of ~20,000 Gentoo packages, so
  `emerge` knows what's installable. **Why:** without it, you can't install anything.

---

### A5. Timezone, language, profile

```bash
echo "Asia/Kolkata" > /etc/timezone && emerge --config sys-libs/timezone-data
```
- `echo "..." > file` = write that text into the file (sets your timezone).
  `emerge --config sys-libs/timezone-data` = apply it. `&&` = "and then".

```bash
echo "en_US.UTF-8 UTF-8" >> /etc/locale.gen && locale-gen
```
- Enables English/UTF-8 text support. `>>` = *append* a line. `locale-gen` = build it.
- **Why:** sets the system's language/character handling.

```bash
eselect profile list
eselect profile set <number>
```
- `eselect profile list` = show the available preset bundles of defaults, each numbered.
- `eselect profile set <number>` = pick one (choose a plain `desktop` one for now;
  in **Milestone B we switch to our Interstellar profile**).
- **Why:** the profile decides thousands of default settings so you don't set them by hand.

---

### A6. The kernel (the actual "Linux")

```bash
echo "sys-kernel/installkernel dracut grub" >> /etc/portage/package.use/kernel
```
- Tells Portage: when installing the kernel, also set up `dracut` (builds the
  startup image) and `grub` (the boot menu). `>>` appends this preference to a file.

```bash
emerge sys-kernel/gentoo-kernel-bin
emerge sys-kernel/linux-firmware
```
- `gentoo-kernel-bin` = the **Linux kernel, prebuilt** — the core of the OS that
  talks to hardware. The `-bin` version is **already compiled**, saving you *hours*
  vs. building it yourself. (This is the big time-saver.)
- `linux-firmware` = tiny driver blobs so Wi-Fi, GPU, etc. work.

---

### A7. Fstab, hostname, password

```bash
cat > /etc/fstab <<EOF
/dev/sda2  /     ext4  defaults,noatime  0 1
/dev/sda1  /efi  vfat  defaults          0 2
EOF
```
- `cat > file <<EOF ... EOF` = write everything between the `EOF` markers into a file.
- **fstab** = the list of "which partition mounts where at boot". Line 1: main slice
  → `/`. Line 2: boot slice → `/efi`. **Why:** so the OS auto-mounts its disk on startup.

```bash
echo "interstellar" > /etc/hostname
```
- Names the computer "interstellar."

```bash
passwd
```
- Sets the **root password**. Type one you'll remember (you'll see nothing as you
  type — that's normal). **Why:** you need it to log in after reboot.

---

### A8. Bootloader (the boot menu)

```bash
emerge sys-boot/grub
```
- Installs **GRUB** — the little menu that loads the OS when the machine powers on.

```bash
grub-install --target=x86_64-efi --efi-directory=/efi --removable
```
- Writes GRUB's boot files onto the EFI (boot) partition. `--removable` = install in
  the universal spot so the VM finds it reliably. **Why:** without this, the PC boots
  to nothing.

```bash
grub-mkconfig -o /boot/grub/grub.cfg
```
- Auto-generates GRUB's config by **detecting your installed kernel** and creating a
  menu entry for it. **Why:** so GRUB knows what to actually boot.

---

### A9. Leave and reboot

```bash
exit
```
- Step **out** of the chroot, back to the live installer environment.

```bash
umount -R /mnt/gentoo
```
- `umount -R` = detach everything we mounted under `/mnt/gentoo` (the reverse of
  mounting). **Why:** cleanly release the disk before rebooting so nothing corrupts.

```bash
reboot
```
- Restart. **Before it boots back up**, in VirtualBox remove the ISO (Devices →
  Optical Drives → Remove disk from virtual drive) so it boots your new *disk*, not
  the installer again. Log in as `root` with the password you set.

✅ **You now have Gentoo. That's the hard part done.**

---

## Milestone B — become Interstellar, explained

```bash
emerge dev-vcs/git
```
- Installs **git** so you can download our project.

```bash
git clone https://github.com/Blehh13/osaima.git /root/osaima
cd /root/osaima
```
- `git clone <url> <folder>` = download our whole repository into `/root/osaima`.
- `cd` into it.

```bash
cp -a gentoo/overlay /var/db/repos/interstellar
```
- Copy our **overlay** (the fork's package/branding files) into the place Portage
  looks for extra repositories. `-a` = copy exactly, keeping everything.
- **Why:** this is literally installing "our layer on top of Gentoo."

```bash
mkdir -p /etc/portage/repos.conf
cp gentoo/config/repos.conf/interstellar.conf /etc/portage/repos.conf/
nano /etc/portage/repos.conf/interstellar.conf
```
- Registers our overlay with Portage (tells it "this repo exists, here's where").
- In `nano`, make sure it says `location = /var/db/repos/interstellar` and
  `auto-sync = no` (since we copied it by hand). Save (Ctrl+O) and exit (Ctrl+X).

```bash
eselect profile list
eselect profile set interstellar:interstellar/base
```
- Now the list **includes our profiles**. `set interstellar:interstellar/base`
  switches the whole system's defaults to **ours**.
- **Why:** this is the switch from "generic Gentoo" to "Interstellar."

```bash
emerge app-misc/interstellar-release
```
- Installs our **branding package** — it writes the `/etc/os-release` file that
  names the OS.

```bash
cat /etc/os-release
```
- `cat` = print a file to the screen. This shows the identity file. You'll see
  **`NAME="Interstellar OS"`**.

🏆 **That output is the proof. You forked Linux into Interstellar OS.** Screenshot it.

---

## The mental model (one picture)

```
Empty VM disk
   → cut into 2 partitions (boot + main)       [A1]
   → format them so they hold files            [A1]
   → mount main slice as /mnt/gentoo           [A1]
   → unpack stage3 seed system into it         [A2]
   → chroot INTO it (pretend we booted)         [A4]
   → add kernel + bootloader + settings         [A5–A8]
   → reboot → real Gentoo                        [A9]
   → drop OUR overlay + profile on top           [B]
   → system now says "Interstellar OS"           [B] 🏆
```

Stuck on any single command? Paste the exact screen/error to me and I'll explain
what went wrong and the next move.
