#!/usr/bin/env python3
"""A frame-deadline benchmark: does a 60 fps style task keep its deadlines?

The task needs `--work-ms` of CPU time every `1/fps` seconds (6 ms every 16.7 ms by
default: about 36% of one core). On an idle machine that is easy. When many programs
want the CPU, a task that needs a steady share gets only its fair share, which can be
less than it needs, and frames arrive late. A frame is late when it is finished more
than one period after it was due to start.

Prints one JSON line with the frame-time percentiles (milliseconds, from the moment the
frame was due to its completion) and the share of late frames.
"""

import argparse
import json
import statistics
import sys
import time


def busy(ms: float) -> None:
    end = time.thread_time_ns() + int(ms * 1e6)
    while time.thread_time_ns() < end:
        pass


def percentile(values: list[float], p: float) -> float:
    ordered = sorted(values)
    return ordered[min(len(ordered) - 1, int(len(ordered) * p))]


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--fps", type=float, default=60.0)
    parser.add_argument("--work-ms", type=float, default=6.0)
    parser.add_argument("--warmup", type=float, default=10.0, help="seconds before measuring (the scheduler needs a few windows)")
    parser.add_argument("--seconds", type=float, default=25.0, help="seconds measured")
    parser.add_argument("--label", default="")
    args = parser.parse_args()

    period = 1.0 / args.fps
    start = time.monotonic()
    measure_from = start + args.warmup
    end = measure_from + args.seconds
    due = start
    frame_ms: list[float] = []
    late = 0

    while due < end:
        wait = due - time.monotonic()
        if wait > 0:
            time.sleep(wait)
        busy(args.work_ms)
        took = time.monotonic() - due
        if due >= measure_from:
            frame_ms.append(took * 1000)
            if took > period:
                late += 1
        due += period
        behind = time.monotonic() - due
        if behind > 0:  # a frame was dropped: carry on from now rather than rushing a burst
            due = time.monotonic()

    if not frame_ms:
        print("no frames were measured", file=sys.stderr)
        return 1
    result = {
        "label": args.label,
        "frames": len(frame_ms),
        "p50_ms": round(statistics.median(frame_ms), 2),
        "p95_ms": round(percentile(frame_ms, 0.95), 2),
        "p99_ms": round(percentile(frame_ms, 0.99), 2),
        "max_ms": round(max(frame_ms), 2),
        "late_percent": round(100.0 * late / len(frame_ms), 1),
        "period_ms": round(period * 1000, 2),
    }
    print(json.dumps(result))
    return 0


if __name__ == "__main__":
    sys.exit(main())
