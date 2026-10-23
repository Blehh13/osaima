#!/usr/bin/env python3
"""
Builds the distillation dataset: (request -> compact intent JSON) pairs in the
exact shape the daemon expects from the LLM (mcp-daemon/src/prompts/intent.txt,
dispatched by the shell's dispatchIntent()).

Sources
  * ai-core/intent-router/{train,val,dataset_llm}.jsonl  -> training requests
  * ai-core/intent-router/testset.jsonl                  -> held-out test requests
    (the independently written paraphrase set the router is also scored on; the
    router never trained on it, so all models are compared on unseen text)
  * Gemini-written requests for what the router data lacks (automations, desktop
    settings, typed file operations, multi-step plans, chat, unclear input),
    split 85/15 into train/test by batch.

Labels
  * rules first: slots.py (the router's own extractor) gives exact params for the
    shell-handled and OS-control intents;
  * Gemini as the teacher for the rest (commands, files, notes, events...), with
    the daemon's full intent prompt, 40 requests per call, cached in cache/.

Output: data/train.jsonl, data/test.jsonl, one record per line:
  {"request", "now", "target": {"intent", "params", "reply"}, "label", "source"}

    python build_dataset.py                # uses cache/, calls Gemini only for what's missing
    python build_dataset.py --no-teacher   # rules only (no network)
"""
import argparse
import collections
import concurrent.futures as cf
import hashlib
import json
import os
import random
import sys
import threading

HERE = os.path.dirname(os.path.abspath(__file__))
ROUTER = os.path.join(HERE, "..", "intent-router")
PROMPTS = os.path.join(HERE, "..", "mcp-daemon", "src", "prompts")
sys.path.insert(0, ROUTER)
sys.path.insert(0, HERE)

import slots  # noqa: E402
import taxonomy  # noqa: E402

K_TRAIN, K_TEST = 36, 6          # requests per router label
BATCH = 40                        # requests labelled per teacher call
SEED = 13

# Intents the shell runs from the router's fast path (router_service.SHELL_HANDLED).
SHELL_HANDLED = {
    "open_app", "close_window", "close_all_windows", "minimize_all", "focus_app", "layout", "workspace",
    "set_gaps", "system_status", "optimize_cpu", "wake_apps", "clean_storage", "fix_network", "timer",
    "stop_timer", "reminder", "calendar", "calculator", "weather", "search_web", "scratchpad",
    "list_notes", "view_note", "delete_note", "search_notes", "greeting", "help",
}
GENERATION = {"email_draft", "commit_message", "code_generate", "code_refactor", "docstring_gen",
              "summarize_text", "explain_concept", "translate_text"}

INTENTS = ["open_app", "focus_app", "close_window", "close_all_windows", "minimize_all", "layout", "workspace",
           "set_gaps", "os_action", "timer", "stop_timer", "reminder", "calendar", "schedule_event", "calculator",
           "weather", "system_status", "optimize_cpu", "wake_apps", "clean_storage", "fix_network", "scratchpad",
           "search_web", "files", "create_note", "edit_note", "delete_note", "view_note", "list_notes",
           "search_notes", "execute_command", "task", "orchestrate", "automation", "generate_text", "greeting",
           "help", "chat", "unknown"]
OS_ACTION_KINDS = {"volume", "media", "brightness", "lock_screen", "wifi", "bluetooth", "power", "settings"}

# Gemini-written requests: (theme, how many per call, calls, description)
SYNTHETIC = [
    ("automation_time", 40, 2, "automations on a clock: once at a time, daily, weekdays/weekends, time windows (from..to), every N minutes, reminders that repeat"),
    ("automation_event", 40, 2, "automations on events: battery below/above N, plugged/unplugged, wifi connected/disconnected, an app opening or closing (while X is open...), after the current task/download finishes, on login, health alerts (cpu, temperature, disk)"),
    ("settings", 40, 3, "desktop settings done now through os_action settings: night light on/off (with temperature), do not disturb, wallpaper (contours, dawn, deep or an image path), display scale, resolution, keyboard layout, pointer speed, screen lock minutes, timezone, sound output device, network status; also asking the current volume or brightness (op get)"),
    ("files", 40, 4, "the user's own files: search by name/kind/size/age (pdf, images, videos, music, documents, archives, code), list a folder, open or reveal a file, copy, move, rename, trash, make a folder, undo the last file change; use ~/ paths"),
    ("orchestrate", 30, 3, "several desktop actions in one request, done now in order (open apps, close windows, layouts, workspaces, volume, web search, notes, files, timers)"),
    ("task", 30, 2, "bigger jobs that need several commands with results checked: set up a project or environment, install and configure software, fix or diagnose a problem, back up or convert many files"),
    ("chat", 40, 2, "small talk, opinions, jokes, feelings, thanks, questions about the assistant itself, with no computer action wanted"),
    ("unknown", 30, 1, "gibberish, random keystrokes, half sentences and truly unclear requests"),
    ("questions", 40, 1, "questions and writing requests: how-to questions, what is X, explain, write an email/letter/poem/code, summarise, translate (generate_text)"),
]

LABEL_RULES = """

You are labelling training data for a small on-device model. Follow the instructions above exactly, with these changes:
- "reply" is one short sentence (at most 15 words), except for chat where it is the full answer (1-2 sentences).
- generate_text: params is only {"title": "..."} with no content; another model writes the text afterwards.
- create_note / edit_note: "content" holds only what the user dictated. If the content has to be composed (a recipe, an essay, a list you would invent), use "content": "" and "write": true.
- execute_command: this system is Gentoo Linux with systemd (install with sudo emerge, services with systemctl). One command.
- Never use the shutdown intent; power off / restart / suspend are os_action with action power.
- "help" is its own intent with params {} for "what can you do" requests.
- Each request ends with the current local time, "(Now: ...)": use it for automation "at" triggers.
- Write paths with ~/ for the home folder.
"""


def now_text(rng):
    """Same format as automations::now_text() in the daemon: "Thu 2026-10-22 14:05"."""
    import datetime
    d = datetime.date(2026, 1, 1) + datetime.timedelta(days=rng.randrange(365))
    return f"{d.strftime('%a')} {d.isoformat()} {rng.randrange(24):02d}:{rng.choice([0, 5, 10, 15, 20, 30, 40, 45, 50, 55]):02d}"


def load(path):
    with open(path, encoding="utf-8") as f:
        return [json.loads(line) for line in f if line.strip()]


def relabel(rows):
    out = []
    for r in rows:
        label = taxonomy.OLD_TO_NEW.get(r["intent"], r["intent"])
        if label in taxonomy.INTENTS:
            out.append({"request": r["utterance"].strip(), "label": label})
    return out


def rule_target(label, text):
    """Exact target from the router's extractor, or None when the teacher must label it."""
    if label == "battery_status":
        return {"intent": "system_status", "params": {}, "reply": "Here's how your system is doing."}
    if label in slots.OS_ACTIONS:
        p = slots.extract(label, text)
        if p is None:
            return None
        return {"intent": "os_action", "params": p, "reply": f"{p['action'].replace('_', ' ').capitalize()}: {p['op']}."}
    if label in SHELL_HANDLED:
        p = slots.extract(label, text)
        if p is None:
            return None
        intent = taxonomy.SHELL_NAME.get(label, label)
        return {"intent": intent, "params": p, "reply": slots.reply_for(intent, p)}
    return None


def valid(t):
    if not isinstance(t, dict) or t.get("intent") not in INTENTS or not isinstance(t.get("params", {}), dict):
        return False
    p = t.get("params") or {}
    if t["intent"] == "os_action" and p.get("action") not in OS_ACTION_KINDS:
        return False
    if t["intent"] == "automation" and not isinstance(p.get("rule"), dict):
        return False
    if t["intent"] == "orchestrate" and not isinstance(p.get("steps"), list):
        return False
    return isinstance(t.get("reply", ""), str)


def clean(t):
    return {"intent": t["intent"], "params": t.get("params") or {}, "reply": (t.get("reply") or "").strip()}


class Cache:
    def __init__(self, path):
        self.path, self.lock = path, threading.Lock()
        self.data = {}
        if os.path.exists(path):
            for row in load(path):
                self.data[row["key"]] = row["value"]

    def get(self, key):
        return self.data.get(key)

    def put(self, key, value):
        with self.lock:
            self.data[key] = value
            with open(self.path, "a", encoding="utf-8") as f:
                f.write(json.dumps({"key": key, "value": value}, ensure_ascii=False) + "\n")


def teacher_system():
    with open(os.path.join(PROMPTS, "intent.txt"), encoding="utf-8") as f:
        return f.read() + LABEL_RULES


def label_batch(items, cache):
    """items: [{request, now, label}] -> fills item["target"] from Gemini (cached by text)."""
    import teacher
    todo = [it for it in items if cache.get(it["request"] + "|" + it["now"]) is None]
    if todo:
        lines = "\n".join(f"{i}. [category hint: {it['label']}] {it['request']} (Now: {it['now']})" for i, it in enumerate(todo))
        msg = [{"role": "system", "content": teacher_system()},
               {"role": "user", "content": "Label each request below. The category hint is a human label of the request's kind; "
                "follow it unless it is clearly wrong. Return {\"items\": [{\"i\": index, \"intent\": ..., \"params\": {...}, "
                "\"reply\": ...}]} with one item per request, in order.\n\n" + lines}]
        text, model, secs = teacher.chat(msg)
        got = teacher.parse_json(text).get("items", [])
        print(f"  teacher {model}: {len(got)}/{len(todo)} labels in {secs:.1f}s", flush=True)
        for g in got:
            i = g.get("i")
            if isinstance(i, int) and 0 <= i < len(todo) and valid(g):
                cache.put(todo[i]["request"] + "|" + todo[i]["now"], clean(g))
    for it in items:
        t = cache.get(it["request"] + "|" + it["now"])
        if t:
            it["target"], it["source"] = t, "teacher"


def synthesize(theme, n, call, desc, cache):
    key = f"{theme}#{call}#{n}"
    if cache.get(key) is not None:
        return cache.get(key)
    import teacher
    rng = random.Random(hashlib.md5(key.encode()).hexdigest())
    now = now_text(rng)
    msg = [{"role": "system", "content": teacher_system()},
           {"role": "user", "content": f"Write {n} new, varied requests a user might type to OSAIMA: {desc}. "
            f"Mix short and long, casual and polite, a few with typos; no two alike (this is batch {call + 1}, so avoid "
            f"the most obvious examples). The current local time is {now}. Then label each one. Return "
            "{\"items\": [{\"request\": ..., \"intent\": ..., \"params\": {...}, \"reply\": ...}]}."}]
    text, model, secs = teacher.chat(msg, max_tokens=24000)
    items = []
    for g in teacher.parse_json(text).get("items", []):
        if isinstance(g.get("request"), str) and g["request"].strip() and valid(g):
            items.append({"request": g["request"].strip(), "now": now, "label": theme, "source": "synthetic",
                          "target": clean(g)})
    print(f"  synthetic {theme} #{call}: {len(items)} from {model} in {secs:.1f}s", flush=True)
    cache.put(key, items)
    return items


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--no-teacher", action="store_true")
    ap.add_argument("--workers", type=int, default=3)
    args = ap.parse_args()
    rng = random.Random(SEED)
    os.makedirs(os.path.join(HERE, "cache"), exist_ok=True)
    os.makedirs(os.path.join(HERE, "data"), exist_ok=True)
    cache = Cache(os.path.join(HERE, "cache", "teacher.jsonl"))

    test_pool = relabel(load(os.path.join(ROUTER, "testset.jsonl")))
    test_texts = {r["request"].lower() for r in test_pool}
    seen, by_label = set(), collections.defaultdict(list)
    for f in ("train.jsonl", "val.jsonl", "dataset_llm.jsonl"):
        for r in relabel(load(os.path.join(ROUTER, f))):
            k = r["request"].lower()
            if k not in seen and k not in test_texts:
                seen.add(k)
                by_label[r["label"]].append(r)
    test_by_label = collections.defaultdict(list)
    for r in test_pool:
        test_by_label[r["label"]].append(r)

    splits = {"train": [], "test": []}
    for label in sorted(by_label):
        for split, pool, k in (("train", by_label[label], K_TRAIN), ("test", test_by_label[label], K_TEST)):
            for r in rng.sample(pool, min(k, len(pool))):
                it = {"request": r["request"], "now": now_text(rng), "label": label}
                t = rule_target(label, r["request"])
                if t:
                    it["target"], it["source"] = t, "rule"
                splits[split].append(it)

    need = [it for s in splits.values() for it in s if "target" not in it]
    print(f"rules labelled {sum(len(s) for s in splits.values()) - len(need)}; teacher needed for {len(need)}", flush=True)
    jobs = []
    if not args.no_teacher:
        with cf.ThreadPoolExecutor(args.workers) as pool:
            for i in range(0, len(need), BATCH):
                jobs.append(pool.submit(label_batch, need[i:i + BATCH], cache))
            synth = [pool.submit(synthesize, theme, n, c, desc, cache)
                     for theme, n, calls, desc in SYNTHETIC for c in range(calls)]
            for j in jobs:
                try:
                    j.result()
                except Exception as e:
                    print("  batch failed:", e, flush=True)
            for theme_calls in synth:
                try:
                    items = theme_calls.result()
                except Exception as e:
                    print("  synthetic failed:", e, flush=True)
                    continue
                # hold out the tail of every synthetic batch (by batch, so no near-duplicates leak)
                cut = int(len(items) * 0.85)
                splits["train"] += items[:cut]
                splits["test"] += items[cut:]

    for split, items in splits.items():
        items = [it for it in items if "target" in it]
        rng.shuffle(items)
        with open(os.path.join(HERE, "data", f"{split}.jsonl"), "w", encoding="utf-8") as f:
            for it in items:
                f.write(json.dumps(it, ensure_ascii=False) + "\n")
        c = collections.Counter(it["target"]["intent"] for it in items)
        src = collections.Counter(it["source"] for it in items)
        print(f"{split}: {len(items)} examples, {len(c)} intents, sources {dict(src)}")
        print("  ", dict(c.most_common()))


if __name__ == "__main__":
    main()
