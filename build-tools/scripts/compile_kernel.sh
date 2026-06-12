#!/bin/bash

# Abort on error
set -e

KERNEL_DIR="/osaima-build/kernel/linux"
OUT_DIR="/osaima-build/out/kernel"

echo "==> Setting up kernel compilation..."
if [ ! -d "$KERNEL_DIR" ]; then
    echo "Error: Kernel directory $KERNEL_DIR not found."
    echo "Did you run 'git submodule update --init --recursive'?"
    exit 1
fi

mkdir -p "$OUT_DIR"

cd "$KERNEL_DIR"

# Generate default config
echo "==> Generating default kernel config..."
make O="$OUT_DIR" defconfig

# Compile the kernel
# We use $(nproc) to compile using all available CPU cores
CORES=$(nproc)
echo "==> Compiling kernel using $CORES cores..."
make O="$OUT_DIR" -j"$CORES" bzImage

echo "==> Kernel compiled successfully! Output is in $OUT_DIR/arch/x86/boot/bzImage"
