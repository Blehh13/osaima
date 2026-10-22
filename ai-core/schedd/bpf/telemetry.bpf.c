// SPDX-License-Identifier: GPL-2.0
/*
 * Scheduler telemetry for osaima-schedd.
 *
 * Hooks the kernel's scheduler tracepoints and keeps, for every thread, how long it
 * ran, how long it waited for a CPU after becoming runnable, how many times it was
 * given the CPU, and whether it gave the CPU up by itself (blocked) or had it taken
 * away (preempted). The numbers are cumulative; the daemon reads the map every few
 * seconds and works on the differences.
 *
 * Built against the kernel's BTF (CO-RE), so one object runs on any kernel that
 * exposes /sys/kernel/btf/vmlinux. See the Makefile.
 */
#include "vmlinux.h"
#include <bpf/bpf_core_read.h>
#include <bpf/bpf_helpers.h>
#include <bpf/bpf_tracing.h>

#define TASK_RUNNING 0
#define MAX_TASKS 16384

struct stats {
	__u64 run_ns;          /* total time on a CPU */
	__u64 wait_ns;         /* total time runnable but waiting for a CPU */
	__u64 slices;          /* times it was switched in */
	__u64 voluntary;       /* switched out while blocked: it gave the CPU up */
	__u64 involuntary;     /* switched out while still runnable: preempted */
	__u64 wake_lat_max_ns; /* the longest single wait for a CPU */
	__u64 run_start;       /* when it was last switched in (0 while off the CPU) */
	__u64 runnable_since;  /* when it last became runnable (0 while running) */
	__u32 tgid;            /* the process this thread belongs to */
	char comm[16];
};

/* Least-recently-used, so a busy machine cannot fill it: old threads are dropped. */
struct {
	__uint(type, BPF_MAP_TYPE_LRU_HASH);
	__uint(max_entries, MAX_TASKS);
	__type(key, __u32); /* the thread id */
	__type(value, struct stats);
} tasks SEC(".maps");

static __always_inline struct stats *get_stats(struct task_struct *t)
{
	__u32 tid = BPF_CORE_READ(t, pid);
	struct stats *s = bpf_map_lookup_elem(&tasks, &tid);

	if (s)
		return s;

	struct stats fresh = {};

	fresh.tgid = BPF_CORE_READ(t, tgid);
	bpf_core_read_str(&fresh.comm, sizeof(fresh.comm), &t->comm);
	bpf_map_update_elem(&tasks, &tid, &fresh, BPF_NOEXIST);
	return bpf_map_lookup_elem(&tasks, &tid);
}

/* A thread became runnable: from now on it is waiting for a CPU. */
SEC("tp_btf/sched_wakeup")
int BPF_PROG(on_wakeup, struct task_struct *p)
{
	struct stats *s = get_stats(p);

	if (s)
		s->runnable_since = bpf_ktime_get_ns();
	return 0;
}

SEC("tp_btf/sched_wakeup_new")
int BPF_PROG(on_wakeup_new, struct task_struct *p)
{
	struct stats *s = get_stats(p);

	if (s)
		s->runnable_since = bpf_ktime_get_ns();
	return 0;
}

SEC("tp_btf/sched_switch")
int BPF_PROG(on_switch, bool preempt, struct task_struct *prev, struct task_struct *next)
{
	__u64 now = bpf_ktime_get_ns();

	/* pid 0 is the idle task: not interesting */
	if (BPF_CORE_READ(prev, pid) != 0) {
		struct stats *p = get_stats(prev);

		if (p) {
			if (p->run_start) {
				p->run_ns += now - p->run_start;
				p->run_start = 0;
			}
			if (BPF_CORE_READ(prev, __state) == TASK_RUNNING) {
				/* still wants the CPU: it was preempted and is back in the queue */
				p->involuntary++;
				p->runnable_since = now;
			} else {
				p->voluntary++;
			}
		}
	}

	if (BPF_CORE_READ(next, pid) != 0) {
		struct stats *n = get_stats(next);

		if (n) {
			if (n->runnable_since) {
				__u64 waited = now - n->runnable_since;

				n->wait_ns += waited;
				if (waited > n->wake_lat_max_ns)
					n->wake_lat_max_ns = waited;
				n->runnable_since = 0;
			}
			n->slices++;
			n->run_start = now;
		}
	}
	return 0;
}

char LICENSE[] SEC("license") = "GPL";
