#!/usr/bin/env bash
#
# run_vm.sh — boot the image from build_vm_image.sh under QEMU/KVM.
#
#   build-tools/scripts/run_vm.sh            # window (GTK) for the desktop
#   HEADLESS=1 build-tools/scripts/run_vm.sh # no window; serial log + QMP for screenshots
#   ISO=1 build-tools/scripts/run_vm.sh      # boot out/interstellar-os.iso instead of the disk image
#   RES=1920x1080 build-tools/scripts/run_vm.sh  # guest screen size (default 1600x900)
#
# Port forwards: ssh root@localhost -p 2222 -i out/vm/id_osaima
#                http://localhost:8778 → the OSAIMA shell inside the VM
set -euo pipefail

OUT="${OUT:-$(realpath "$(dirname "$0")/../..")/out/vm}"
MEM="${MEM:-6G}"
CPUS="${CPUS:-$(( $(nproc) / 2 > 2 ? $(nproc) / 2 : 2 ))}"
# Guest screen size: advertised to the guest via EDID, so sway picks it.
RES="${RES:-1600x900}"

[ -w /dev/kvm ] || { echo "/dev/kvm not usable: enable virtualisation in BIOS / join the kvm group" >&2; exit 1; }

if [ -n "${HEADLESS:-}" ]; then
	display=(-display none -serial "file:$OUT/serial.log" -qmp "unix:$OUT/qmp.sock,server,nowait")
else
	display=(-display gtk,zoom-to-fit=on -serial "file:$OUT/serial.log")
fi

if [ -n "${ISO:-}" ]; then
	# boot the live ISO from build_iso.sh, as a real machine would from USB/DVD
	boot=(-cdrom "${ISO_PATH:-$(dirname "$OUT")/interstellar-os.iso}" -boot d)
else
	boot=(-kernel "$OUT/vmlinuz" -initrd "$OUT/initramfs.img"
	      -append "root=/dev/vda rw console=tty1 console=ttyS0 quiet"
	      -drive "file=$OUT/interstellar.qcow2,if=virtio,format=qcow2")
fi

exec qemu-system-x86_64 \
	-enable-kvm -cpu host -m "$MEM" -smp "$CPUS" \
	"${boot[@]}" \
	-device "virtio-vga,edid=on,xres=${RES%x*},yres=${RES#*x}" \
	-nic user,model=virtio-net-pci,hostfwd=tcp:127.0.0.1:2222-:22,hostfwd=tcp:127.0.0.1:8778-:8777 \
	"${display[@]}"
