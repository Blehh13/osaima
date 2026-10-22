#!/usr/bin/env bash
#
# build_iso.sh — bootable live ISO of Interstellar OS from the rootfs that
# build_vm_image.sh assembles (run that first; SKIP_DISK=1 skips its qcow2).
#
#   sudo build-tools/scripts/build_iso.sh
#   ISO=1 build-tools/scripts/run_vm.sh       # boot it in QEMU
#
# Layout: squashfs root + dracut dmsquash-live initramfs (writes go to a RAM
# overlay), GRUB for BIOS and UEFI. Output: out/interstellar-os.iso
#
#   LITE=1  also write out/interstellar-os-lite.iso: the same system without the LLM
#           weights (about 2.4 GB smaller); it downloads them on first start
#           (ai-core/llm/start-llm.sh), so it needs the internet once.
#
# Host tools: squashfs-tools xorriso mtools grub-pc-bin grub-efi-amd64-bin

set -euo pipefail

REPO_ROOT="$(realpath "$(dirname "$0")/../..")"
WORK="${WORK:-/var/tmp/osaima-vm}"
ROOTFS="$WORK/rootfs"
ISO_DIR="$WORK/iso"
OUT="${OUT:-$REPO_ROOT/out}"
VOLID="INTERSTELLAR"
ISO="$OUT/interstellar-os.iso"
LITE_ISO="$OUT/interstellar-os-lite.iso"

log() { echo -e "\n==> $(date +%H:%M:%S) $*"; }
[ "$(id -u)" = 0 ] || { echo "Run as root (sudo)." >&2; exit 1; }
[ -e "$ROOTFS/etc/os-release" ] || { echo "No rootfs at $ROOTFS: run build_vm_image.sh first." >&2; exit 1; }
for t in mksquashfs grub-mkrescue xorriso; do command -v "$t" >/dev/null || { echo "missing host tool: $t" >&2; exit 1; }; done

# The host's /dev, /proc, /sys and /run are mounted into the chroot as SLAVES:
# unmounting a shared copy would also unmount the host's own (and take the
# user's desktop session down with /run/user). Mount in one step so there is
# no shared moment, never stack a second copy, and refuse to unmount anything
# that is not a slave.
mount_host() {
	for m in proc sys dev run; do
		mountpoint -q "$ROOTFS/$m" || mount --rbind --make-rslave "/$m" "$ROOTFS/$m"
	done
}
cleanup() {
	for m in run proc sys dev; do
		mountpoint -q "$ROOTFS/$m" || continue
		if findmnt -rn -o PROPAGATION "$ROOTFS/$m" | grep -q shared; then
			echo "NOT unmounting $ROOTFS/$m: it is shared with the host; reboot or unmount by hand" >&2
			continue
		fi
		umount -R -l "$ROOTFS/$m" 2>/dev/null || true
	done
}
trap cleanup EXIT

kver="$(ls "$ROOTFS/lib/modules" | sort -V | tail -1)"

log "Live initramfs (dracut dmsquash-live) for kernel $kver"
mount_host
chroot "$ROOTFS" /bin/bash -e <<CHROOT
	# dmsquash-live needs device-mapper (dmsetup) in the initramfs
	command -v dmsetup >/dev/null || { source /etc/profile 2>/dev/null; emerge --quiet --getbinpkg --noreplace sys-fs/lvm2; }
	dracut --force --no-hostonly --no-hostonly-cmdline --quiet \
		--add "dmsquash-live" \
		--add-drivers "squashfs loop overlay iso9660 sr_mod cdrom virtio_blk virtio_scsi virtio_pci ahci usb_storage uas" \
		/boot/initramfs-live.img "$kver"
CHROOT
cleanup

log "Laying out the ISO tree"
rm -rf "$ISO_DIR"
install -d "$ISO_DIR/boot/grub" "$ISO_DIR/LiveOS" "$OUT"
cp "$(ls "$ROOTFS"/boot/{kernel,vmlinuz}-"$kver" 2>/dev/null | head -1)" "$ISO_DIR/boot/vmlinuz"
mv "$ROOTFS/boot/initramfs-live.img" "$ISO_DIR/boot/initramfs.img"

# LLM weights go on the ISO as plain files, not inside the xz squashfs: they
# barely compress, and decompressing them on every read made the model take
# >4 minutes to load at boot. The live system reads them from
# /run/initramfs/live/osaima/models (see ai-core/llm/start-llm.sh).
install -d "$ISO_DIR/osaima/models"
cp "$ROOTFS"/usr/share/osaima/models/*.gguf "$ISO_DIR/osaima/models/"

log "Squashing the root filesystem (xz; the long step)"
# The live system boots from this read-only image; the Gentoo package tree,
# download caches and build leftovers are not needed to run it.
mksquashfs "$ROOTFS" "$ISO_DIR/LiveOS/squashfs.img" -comp xz -b 1M -noappend -no-progress \
	-e var/db/repos/gentoo var/cache/distfiles var/cache/binpkgs var/tmp/portage \
	   proc sys dev run tmp root/.cache usr/share/osaima/models
# The live root has no fstab entry: dracut mounts it from the squashfs.
unsquashfs -s "$ISO_DIR/LiveOS/squashfs.img" | head -3

cat > "$ISO_DIR/boot/grub/grub.cfg" <<GRUB
set timeout=5
set default=0
insmod all_video
menuentry "Interstellar OS (OSAIMA) — live" {
    linux /boot/vmlinuz root=live:CDLABEL=$VOLID rd.live.image rd.live.overlay.overlayfs=1 console=tty1 console=ttyS0 quiet
    initrd /boot/initramfs.img
}
menuentry "Interstellar OS (OSAIMA) — live, copy to RAM" {
    linux /boot/vmlinuz root=live:CDLABEL=$VOLID rd.live.image rd.live.overlay.overlayfs=1 rd.live.ram=1 console=tty1 console=ttyS0 quiet
    initrd /boot/initramfs.img
}
GRUB

log "Building the ISO (BIOS + UEFI)"
grub-mkrescue -o "$ISO" "$ISO_DIR" -- -volid "$VOLID" 2>&1 | tail -2
chown "${SUDO_USER:-root}" "$ISO"

if [ -n "${LITE:-}" ]; then
	log "Building the lite ISO (no LLM weights; downloaded on first start)"
	rm -rf "$ISO_DIR/osaima/models"
	grub-mkrescue -o "$LITE_ISO" "$ISO_DIR" -- -volid "$VOLID" 2>&1 | tail -2
	chown "${SUDO_USER:-root}" "$LITE_ISO"
fi
log "Done"
ls -lh "$ISO" ${LITE:+"$LITE_ISO"}
