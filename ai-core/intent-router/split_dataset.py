#!/usr/bin/env python3
"""
Split the intent corpus into train/val/test (80/10/10), STRATIFIED by intent so
every split keeps all 40 classes in proportion. Edge cases are spread across
splits too. (PDR §9.1 keeps a 1,000-utterance held-out test set; you can either
use testset.jsonl directly as test, or produce an internal split here.)

Usage:
    python split_dataset.py                     # splits dataset.jsonl
    python split_dataset.py --input dataset_llm.jsonl --ratios 0.8 0.1 0.1
Output: train.jsonl, val.jsonl, test.jsonl + labels.json (intent<->id maps)
"""
import argparse, json, random
from collections import defaultdict

random.seed(42)

def load(path):
    with open(path, encoding="utf-8") as f:
        return [json.loads(l) for l in f if l.strip()]

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--input", default="dataset.jsonl")
    ap.add_argument("--ratios", nargs=3, type=float, default=[0.8, 0.1, 0.1])
    args = ap.parse_args()
    assert abs(sum(args.ratios) - 1.0) < 1e-6, "ratios must sum to 1"

    rows = load(args.input)

    # stratify by intent
    by_intent = defaultdict(list)
    for r in rows: by_intent[r["intent"]].append(r)

    train, val, test = [], [], []
    for intent, items in by_intent.items():
        random.shuffle(items)
        n = len(items)
        n_tr = int(n * args.ratios[0])
        n_va = int(n * args.ratios[1])
        train += items[:n_tr]
        val   += items[n_tr:n_tr+n_va]
        test  += items[n_tr+n_va:]

    for split in (train, val, test): random.shuffle(split)

    # stable label maps for the classifier
    intents = sorted(by_intent)
    tracks = sorted({r["track"] for r in rows})
    labels = {
        "intent2id": {v: i for i, v in enumerate(intents)},
        "id2intent": {i: v for i, v in enumerate(intents)},
        "track2id":  {v: i for i, v in enumerate(tracks)},
        "id2track":  {i: v for i, v in enumerate(tracks)},
    }

    for name, split in [("train", train), ("val", val), ("test", test)]:
        with open(f"{name}.jsonl", "w", encoding="utf-8") as f:
            for r in split: f.write(json.dumps(r, ensure_ascii=False) + "\n")
    with open("labels.json", "w", encoding="utf-8") as f:
        json.dump(labels, f, indent=2)

    print(f"input {len(rows)} -> train {len(train)} | val {len(val)} | test {len(test)}")
    print(f"{len(intents)} intents, {len(tracks)} tracks -> labels.json")
    # verify every intent survived in every split
    for name, split in [("train", train), ("val", val), ("test", test)]:
        miss = set(intents) - {r["intent"] for r in split}
        if miss: print(f"  WARNING {name} missing intents: {miss}")

if __name__ == "__main__":
    main()
