#!/usr/bin/env bash
set -e

# Simple installer for Interstellar OS (run from initramfs)
# Usage: ./install.sh [target-disk]
# Default target disk is /dev/sda

TARGET_DISK="${1:-/dev/sda}"

echo "[Installer] Installing Interstellar OS to $TARGET_DISK ..."

# Ensure required tools exist
for cmd in sgdisk mkfs.vfat mkfs.ext4 rsync grub-install; do
  if ! command -v $cmd >/dev/null 2>&1; then
    echo "[Installer] Error: required command $cmd not found in live environment." >&2
    exit 1
  fi
done

# Wipe existing partition table
sgdisk -Z "$TARGET_DISK"
# Create new GPT with 2048 sector alignment
sgdisk -a 2048 -o "$TARGET_DISK"
# EFI partition (first, 512M)
sgdisk -n 1:0:+512M -t 1:EF00 "$TARGET_DISK"
# Root partition (rest of space)
sgdisk -n 2:0:0 -t 2:8300 "$TARGET_DISK"

# Format partitions
mkfs.vfat -F32 "${TARGET_DISK}1"
mkfs.ext4 "${TARGET_DISK}2"

# Mount root and EFI partitions
mount "${TARGET_DISK}2" /mnt
mkdir -p /mnt/boot/efi
mount "${TARGET_DISK}1" /mnt/boot/efi

# Copy OS payload (mounted at /install_root during initramfs)
rsync -a /install_root/ /mnt/

# Install GRUB for EFI
grub-install --target=x86_64-efi \
  --efi-directory=/mnt/boot/efi \
  --boot-directory=/mnt/boot \
  --removable --recheck

# Write minimal GRUB configuration
cat > /mnt/boot/grub/grub.cfg <<'GRUBCFG'
set timeout=5
menuentry "Interstellar OS" {
    linux /boot/vmlinuz
    initrd /boot/initrd.img
}
GRUBCFG

# Unmount and finish
umount -R /mnt

echo "[Installer] Installation complete. Rebooting now..."
reboot
