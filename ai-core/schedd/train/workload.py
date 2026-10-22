#!/usr/bin/env python3
"""A synthetic program with a known behaviour, for training and benchmarking the scheduler.

Each kind imitates what real programs of that kind do to the CPU:

  interactive  waits for input, then does a short burst of work (editors, shells, menus)
  media        a fixed-rate loop with a deadline: work, then sleep until the next tick
               (video and audio playback, games, animation)
  batch        computes flat out, with the odd pause (compilers, encoders, renderers)
  background   wakes rarely, does almost nothing, goes back to sleep (daemons, indexers)

"Work" is CPU time, not wall time: if the program is held up by other programs, it takes
longer to get its work done, exactly as a real one would.

  workload.py KIND [--seed N] [--seconds S]
"""

import argparse
import random
import sys
import time


def busy(ms: float) -> None:
    """Use `ms` milliseconds of CPU time."""
    end = time.thread_time_ns() + int(ms * 1e6)
    while time.thread_time_ns() < end:
        pass


def interactive(rng: random.Random, deadline: float) -> None:
    while time.monotonic() < deadline:
        busy(rng.uniform(0.3, 6.0))
        time.sleep(rng.uniform(0.01, 0.15))


def media(rng: random.Random, deadline: float) -> None:
    period = rng.choice([1 / 30, 1 / 50, 1 / 60, 1 / 100])
    work_ms = period * 1000 * rng.uniform(0.15, 0.55)
    tick = time.monotonic()
    while tick < deadline:
        busy(work_ms)
        tick += period
        pause = tick - time.monotonic()
        if pause > 0:
            time.sleep(pause)
        else:
            tick = time.monotonic()  # fell behind: do not try to catch up


def batch(rng: random.Random, deadline: float) -> None:
    pause_every = rng.choice([None, 2.0, 5.0])
    last_pause = time.monotonic()
    while time.monotonic() < deadline:
        busy(rng.uniform(150, 1500))
        if pause_every and time.monotonic() - last_pause > pause_every:
            time.sleep(rng.uniform(0.005, 0.05))  # a bit of I/O now and then
            last_pause = time.monotonic()


def background(rng: random.Random, deadline: float) -> None:
    while time.monotonic() < deadline:
        busy(rng.uniform(0.05, 1.0))
        time.sleep(rng.uniform(0.5, 5.0))


KINDS = {"interactive": interactive, "media": media, "batch": batch, "background": background}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("kind", choices=sorted(KINDS))
    parser.add_argument("--seed", type=int, default=0)
    parser.add_argument("--seconds", type=float, default=3600.0)
    args = parser.parse_args()
    KINDS[args.kind](random.Random(args.seed), time.monotonic() + args.seconds)
    return 0


if __name__ == "__main__":
    sys.exit(main())
