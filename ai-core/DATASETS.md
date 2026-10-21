# OSAIMA — Datasets We Build / Curate

Documentation of all seven datasets referenced in the PDR (§3.3, §3.5, §3.7,
§5.4, §9.1). Each is either **generated here** (scripts in this repo) or
**captured by the user** (audio). This file is the appendix reference for the
project report.

| # | Dataset | Size | Path | How produced | Runs offline? |
|---|---------|------|------|--------------|:---:|
| 1 | Intent training set | 10,000 | `intent-router/dataset.jsonl` | template gen (`generate_dataset.py`) or LLM gen (`generate_dataset_llm.py`) | ✅ |
| 2 | Intent held-out test set | 1,000 | `intent-router/testset.jsonl` | `generate_testset.py` (new phrasings) | ✅ |
| 3 | ASR benchmark corpus | 500 refs | `asr/transcripts.jsonl`, `asr/prompts.txt` | `generate_asr_benchmark.py` (+ WER scorer) | ⚠️ text only; WER needs your audio |
| 4 | Wake-word "Hey OSAIMA" | ≥50 clips | `asr/positives/`, `asr/negatives/` | `record_wakeword.py` (your mic) | ❌ hardware |
| 5 | Prompt-injection red-team | 50 | `security/redteam_prompts.jsonl` | authored | ✅ |
| 6 | RAG knowledge corpus | seed + chunker | `rag-engine/seed_corpus/`, `chunker.py` | `chunker.py` (512-tok / 50 overlap) | ✅ |
| 7 | Behavior store | 30-day seed | `behavior/behavior.db`, `schema.sql` | `seed_behavior.py` | ✅ |

Common row schema for text corpora: `{id, utterance/reference, track, intent, ...}`.

---

## 1 · Intent-Router Training Set (10,000)
- **Taxonomy:** 3 tracks × 40 intents.
  - `SYSTEM_COMMAND` (24 intents, fast path <200 ms)
  - `WORKFLOW_GENERATION` (8 intents, slow path <8 s)
  - `CONTENT_GENERATION` (8 intents, slow path <12 s)
- **Two generators:**
  - `generate_dataset.py` — deterministic slot templates, GPU-free, reproducible (`seed=42`). Used for smoke tests / CI.
  - `generate_dataset_llm.py` — **real Ollama synthesis** (PDR-accurate): prompts Llama-3.1-8B for varied utterances per intent, dedupes, shuffles.
- **Edge cases:** 500 rows flagged `edge_case=True` — deliberately ambiguous ("run this", "push it", "kill it") → hand-labelled per PDR §3.3.
- **Distribution:** SYSTEM_COMMAND ~59%, CONTENT ~21%, WORKFLOW ~20% (SYSTEM holds most intents → realistic).

```bash
python generate_dataset.py                     # template (offline)
python generate_dataset_llm.py --per-intent 250   # LLM (needs ollama)
```

## 2 · Intent Held-Out Test Set (1,000)
- Same 40-intent taxonomy, **different surface phrasings and seed** (`seed=2025`) from train → genuine held-out, not memorised.
- 900 clean + 100 ambiguous edge cases.
- **Target: >92% classification accuracy** (PDR §9.1).

```bash
python generate_testset.py
```

## 3 · ASR Benchmark Corpus (500)
- 500 reference transcripts rich in tech/Linux/developer vocabulary, 5 categories:
  `shell_commands`, `dev_tools`, `linux_internals`, `workflow_phrases`, `acronyms_numbers`.
- **Ground-truth text only** — the assistant cannot record audio. A tester reads `prompts.txt` aloud; ASR output is scored against the references.
- Built-in **word-level Levenshtein WER** scorer (self-tested 0% on perfect input).
- **Target: WER < 8% (Whisper), < 15% (Vosk)** (PDR §9.1).

```bash
python generate_asr_benchmark.py               # write 500 references
python generate_asr_benchmark.py --wer hyp.txt # score your hypotheses
```

## 4 · Wake-Word Data — "Hey OSAIMA" (≥50)
- The **only dataset requiring hardware** (your microphone). openWakeWord trains
  on real voice clips.
- `record_wakeword.py` captures 16 kHz mono PCM16 clips into `positives/` / `negatives/`.
- Full capture → augment → train → validate flow in **`asr/WAKEWORD-GUIDE.md`**.
- **Target: false wake-word rate < 1 per hour idle** (PDR §9.1).

```bash
python record_wakeword.py --label positive --count 60
python record_wakeword.py --label negative --count 150
```

## 5 · Prompt-Injection Red-Team Set (50)
- 50 adversarial prompts across **10 attack vectors**: clipboard / email / webpage /
  filename / document injection, role-override jailbreaks, canary probes,
  direct-destructive, obfuscation (base64/hex/IFS), confused-deputy.
- Labels per row: `vector`, `injected_context`, `expected_behavior`
  (`REFUSE` ×43 / `CONFIRM_IRREVERSIBLE` ×7), `expected_action`.
- **Target: 0 successful injections** (PDR §9.1). Drives the security-layer test suite (§4).

## 6 · RAG Knowledge Corpus
- Seed docs in `rag-engine/seed_corpus/` (OSAIMA FAQ, Linux cheatsheet); extend
  with user notes, man pages (via `pandoc`), project READMEs.
- `chunker.py` — recursive splitter, **512-token chunks / 50-token overlap**,
  fenced code blocks preserved intact (PDR §3.5, §5.5).
- Downstream: embed with `all-MiniLM-L6-v2` → ChromaDB → top-5 cosine + cross-encoder re-rank (see `retriever.py`, `indexer.py`).

```bash
python chunker.py seed_corpus/ -o chunks.jsonl
```

## 7 · Behavior Store (runtime + seed)
- Real data accrues at runtime; `seed_behavior.py` builds a plausible 30-day
  developer profile for demos/tests.
- `schema.sql` tables: `interactions` (append-only), `style_rules` (+ChromaDB
  embeddings), `workflows` (detected sequences), `app_frequency`.
- Seed: ~400 interactions, 5 style rules, 1 detected workflow, 6 apps.

```bash
python seed_behavior.py            # build behavior.db
python seed_behavior.py --report   # print learned summary
```

---

## Intent-router training pipeline (datasets → model)
```bash
# 1. generate corpus (template OR llm)
python generate_dataset_llm.py --per-intent 250
# 2. stratified 80/10/10 split (all 40 intents in every split)
python split_dataset.py --input dataset_llm.jsonl
# 3. fine-tune BERT-tiny (~15 MB, >92% target, ~30 ms CPU)
python train_bert_tiny.py --epochs 5 --export-onnx
# 4. sanity-check one utterance
python train_bert_tiny.py --predict "open firefox"
```

## Reproducibility
- All offline generators use fixed seeds (`42`, `2025`, `7`, `11`) → identical
  output every run.
- LLM generator (`generate_dataset_llm.py`) is stochastic by design
  (`temperature=1.0`); pin a seed in Ollama options if exact reproduction needed.

## What the assistant could NOT produce
- **Wake-word audio (#4)** — needs a microphone. Script + guide provided instead.
- **Real WER numbers (#3)** — needs recorded speech. Reference text + scorer provided.
- **Real behavior data (#7)** — accrues from actual OS use. Schema + realistic seed provided.
