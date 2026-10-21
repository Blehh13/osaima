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
		sh -c 'echo \$\$ > '\$cg'/high/cgroup.procs; exec python3 /tmp/latprobe.py 4' | sed 's|^|$label: |'
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

section "C eBPF toolchain (clang + libbpf + bpftool): can a CO-RE program be built and loaded?"
fence
sudo apt-get install -y -qq clang llvm libbpf-dev linux-libc-dev libelf-dev zlib1g-dev > /tmp/apt.log 2>&1 || tail -5 /tmp/apt.log
clang --version | head -1
bpftool --version 2>&1 | head -2
mkdir -p /tmp/bpfprobe && cd /tmp/bpfprobe
bpftool btf dump file /sys/kernel/btf/vmlinux format c > vmlinux.h && echo "vmlinux.h: $(wc -l < vmlinux.h) lines"
cat > probe.bpf.c <<'EOF'
#include "vmlinux.h"
#include <bpf/bpf_helpers.h>
#include <bpf/bpf_tracing.h>

struct { __uint(type, BPF_MAP_TYPE_ARRAY); __uint(max_entries, 1); __type(key, __u32); __type(value, __u64); } count SEC(".maps");

SEC("tp_btf/sched_switch")
int BPF_PROG(on_switch, bool preempt, struct task_struct *prev, struct task_struct *next)
{
	__u32 k = 0;
	__u64 *v = bpf_map_lookup_elem(&count, &k);
	if (v) __sync_fetch_and_add(v, 1);
	return 0;
}
char LICENSE[] SEC("license") = "GPL";
EOF
if clang -g -O2 -target bpf -D__TARGET_ARCH_x86 -c probe.bpf.c -o probe.bpf.o 2> clang.log; then
	echo "compiled probe.bpf.o ($(stat -c %s probe.bpf.o) bytes)"
	sudo mkdir -p /sys/fs/bpf/osaima-probe/maps
	sudo bpftool prog loadall probe.bpf.o /sys/fs/bpf/osaima-probe/progs autoattach pinmaps /sys/fs/bpf/osaima-probe/maps 2>&1 | head -5
	echo "pinned:"; sudo find /sys/fs/bpf/osaima-probe | head
	sleep 1
	echo "map dump (json):"
	sudo bpftool -j map dump pinned /sys/fs/bpf/osaima-probe/maps/count 2>&1 | head -c 600; echo
	echo "after the loader has exited, is the program still attached?"
	sudo bpftool -j prog show 2>/dev/null | python3 -c "import sys,json; print([p['name'] for p in json.load(sys.stdin) if p.get('name') in ('on_switch',)])"
	sudo bpftool link show 2>&1 | head -4
	sudo rm -rf /sys/fs/bpf/osaima-probe && echo "unpinned"
	sleep 1
	sudo bpftool -j prog show 2>/dev/null | python3 -c "import sys,json; print('still loaded:', [p['name'] for p in json.load(sys.stdin) if p.get('name') in ('on_switch',)])"
else
	echo "BPF C compile failed"; grep -E "error" clang.log | head -5
fi
fence

section "sched_ext: what does this kernel expose?"
fence
ls /sys/kernel/sched_ext/ 2>&1
cat /sys/kernel/sched_ext/state 2>&1
grep -c scx_bpf /sys/kernel/btf/vmlinux 2>/dev/null || true
bpftool btf dump file /sys/kernel/btf/vmlinux format raw 2>/dev/null | grep -E "FUNC 'scx_bpf_(dsq_insert|dispatch|dsq_move_to_local|consume)'" | head -6
fence
