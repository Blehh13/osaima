#!/usr/bin/env python3
"""
OSAIMA Intent Router — synthetic training set generator.

Produces the training corpus described in the PDR (§3.3, §5.4), labelled with
the unified taxonomy in taxonomy.py (3 tracks, one label per intent the shell or
the AI core can act on).

Deterministic and dependency-free (seed 42): slot templates are filled with
varied values and wrapped in natural prefixes/suffixes, with light casing,
punctuation and typo noise so the classifier learns meaning, not exact strings.
generate_dataset_llm.py is the LLM-written alternative.

Output:  dataset.jsonl   one JSON object per line
         sample.csv      first 200 rows, for the report/appendix

Row schema:
  {"id", "utterance", "track", "intent", "edge_case"}
"""

import csv
import json
import random
import re
from collections import Counter

import taxonomy

random.seed(42)  # reproducible

PER_INTENT = 300

PREFIXES = ["", "", "", "", "hey osaima ", "osaima, ", "can you ", "please ", "could you ", "ok ",
            "i want to ", "i'd like to ", "quickly ", "hey, ", "would you mind if you ", "go ahead and "]
SUFFIXES = ["", "", "", "", " please", " now", " for me", " thanks", " asap", " right now", "?", "!", "."]

# Ambiguous one-liners from the original generator, kept once each (not
# duplicated) with the label a user most likely means.
EDGE_CASES = [
    ("run this", "execute_command"),
    ("open my project", "file_open"),
    ("fix this", "code_refactor"),
    ("push it", "git_workflow"),
    ("close everything and clean up", "close_all_windows"),
    ("set it up the usual way", "project_scaffold"),
    ("write it out", "code_generate"),
    ("show me the notes about the tests", "search_notes"),
    ("make this better", "code_refactor"),
    ("send it", "email_draft"),
    ("kill it", "process_kill"),
    ("build me something", "code_generate"),
    ("connect", "wifi_control"),
    ("summarize and email it to my team", "email_draft"),
    ("turn it off", "power_control"),
]

# Keyboard-adjacent swaps for occasional typos.
NEIGHBOURS = {"a": "s", "e": "r", "i": "o", "o": "p", "n": "m", "t": "y", "s": "a", "r": "e", "u": "i"}


def fill(template):
    return re.sub(r"\{(\w+)\}", lambda m: random.choice(taxonomy.SLOTS[m.group(1)]), template)


def noisy(text):
    roll = random.random()
    if roll < 0.06 and len(text) > 6:  # one typo
        idxs = [i for i, c in enumerate(text) if c in NEIGHBOURS]
        if idxs:
            i = random.choice(idxs)
            text = text[:i] + NEIGHBOURS[text[i]] + text[i + 1:]
    elif roll < 0.12:
        text = text.capitalize()
    elif roll < 0.15:
        text = text.upper()
    return text


def utterance(template):
    core = fill(template)
    # Prefixes like "can you " read badly before a bare noun or a question.
    prefix = random.choice(PREFIXES) if len(core.split()) > 1 and not core.endswith("?") else ""
    return noisy((prefix + core + random.choice(SUFFIXES)).strip())


def generate(per_intent=PER_INTENT):
    rows = []
    for intent, (track, templates) in taxonomy.INTENTS.items():
        seen = set()
        attempts = 0
        while sum(1 for r in rows if r["intent"] == intent) < per_intent and attempts < per_intent * 20:
            attempts += 1
            u = utterance(random.choice(templates))
            if u.lower() in seen:
                continue
            seen.add(u.lower())
            rows.append({"utterance": u, "track": track, "intent": intent, "edge_case": False})

    for utt, intent in EDGE_CASES:
        rows.append({"utterance": utt, "track": taxonomy.track_of(intent), "intent": intent, "edge_case": True})

    random.shuffle(rows)
    for new_id, r in enumerate(rows):
        r["id"] = new_id
    return [{"id": r["id"], **{k: r[k] for k in ("utterance", "track", "intent", "edge_case")}} for r in rows]


def main():
    rows = generate()

    with open("dataset.jsonl", "w", encoding="utf-8") as f:
        for r in rows:
            f.write(json.dumps(r, ensure_ascii=False) + "\n")

    with open("sample.csv", "w", newline="", encoding="utf-8") as f:
        w = csv.DictWriter(f, fieldnames=["id", "utterance", "track", "intent", "edge_case"])
        w.writeheader()
        for r in rows[:200]:
            w.writerow(r)

    # quick label distribution report
    per_intent = Counter(r["intent"] for r in rows)
    print(f"Wrote {len(rows)} rows -> dataset.jsonl  (sample.csv = first 200)")
    print("Track distribution:", dict(Counter(r["track"] for r in rows)))
    print("Edge cases:", sum(r["edge_case"] for r in rows))
    print(f"Distinct intents: {len(per_intent)}  (min {min(per_intent.values())}, max {max(per_intent.values())} per intent)")


if __name__ == "__main__":
    main()
