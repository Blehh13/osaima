#!/usr/bin/env python3
"""Collect labelled training data: how programs of each kind look to the scheduler.

Starts several synthetic programs of every kind at once (so they compete for the CPU
like real ones do), measures them with `osaima-schedd sample` for a while, and writes one
row per program per window: the label and the five features the classifier reads. The
features are measured by the daemon's own code, so training and use cannot disagree.

  collect.py --schedd target/release/osaima-schedd --out dataset.csv
"""

import argparse
import csv
import os
import signal
import subprocess
import sys
import time

KINDS = ("interactive", "media", "batch", "background")
HERE = os.path.dirname(os.path.abspath(__file__))
FEATURES = ["cpu_util", "vol_rate", "invol_rate", "slice_ms", "wait_ratio"]


def run_round(schedd: str, seed: int, per_kind: int, hogs: int, seconds: float, interval: float):
    """One group of programs, measured together. Returns (label, features) rows."""
    workers: list[subprocess.Popen] = []
    labels: dict[int, str] = {}
    try:
        for kind in KINDS:
            for i in range(per_kind):
                p = subprocess.Popen([sys.executable, os.path.join(HERE, "workload.py"), kind, "--seed", str(seed * 100 + i)])
                workers.append(p)
                labels[p.pid] = kind
        for i in range(hogs):  # extra load so that waiting for a CPU is realistic; not measured
            workers.append(subprocess.Popen([sys.executable, os.path.join(HERE, "workload.py"), "batch", "--seed", str(9000 + i)]))
        time.sleep(1.5)  # let them settle
        out = subprocess.run(
            [schedd, "sample", "--pids", ",".join(map(str, labels)), "--seconds", str(seconds), "--interval", str(interval)],
            capture_output=True,
            text=True,
            check=True,
        ).stdout
    finally:
        for p in workers:
            p.send_signal(signal.SIGKILL)
        for p in workers:
            p.wait()
    rows = []
    for line in out.splitlines()[1:]:
        pid, _comm, *values = line.split(",")
        if int(pid) in labels:
            rows.append((labels[int(pid)], [float(v) for v in values]))
    return rows


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--schedd", required=True, help="the osaima-schedd binary")
    parser.add_argument("--out", required=True)
    parser.add_argument("--rounds", type=int, default=8)
    parser.add_argument("--per-kind", type=int, default=3)
    parser.add_argument("--hogs", type=int, default=3)
    parser.add_argument("--seconds", type=float, default=20.0)
    parser.add_argument("--interval", type=float, default=2.0)
    args = parser.parse_args()

    total = 0
    with open(args.out, "w", newline="") as handle:
        writer = csv.writer(handle)
        writer.writerow(["label", *FEATURES])
        for round_number in range(args.rounds):
            rows = run_round(args.schedd, round_number + 1, args.per_kind, args.hogs, args.seconds, args.interval)
            for label, values in rows:
                writer.writerow([label, *values])
            total += len(rows)
            print(f"round {round_number + 1}/{args.rounds}: {len(rows)} rows", flush=True)
    print(f"{total} rows written to {args.out}")
    return 0 if total else 1


if __name__ == "__main__":
    sys.exit(main())
