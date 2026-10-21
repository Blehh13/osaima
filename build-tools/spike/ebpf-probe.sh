#!/usr/bin/env bash
#
# eBPF scheduler spike: what can a CI runner's kernel do?
#
# Answers, with measurements, whether the scheduler work (eBPF telemetry, ML
# classification, cgroup actuation) can be built and tested on a free GitHub
# runner. Run as a normal user with sudo; prints a report.

set -uo pipefail

section() { printf '\n## %s\n\n' "$*"; }
fence() { printf '```\n'; }

section "Kernel"
fence
uname -srm
nproc
echo "BTF (needed for CO-RE programs): $([ -e /sys/kernel/btf/vmlinux ] && echo yes || echo no)"
for opt in CONFIG_BPF CONFIG_BPF_SYSCALL CONFIG_BPF_JIT CONFIG_DEBUG_INFO_BTF CONFIG_TRACEPOINTS \
	CONFIG_CGROUPS CONFIG_CGROUP_SCHED CONFIG_FAIR_GROUP_SCHED CONFIG_CGROUP_BPF CONFIG_SCHED_CLASS_EXT; do
	printf '%-26s %s\n' "$opt" "$(grep -h "^$opt=" "/boot/config-$(uname -r)" 2> /dev/null || echo 'not set or unknown')"
done
echo "sched_ext: $([ -d /sys/kernel/sched_ext ] && echo present || echo absent)"
echo "unprivileged BPF disabled: $(cat /proc/sys/kernel/unprivileged_bpf_disabled 2> /dev/null)"
fence

section "cgroup v2 and CPU weights (what the policy will actuate)"
fence
mount | grep cgroup2 || echo "no cgroup2 mount"
cat /sys/fs/cgroup/cgroup.controllers 2> /dev/null
sudo sh -c '
	set -e
	cg=/sys/fs/cgroup/osaima-probe
	echo +cpu > /sys/fs/cgroup/cgroup.subtree_control 2>/dev/null || true
	mkdir -p $cg/high $cg/low
	echo +cpu > $cg/cgroup.subtree_control
	echo 800 > $cg/high/cpu.weight
	echo 20  > $cg/low/cpu.weight
	echo "high: weight $(cat $cg/high/cpu.weight)   low: weight $(cat $cg/low/cpu.weight)"
	rmdir $cg/high $cg/low $cg
' 2>&1 || echo "could not create cgroups with weights"
fence

section "Do CPU weights change a real latency? (a 4-second test)"
# One thread that wakes every 2 ms and measures how late it ran, against hogs on
# every CPU. Run in two cgroups: equal weights, then weights 800 versus 20.
cat > /tmp/latprobe.py <<'EOF'
import sys, time, statistics
late = []
end = time.perf_counter() + float(sys.argv[1])
while time.perf_counter() < end:
    want = time.perf_counter() + 0.002
    time.sleep(0.002)
    late.append((time.perf_counter() - want) * 1000)
late.sort()
print(f"wakeups {len(late)}  p50 {late[len(late)//2]:.2f} ms  p99 {late[int(len(late)*0.99)]:.2f} ms  max {late[-1]:.2f} ms")
EOF
cat > /tmp/hog.py <<'EOF'
import sys, time
end = time.perf_counter() + float(sys.argv[1])
while time.perf_counter() < end:
    pass
EOF
fence
run_case() {
	local label=$1 hi=$2 lo=$3
	sudo sh -c "
		cg=/sys/fs/cgroup/osaima-probe
		mkdir -p \$cg/high \$cg/low
		echo +cpu > \$cg/cgroup.subtree_control
		echo $hi > \$cg/high/cpu.weight
		echo $lo > \$cg/low/cpu.weight
		for i in \$(seq 1 \$(( $(nproc) * 2 ))); do
			sh -c 'echo \$\$ > '\$cg'/low/cgroup.procs; exec python3 /tmp/hog.py 5' &
		done
		sleep 0.5
		sh -c 'echo \$\$ > '\$cg'/high/cgroup.procs; exec python3 /tmp/latprobe.py 4' | sed 's/^/$label: /'
		wait
		rmdir \$cg/high \$cg/low \$cg 2>/dev/null || true
	" 2>&1
}
run_case "equal weights (100/100)" 100 100
run_case "interactive favoured (800/20)" 800 20
fence

section "bpftrace: can a tracepoint program load and read scheduler events?"
sudo apt-get install -y -qq bpftrace > /dev/null 2>&1 || sudo apt-get install -y bpftrace 2>&1 | tail -2
fence
if command -v bpftrace > /dev/null; then
	bpftrace --version
	sudo timeout 20 bpftrace -e '
		tracepoint:sched:sched_switch { @switches = count(); @by[args->prev_comm] = count(); }
		interval:s:2 { print(@switches); exit(); }' 2>&1 | head -8
else
	echo "bpftrace not installed"
fi
fence

section "Rust eBPF toolchain (aya): does it install, and how long does it take?"
fence
start=$(date +%s)
if rustup toolchain install nightly --profile minimal --component rust-src > /tmp/nightly.log 2>&1; then
	echo "nightly with rust-src: ok ($(( $(date +%s) - start )) s)"
else
	echo "nightly install failed"; tail -5 /tmp/nightly.log
fi
start=$(date +%s)
sudo apt-get install -y -qq llvm clang libclang-dev > /dev/null 2>&1 || true
if cargo install bpf-linker --locked > /tmp/bpf-linker.log 2>&1; then
	echo "bpf-linker: ok ($(( $(date +%s) - start )) s)"
else
	echo "bpf-linker install failed"; tail -8 /tmp/bpf-linker.log
fi
fence
