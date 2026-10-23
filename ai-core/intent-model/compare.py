#!/usr/bin/env python3
"""Compares saved bench.py runs on the requests they share (a small cloud or
Phi-4 sample against the full-split runs), so every row is the same test set.

    python compare.py phi4-mini-full router qwen0.5b-untuned gemini-flash-lite
The first name picks the sample; the others are scored on the same requests.
"""
import json
import os
import statistics
import sys

import bench

HERE = os.path.dirname(os.path.abspath(__file__))


def load(name):
    with open(os.path.join(HERE, "results", name + ".jsonl"), encoding="utf-8") as f:
        rows = [json.loads(l) for l in f]
    for r in rows:
        r.update(bench.score(r["gold"], r["pred"]) if isinstance(r["pred"], dict) else {"intent": False, "exact": False})
    return {r["request"]: r for r in rows}


def main():
    names = sys.argv[1:]
    runs = {n: load(n) for n in names}
    sample = set(runs[names[0]])
    for n in names[1:]:
        sample &= set(runs[n])
    print(f"{len(sample)} shared requests")
    print("| backend | intent | intent+params | p50 ms |")
    print("|---|---|---|---|")
    for n in names:
        rows = [runs[n][q] for q in sample]
        print(f"| {n} | {100 * sum(r['intent'] for r in rows) / len(rows):.1f}% | "
              f"{100 * sum(r['exact'] for r in rows) / len(rows):.1f}% | {statistics.median(r['ms'] for r in rows):.0f} |")


if __name__ == "__main__":
    main()
