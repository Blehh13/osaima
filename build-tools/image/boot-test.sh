#!/usr/bin/env bash
#
# Boot the ISO headless in QEMU and wait for the login prompt on the serial
# console. Runs on the CI host, not in the container.
#
#   boot-test.sh path/to/interstellar-os.iso [log] [timeout-seconds]

set -uo pipefail

ISO=${1:?usage: boot-test.sh ISO [log] [timeout]}
LOG=${2:-serial.log}
TIMEOUT=${3:-600}
EXPECT=${EXPECT:-login:}

if [ -e /dev/kvm ]; then
	sudo chmod a+rw /dev/kvm || true
fi
accel=kvm:tcg   # hardware acceleration when the runner has it, else software
[ -w /dev/kvm ] && echo "KVM is available" || echo "no KVM: booting with software emulation (slow)"

: > "$LOG"
qemu-system-x86_64 -machine "accel=$accel" -cpu max -m 4096 -smp 2 \
	-cdrom "$ISO" -boot d -display none -no-reboot \
	-serial "file:$LOG" -netdev user,id=n0 -device virtio-net-pci,netdev=n0 &
QEMU=$!

start=$(date +%s)
result=timeout
while [ $(( $(date +%s) - start )) -lt "$TIMEOUT" ]; do
	if grep -q "$EXPECT" "$LOG"; then result=ok; break; fi
	if ! kill -0 "$QEMU" 2>/dev/null; then result=exited; break; fi
	sleep 5
done
elapsed=$(( $(date +%s) - start ))
kill "$QEMU" 2>/dev/null || true
wait "$QEMU" 2>/dev/null || true

echo "result: $result after ${elapsed}s"
echo "---- last 40 lines of the console ----"
tail -40 "$LOG"
[ "$result" = ok ]
