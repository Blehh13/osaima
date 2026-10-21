#!/usr/bin/env python3
"""
OSAIMA Intent Router — REAL LLM synthesis (PDR §3.3, §5.4 step 15).

Generates the 10,000-utterance training corpus by prompting the local LLM via
Ollama, exactly as the PDR specifies ("Synthesize 10,000 utterances using the
LLM itself"). Each intent gets a tailored generation prompt; the model returns
varied, natural utterances that a user might actually speak.

This is the production generator. generate_dataset.py (template-based) remains as
a deterministic, GPU-free fallback / smoke test.

Prereqs:
    curl -fsSL https://ollama.com/install.sh | sh
    ollama pull llama3.1:8b-instruct-q4_K_M
    pip install ollama

Usage:
    python generate_dataset_llm.py                          # 10k, default model
    python generate_dataset_llm.py --per-intent 250 --model mistral:7b-instruct
    python generate_dataset_llm.py --dry-run                # print prompts, call nothing

Output: dataset_llm.jsonl  {id, utterance, track, intent, edge_case, source}
"""
import argparse, json, os, sys, time, random

random.seed(42)

# 3-track x 40-intent taxonomy with a natural-language DESCRIPTION per intent.
# The description is what the LLM is asked to write utterances for.
TAXONOMY = {
    # track, human description used in the generation prompt
    "app_launch":        ("SYSTEM_COMMAND", "opening or launching a desktop application"),
    "app_close":         ("SYSTEM_COMMAND", "closing or quitting a running application"),
    "window_focus":      ("SYSTEM_COMMAND", "moving keyboard focus between open windows"),
    "window_tile":       ("SYSTEM_COMMAND", "changing window layout: tile, fullscreen, grid, or snap to an edge"),
    "window_move_ws":    ("SYSTEM_COMMAND", "moving a window to another workspace or switching workspaces"),
    "volume_control":    ("SYSTEM_COMMAND", "changing or muting system audio volume"),
    "media_playback":    ("SYSTEM_COMMAND", "controlling media playback: play, pause, next, previous"),
    "brightness_control":("SYSTEM_COMMAND", "changing the screen brightness"),
    "screenshot":        ("SYSTEM_COMMAND", "taking a screenshot of the screen or a window"),
    "lock_screen":       ("SYSTEM_COMMAND", "locking the screen or the computer"),
    "power_control":     ("SYSTEM_COMMAND", "shutting down, rebooting, or suspending the machine"),
    "wifi_control":      ("SYSTEM_COMMAND", "enabling, disabling, or connecting wifi"),
    "bluetooth_control": ("SYSTEM_COMMAND", "enabling, disabling, or pairing bluetooth devices"),
    "vpn_control":       ("SYSTEM_COMMAND", "connecting to or disconnecting from a VPN"),
    "service_control":   ("SYSTEM_COMMAND", "starting, stopping, restarting, or checking a systemd service"),
    "process_list":      ("SYSTEM_COMMAND", "listing or showing the currently running processes"),
    "process_kill":      ("SYSTEM_COMMAND", "killing, terminating, or force-quitting a process"),
    "system_stats":      ("SYSTEM_COMMAND", "asking about CPU, memory, disk, or temperature usage"),
    "battery_status":    ("SYSTEM_COMMAND", "asking about battery level or charging status"),
    "file_search":       ("SYSTEM_COMMAND", "searching for or locating files by name, type, or date"),
    "file_open":         ("SYSTEM_COMMAND", "opening a specific file"),
    "clipboard_control": ("SYSTEM_COMMAND", "copying, pasting, or viewing clipboard contents"),
    "notification_ctrl": ("SYSTEM_COMMAND", "toggling do-not-disturb or clearing notifications"),
    "mount_control":     ("SYSTEM_COMMAND", "mounting, unmounting, or ejecting a drive"),
    "project_scaffold":  ("WORKFLOW_GENERATION", "setting up or scaffolding a new software project"),
    "git_workflow":      ("WORKFLOW_GENERATION", "multi-step git operations like commit, branch, and push"),
    "env_setup":         ("WORKFLOW_GENERATION", "creating and activating a virtual or conda environment"),
    "package_install":   ("WORKFLOW_GENERATION", "installing project dependencies or packages"),
    "run_tests":         ("WORKFLOW_GENERATION", "running a test suite, optionally with coverage"),
    "deploy_workflow":   ("WORKFLOW_GENERATION", "building a container, deploying, or starting a dev server"),
    "file_batch_op":     ("WORKFLOW_GENERATION", "batch file operations: rename, convert, or organize many files"),
    "build_compile":     ("WORKFLOW_GENERATION", "compiling or building a project, possibly in watch mode"),
    "email_draft":       ("CONTENT_GENERATION", "drafting, replying to, or composing an email"),
    "commit_message":    ("CONTENT_GENERATION", "writing a git commit message for some changes"),
    "code_generate":     ("CONTENT_GENERATION", "generating new code such as a function, script, or class"),
    "code_refactor":     ("CONTENT_GENERATION", "refactoring, cleaning up, or restructuring existing code"),
    "docstring_gen":     ("CONTENT_GENERATION", "generating docstrings, comments, or type hints"),
    "summarize_text":    ("CONTENT_GENERATION", "summarizing a document, readme, or file"),
    "explain_concept":   ("CONTENT_GENERATION", "explaining a concept, command, or error message"),
    "translate_text":    ("CONTENT_GENERATION", "translating text into another language"),
}
assert len(TAXONOMY) == 40

SYS_PROMPT = (
    "You generate short, natural spoken commands a person would say out loud to a "
    "voice-controlled Linux assistant. Output ONLY a JSON array of strings. "
    "No numbering, no explanations, no markdown. Each string is one utterance, "
    "lowercase, 3-14 words, phrased many different ways (imperative, question, "
    "polite, terse). Do not repeat yourself."
)

def gen_prompt(desc, n):
    return (f"Generate {n} distinct utterances for the intent: {desc}. "
            f"Vary the wording heavily. Return a JSON array of exactly {n} strings.")

def parse_array(text):
    """Best-effort extraction of a JSON string array from model output."""
    text = text.strip()
    start, end = text.find("["), text.rfind("]")
    if start != -1 and end != -1:
        try:
            arr = json.loads(text[start:end+1])
            return [str(x).strip() for x in arr if str(x).strip()]
        except json.JSONDecodeError:
            pass
    # fallback: one utterance per line
    return [l.strip(" -*\t\"") for l in text.splitlines() if l.strip()]

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--model", default="llama3.1:8b-instruct-q4_K_M")
    ap.add_argument("--per-intent", type=int, default=250)   # 250 x 40 = 10,000
    ap.add_argument("--out", default="dataset_llm.jsonl")
    ap.add_argument("--batch", type=int, default=25)         # utterances per LLM call
    ap.add_argument("--dry-run", action="store_true", help="print prompts, do not call Ollama")
    args = ap.parse_args()

    if args.dry_run:
        for intent, (track, desc) in list(TAXONOMY.items())[:3]:
            print(f"\n# {intent} [{track}]\nSYSTEM: {SYS_PROMPT}\nUSER: {gen_prompt(desc, args.batch)}")
        print(f"\n(dry run) would generate {args.per_intent} x {len(TAXONOMY)} "
              f"= {args.per_intent*len(TAXONOMY)} utterances via '{args.model}'")
        return

    try:
        from ollama import Client
    except ImportError:
        sys.exit("pip install ollama  (and make sure the ollama server is running)")

    # timeout so a hung Ollama request raises instead of blocking forever;
    # the batch loop below then retries automatically.
    client = Client(timeout=90)

    # ---- CRASH-SAFE / RESUMABLE ----
    # We APPEND every intent's rows to args.out as soon as they're generated, so a
    # Ctrl+C, crash, or network blip never loses finished work. On restart we read
    # what's already there and skip intents that are already complete.
    seen = set()
    done_counts = {}
    if os.path.exists(args.out):
        with open(args.out, encoding="utf-8") as f:
            for line in f:
                try:
                    r = json.loads(line)
                except json.JSONDecodeError:
                    continue
                seen.add((r["intent"], r["utterance"].lower()))
                done_counts[r["intent"]] = done_counts.get(r["intent"], 0) + 1
        if done_counts:
            print(f"Resuming: {sum(done_counts.values())} rows already in {args.out}", flush=True)

    out = open(args.out, "a", encoding="utf-8")  # append mode
    total = sum(done_counts.values())

    try:
        for intent, (track, desc) in TAXONOMY.items():
            already = done_counts.get(intent, 0)
            need = args.per_intent - already
            if need <= 0:
                print(f"[{intent}] already done ({already}) — skip", flush=True)
                continue
            got = 0
            print(f"[{intent}] generating {need}"
                  + (f" (had {already})" if already else "") + "...", flush=True)
            while got < need:
                n = min(args.batch, need - got)
                try:
                    resp = client.chat(
                        model=args.model,
                        messages=[{"role": "system", "content": SYS_PROMPT},
                                  {"role": "user", "content": gen_prompt(desc, n)}],
                        options={"temperature": 1.0, "top_p": 0.95},
                        keep_alive="30m",   # don't let the model unload mid-run
                    )
                    utts = parse_array(resp["message"]["content"])
                except Exception as e:
                    print(f"  ! {intent}: {e} — retrying in 3s", flush=True)
                    time.sleep(3); continue

                for u in utts:
                    key = (intent, u.lower())
                    if u and key not in seen:
                        seen.add(key)
                        out.write(json.dumps({"id": total, "utterance": u, "track": track,
                                  "intent": intent, "edge_case": False, "source": "llm"},
                                  ensure_ascii=False) + "\n")
                        total += 1; got += 1
                        if got >= need: break
                out.flush()  # persist after every batch
    except KeyboardInterrupt:
        print(f"\nInterrupted — {total} rows safely saved in {args.out}. "
              f"Re-run the same command to resume.", flush=True)
        out.close(); return
    finally:
        out.close()

    print(f"\nWrote {total} LLM-synthesized utterances -> {args.out}")
    print("Tip: shuffle with a one-liner if you want, then:")
    print("  python split_dataset.py --input", args.out)
    print("NOTE: hand-label ~500 ambiguous ones as edge_case=True (PDR §3.3).")

if __name__ == "__main__":
    main()
