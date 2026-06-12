#!/bin/bash

# Abort on error
set -e

BZIMAGE="/osaima-build/out/kernel/arch/x86/boot/bzImage"
ISO_DIR="/osaima-build/out/isodir"
OUTPUT_ISO="/osaima-build/out/interstellar-os-v0.1.iso"
INITRAMFS="/osaima-build/out/initramfs.cpio.gz"

if [ ! -f "$BZIMAGE" ]; then
    echo "Error: bzImage not found at $BZIMAGE. Please run compile_kernel.sh first."
    exit 1
fi

echo "==> Creating minimal initramfs..."
mkdir -p /osaima-build/out/initramfs
cd /osaima-build/out/initramfs
mkdir -p bin dev etc lib proc sys tmp
# Copy static busybox
cp /bin/busybox bin/
# Create symlinks for common commands
for cmd in $(./bin/busybox --list); do
    ln -s busybox bin/$cmd
done

# Create a simple init script with installer support
# Copy installer files into the initramfs
mkdir -p installer
cp -r /osaima-build/installer/* installer/
chmod +x installer/init-install.sh

cat << 'EOF' > init
#!/bin/sh
mount -t proc none /proc
mount -t sysfs none /sys
mount -t devtmpfs none /dev

# If booted with "install" flag, launch the installer
if grep -q "\binstall\b" /proc/cmdline; then
    echo "Launching Interstellar OS installer..."
    exec /installer/init-install.sh
fi

echo "======================================="
echo " Welcome to Interstellar OS (Phase 1) "
echo "======================================="
echo "Kernel successfully booted!"
echo "Dropping to a minimal shell."

exec /bin/sh
EOF

chmod +x init

# Copy installer into initramfs (so it runs when booted with "install")
mkdir -p installer
cp -r /osaima-build/installer/* installer/
chmod +x installer/init-install.sh

# Pack initramfs
find . -print0 | cpio --null -ov --format=newc | gzip -9 > "$INITRAMFS"

echo "==> Preparing ISO directory..."
mkdir -p "$ISO_DIR/boot/grub"
cp "$BZIMAGE" "$ISO_DIR/boot/vmlinuz"
# Copy OS payload (kernel, UI, ai-core, gentoo) into ISO for installer
mkdir -p "$ISO_DIR/install_root"
cp -a /osaima-build/kernel "$ISO_DIR/install_root/"
cp -a /osaima-build/ai-core "$ISO_DIR/install_root/"
cp -a /osaima-build/gentoo "$ISO_DIR/install_root/"
cp -a /osaima-build/ui/osaima-shell/target/release/osaima-shell "$ISO_DIR/install_root/osaima-shell"

# Copy installer configuration into ISO
cp -r /installer "$ISO_DIR/installer"


# Create GRUB configuration
cat << 'EOF' > "$ISO_DIR/boot/grub/grub.cfg"
set timeout=5
set default=0

menuentry "Interstellar OS (Phase 1)" {
    linux /boot/vmlinuz
    initrd /boot/initrd.img
}
EOF

echo "==> Generating ISO..."
grub-mkrescue -o "$OUTPUT_ISO" "$ISO_DIR"

echo "==> ISO generated successfully! Output is at $OUTPUT_ISO"
