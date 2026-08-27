#!/usr/bin/env bash
#
# build_live_iso.sh — build a bootable LIVE ISO from a running Interstellar OS
# install. Produces /root/interstellar-os.iso which boots the system from CD/USB
# using a squashfs root + dracut dmsquash-live overlay.
#
# Run as root ON the installed Interstellar OS (not on the build host). Needs a
# few GB of free space. This is the advanced "Milestone D" step.

set -euo pipefail

KVER="$(uname -r)"
WORK="/root/iso-build"
SQUASH="/root/squash"
ISO="/root/interstellar-os.iso"
VOLID="INTERSTELLAR"

log() { echo -e "\n==> $*"; }

log "Installing ISO tools"
emerge -qv sys-fs/squashfs-tools dev-libs/libisoburn sys-fs/mtools net-misc/rsync

log "Preparing work directories"
rm -rf "$WORK" "$SQUASH"
mkdir -p "$WORK"/boot/grub "$WORK"/LiveOS "$SQUASH"/LiveOS /mnt/rootfs

log "Creating ext4 root image (6 GiB)"
dd if=/dev/zero of="$SQUASH"/LiveOS/rootfs.img bs=1M count=6144 status=progress
mkfs.ext4 -F -L "${VOLID}_ROOT" "$SQUASH"/LiveOS/rootfs.img
mount -o loop "$SQUASH"/LiveOS/rootfs.img /mnt/rootfs

log "Copying the live system into the image"
rsync -aHAX --info=progress2 \
	--exclude='/proc/*' --exclude='/sys/*' --exclude='/dev/*' --exclude='/run/*' \
	--exclude='/tmp/*' --exclude='/mnt/*' --exclude='/media/*' \
	--exclude="$WORK" --exclude="$SQUASH" --exclude="$ISO" \
	--exclude='/var/cache/distfiles/*' --exclude='/var/tmp/*' \
	--exclude='/var/cache/binpkgs/*' \
	/ /mnt/rootfs/

# The live root is provided by dracut, so the installed-disk fstab must not apply.
: > /mnt/rootfs/etc/fstab
umount /mnt/rootfs

log "Squashing the root image (xz)"
mksquashfs "$SQUASH" "$WORK"/LiveOS/squashfs.img -comp xz -noappend

log "Building the live initramfs"
dracut --force --no-hostonly --nolvmconf --nomdadmconf \
	--add dmsquash-live \
	--add-drivers "squashfs loop overlay iso9660" \
	"$WORK"/boot/initramfs-live.img "$KVER"
cp "/boot/vmlinuz-$KVER" "$WORK"/boot/vmlinuz

log "Writing the GRUB menu"
cat > "$WORK"/boot/grub/grub.cfg <<GRUB
set timeout=10
set default=0
menuentry "Interstellar OS (OSAIMA) — Live" {
    linux /boot/vmlinuz root=live:CDLABEL=${VOLID} rd.live.image quiet
    initrd /boot/initramfs-live.img
}
GRUB

log "Building the ISO"
grub-mkrescue -o "$ISO" "$WORK" -- -volid "$VOLID"

log "Done"
ls -lh "$ISO"
