#!/usr/bin/env python3
"""
Held-out test utterances for intents that the original testset.jsonl does not
cover, written by the local LLM rather than by the person who wrote the
training templates, so the test does not share the trainer's phrasing habits.

Each intent is described to the model using the shell's own intent definitions
(ai-core/mcp-daemon/src/prompts/intent.txt). Needs an OpenAI-compatible server,
e.g. ai-core/llm/start-llm.sh.

Usage:  python generate_heldout_llm.py [--per-intent 20] [--url http://127.0.0.1:8080/v1]
Output: heldout_llm.jsonl   schema {utterance, track, intent, source}
"""
import argparse, json, os, re, urllib.request

import taxonomy

HERE = os.path.dirname(os.path.abspath(__file__))
PROMPT_FILE = os.path.join(HERE, "..", "mcp-daemon", "src", "prompts", "intent.txt")

# Intents the shell prompt does not describe (they come from the PDR list).
EXTRA = {
    "help": "User asks what the assistant can do or how to use it.",
}


def shell_descriptions():
    desc = {}
    for line in open(PROMPT_FILE, encoding="utf-8"):
        m = re.match(r"- ([a-z_]+): (.+)", line.strip())
        if m:
            desc[m.group(1)] = m.group(2)
    return desc


def ask(url, intent, description, n):
    prompt = (
        f"Write {n} different things a person might type or say to their computer's AI assistant "
        f"when they want this:\n\n{description}\n\n"
        "Vary the wording a lot: short commands, polite requests, questions, casual slang, "
        "and sentences with extra context. Do not number them. Use different verbs. "
        'Reply with only a JSON object: {"examples": ["...", "..."]}'
    )
    body = json.dumps({
        "messages": [{"role": "user", "content": prompt}],
        "temperature": 0.9, "max_tokens": 900,
        "response_format": {"type": "json_object"},
    }).encode()
    req = urllib.request.Request(f"{url}/chat/completions", body, {"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=600) as r:
        text = json.load(r)["choices"][0]["message"]["content"]
    return [e.strip() for e in json.loads(text).get("examples", []) if isinstance(e, str) and e.strip()]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--url", default="http://127.0.0.1:8080/v1")
    ap.add_argument("--per-intent", type=int, default=20)
    ap.add_argument("--out", default="heldout_llm.jsonl")
    args = ap.parse_args()

    covered = set(taxonomy.OLD_TO_NEW.values())
    targets = [i for i in taxonomy.INTENTS if i not in covered]
    desc = {**shell_descriptions(), **EXTRA}
    # the shell calls these shutdown/vitals; the unified names differ
    train_texts = {t.lower() for _, ts in taxonomy.INTENTS.values() for t in ts}

    done = set()
    if os.path.exists(args.out):
        done = {json.loads(l)["intent"] for l in open(args.out, encoding="utf-8") if l.strip()}
    with open(args.out, "a", encoding="utf-8") as out:
        for intent in targets:
            if intent in done:
                continue
            if intent not in desc:
                print(f"skip {intent}: no description")
                continue
            examples = ask(args.url, intent, desc[intent], args.per_intent)
            seen = set()
            kept = 0
            for e in examples:
                key = e.lower().rstrip(".!?")
                if key in seen or key in train_texts:
                    continue
                seen.add(key)
                out.write(json.dumps({"utterance": e, "track": taxonomy.track_of(intent),
                                      "intent": intent, "source": "llm"}, ensure_ascii=False) + "\n")
                kept += 1
            out.flush()
            print(f"{intent:<18} {kept} examples", flush=True)


if __name__ == "__main__":
    main()
