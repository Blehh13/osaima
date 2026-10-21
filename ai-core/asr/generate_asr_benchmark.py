#!/usr/bin/env python3
"""
OSAIMA ASR benchmark corpus (PDR §9.1): 500 reference transcripts with
tech / Linux / developer vocabulary.

WHAT THIS PRODUCES: the GROUND-TRUTH TEXT only (transcripts.jsonl + prompts.txt).
To measure WER you (or a tester) read each prompt aloud and record it; the ASR
output is then compared against the reference here.

WER computation is included (Levenshtein at word level) so once you have the
hypothesis transcripts you can score immediately:
    python generate_asr_benchmark.py                 # writes the 500 references
    python generate_asr_benchmark.py --wer hyp.txt   # scores hypotheses vs refs

Categories weight the corpus toward the vocabulary the router/LLM actually see.
"""
import json, argparse, random
random.seed(7)

# Sentence banks rich in tech/Linux/developer terms (hard for generic ASR)
BANKS = {
    "shell_commands": [
        "run sudo apt install pipewire and wireplumber",
        "chmod plus x the deploy script then execute it",
        "grep dash r for the string TODO across the source tree",
        "kill dash nine the process id four two seven one",
        "systemctl restart the nginx service and check its status",
        "export the PATH variable then source the bashrc",
        "use ripgrep to search the entire codebase for the function",
        "docker compose up dash d and tail the logs",
        "git rebase dash i onto the main branch",
        "curl dash f s s L the install script and pipe it to bash",
    ],
    "dev_tools": [
        "open the integrated terminal in visual studio code",
        "the language server protocol handles autocomplete and refactoring",
        "fine tune the bert tiny model and export it to onnx",
        "the ollama runtime serves llama three point one on localhost",
        "chromadb stores the embeddings from the mini l m model",
        "the pytest suite reports coverage for every module",
        "webpack bundles the typescript into a single artifact",
        "the kubernetes pod is stuck in crash loop back off",
        "npm install the model context protocol server package",
        "the cuda toolkit accelerates whisper large v three inference",
    ],
    "linux_internals": [
        "read proc meminfo to check the available memory",
        "the ebpf program attaches to the perf event source",
        "apparmor confines the daemon to a read only home directory",
        "the systemd unit runs as a user scope with private tmp",
        "d bus exposes the network manager interface over the session bus",
        "at spi provides the accessibility tree for gtk four apps",
        "pipewire replaces pulseaudio for lower latency capture",
        "the cgroups v two hierarchy accounts per process resource usage",
        "inotify watches the documents directory for new files",
        "wayland compositors handle window management differently from x eleven",
    ],
    "workflow_phrases": [
        "set up a django project with my usual virtual environment",
        "commit everything with my conventional commit format and push",
        "scaffold a fast api service with async endpoints",
        "install the requirements then run the migrations",
        "build the release binary and strip the debug symbols",
        "create a new git branch called feature slash auth",
        "generate a docstring for every public method in this class",
        "refactor the handler into smaller pure functions",
        "add type hints to the parameters and return value",
        "summarize the readme and draft a release note",
    ],
    "acronyms_numbers": [
        "the a s r daemon emits j s o n over a unix socket",
        "quantize the model to q four k m for six gigabytes of ram",
        "the c p u load is at ninety five percent on eight cores",
        "the r a g engine retrieves the top five chunks by cosine similarity",
        "l l m latency should stay under eight seconds on c p u",
        "the s h a two fifty six audit log is append only",
        "the g p u is an r t x three zero six zero with twelve gigabytes",
        "the w e r target is under eight percent for whisper",
        "http localhost one one four three four serves the ollama a p i",
        "the l s p client talks j s o n r p c over a web socket",
    ],
}

def build(n=500):
    cats = list(BANKS)
    rows, i = [], 0
    # round-robin across categories until we hit n
    while len(rows) < n:
        cat = cats[i % len(cats)]
        text = random.choice(BANKS[cat])
        rows.append({"id": len(rows), "category": cat, "reference": text,
                     "n_words": len(text.split())})
        i += 1
    return rows

def wer(ref, hyp):
    r, h = ref.split(), hyp.split()
    # word-level Levenshtein
    d = [[0]*(len(h)+1) for _ in range(len(r)+1)]
    for a in range(len(r)+1): d[a][0] = a
    for b in range(len(h)+1): d[0][b] = b
    for a in range(1, len(r)+1):
        for b in range(1, len(h)+1):
            cost = 0 if r[a-1] == h[b-1] else 1
            d[a][b] = min(d[a-1][b]+1, d[a][b-1]+1, d[a-1][b-1]+cost)
    return d[len(r)][len(h)] / max(1, len(r))

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--wer", metavar="HYP_FILE",
                    help="score a hypothesis file (one transcript per line) against the references")
    args = ap.parse_args()
    rows = build()

    if args.wer:
        with open(args.wer, encoding="utf-8") as f:
            hyps = [l.strip() for l in f if l.strip()]
        assert len(hyps) == len(rows), f"need {len(rows)} hypotheses, got {len(hyps)}"
        scores = [wer(r["reference"], h) for r, h in zip(rows, hyps)]
        print(f"Corpus WER: {sum(scores)/len(scores)*100:.2f}%  over {len(scores)} utterances")
        return

    with open("transcripts.jsonl", "w", encoding="utf-8") as f:
        for r in rows: f.write(json.dumps(r, ensure_ascii=False) + "\n")
    with open("prompts.txt", "w", encoding="utf-8") as f:   # what the tester reads aloud
        for r in rows: f.write(r["reference"] + "\n")
    from collections import Counter
    print(f"Wrote {len(rows)} references -> transcripts.jsonl + prompts.txt")
    print("By category:", dict(Counter(r["category"] for r in rows)))
    print("Total words:", sum(r["n_words"] for r in rows))

if __name__ == "__main__":
    main()
