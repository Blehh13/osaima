# OSAIMA — Implementation Audit vs. "Dataset Identification" Slide

**Scope:** every dataset card and every "Pretrained Model" box on the Dataset
Identification slide, checked against what actually exists in the repo *after tonight's work*.
**Verdict key:** ✅ built & present · 🟡 partial · ❌ not present · ⚠️ claim overstated

> **Good news first:** unlike the Methodology slide, **most of this slide is now real** —
> the seven datasets were generated tonight and live in `ai-core/`. The two weak spots are
> (a) the **"Pretrained Models Used"** row (4 of 5 are *not* used in code), and (b) the
> **"on-device / your data stays with you"** tagline, which still contradicts the cloud-LLM
> daemon (`llm.rs` calls Groq/Gemini). See the Methodology audit for that.

---

## Part A — "Datasets We Build / Curate" (7 cards)

| # | Slide card | What exists in repo | Input | Output | Verdict |
|---|-----------|--------------------|-------|--------|---------|
| 1 | Intent-router training set — 10,000 synthetic, 3-track+40-intent, 500 edge manual | `intent-router/dataset.jsonl` (10k, template) **+** `dataset_llm.jsonl` (**full 10,000 real Ollama/Llama-3.1-8B run completed** — all 40 intents ×250, 0 duplicates) → split into train 8k/val 1k/test 1k | intent taxonomy | JSONL `{id,utterance,track,intent,edge_case}` | ✅ |
| 2 | Intent test set — held-out 1,000, target >92% | `intent-router/testset.jsonl` (1,000) + `test.jsonl` split | taxonomy | JSONL | ✅ built · ✅ **BERT-tiny trained on it → 91.3% measured** (0.7% under the 92% target) |
| 3 | ASR benchmark — 500, tech/Linux vocab, WER <8%/<15% | `asr/transcripts.jsonl` + `prompts.txt` + WER scorer | 5 vocab banks | 500 reference transcripts | 🟡 **text only** — WER needs recorded audio you don't have yet |
| 4 | Wake-word — ≥50 "Hey OSAIMA", train openWakeWord | `asr/record_wakeword.py` + `WAKEWORD-GUIDE.md` only | your mic | (would be) WAV clips | ❌ **no audio captured** — script + guide exist, hardware-blocked |
| 5 | Prompt-injection red-team — 50, target 0 injections | `security/redteam_prompts.jsonl` (50, 10 vectors) | — | labelled adversarial prompts | ✅ built · 🟡 "0 injections" is a **target** — the defences it tests aren't implemented (see Methodology audit §8) |
| 6 | RAG knowledge corpus — notes/man-pages/READMEs, 512/50 chunks | `rag-engine/seed_corpus/` (2 docs) + `chunker.py` (512/50, code-safe) + real `indexer.py` | docs | `chunks.jsonl` / Chroma vectors | 🟡 seed + chunker real; **man-pages not ingested**, indexer path hard-coded to `~/Desktop/osaima` |
| 7 | Behavior data (runtime) — logs/style-diffs/workflows/freq, SQLite+ChromaDB | `behavior/schema.sql` + `seed_behavior.py` → `behavior.db` (406 interactions, 5 style rules, 1 workflow, 6 apps) | usage events | SQLite DB | 🟡 **seed only** — real runtime capture not wired into the daemon |

**Card-by-card notes**
- **#1** is the strongest claim and it's true twice over — a deterministic template set *and* a
  real LLM-synthesised path (now that Ollama + Llama-3.1-8B are installed).
- **#2/#5** say "target >92%" / "0 injections" — correct to phrase as **targets**; just don't
  present them as achieved numbers, because no classifier is trained and no injection defence
  is coded yet.
- **#3** the corpus is real *text*; the **WER metric** requires you to read the 500 prompts
  aloud and record them. Say "reference corpus built; WER pending audio capture."
- **#4** is the one card with **nothing generated** — audio needs your microphone.
- **#6** "chunked 512 tokens / 50-token overlap" ✅ matches both `indexer.py` and `chunker.py`.
- **#7** the DB exists and is queryable (`python seed_behavior.py --report`), but it's seeded
  synthetic data, not learned from real use.

---

## Part B — "Pretrained Models Used" (⚠️ mostly *planned*, not *used*)

The header says **Used**. In the actual code, only one of these five is used.

| Model box | Slide claim | Reality in code | Verdict |
|-----------|-------------|-----------------|---------|
| **Vosk** `vosk-model-en-us-0.22` (~1.8 GB) | ASR | **Not referenced anywhere in code.** No ASR subsystem exists | ❌ not used |
| **Whisper** large-v3 | ASR (GPU) | Not referenced in code | ❌ not used |
| **LLM Models** Llama-3.1-8B (Q4_K_M) / Mistral-7B / Phi-3-mini | reasoning core | Llama-3.1-8B is now installed **for the dataset generator only**; the **daemon uses cloud Groq/Gemini**, not these | ⚠️ used for data-gen, **not** as the runtime LLM |
| **BERT-tiny** (fine-tuned for intent, ~15 MB) | intent classifier | ✅ **Now trained** → `intent_bert_tiny/`, 40-intent, **91.3% test accuracy**. Not yet wired into the Rust router, but the model exists and predicts correctly | ✅ trained (wiring pending) |
| **Embeddings & Re-rank** — `all-MiniLM-L6-v2` (80 MB, 384-dim) + cross-encoder `ms-marco-MiniLM-L-6-v2` | RAG embed + re-rank | **MiniLM ✅ used** in `indexer.py`/`retriever.py`. **cross-encoder ❌ not used** — no re-ranker in code | 🟡 half |

**Honest reframing for this row:** title it **"Pretrained Models (planned)"** or split it:
- **Actually used now:** `all-MiniLM-L6-v2` (RAG embeddings).
- **Used for tooling:** Llama-3.1-8B (offline dataset synthesis).
- **Planned / not yet integrated:** Vosk, Whisper, BERT-tiny, cross-encoder re-ranker.

---

## Part C — Word-by-word check of slide text
- **Subtitle** "curated, synthetic and real-world data" — accurate: synthetic (#1,#2), curated
  (#3,#5,#6), runtime/real-world (#7). ✅
- **"OSAIMA — ON-DEVICE. YOURS."** and **"Your data. Your OS. Your way."** — the *data* is
  on-device ✅, but the *LLM* is cloud (Groq/Gemini) ⚠️, so "on-device" is only half true.
- **"Data that understands you, stays with you."** — true for the datasets/behavior DB (all
  local files); **not** true for LLM inference (leaves the device).
- **"generated by the LLM itself"** (#1) — now literally true via `generate_dataset_llm.py`
  (Llama-3.1-8B). ✅
- **"500 edge cases labelled manually"** — in code the 500 are **flagged** `edge_case=true`;
  they are *seeded* ambiguous cases, not yet hand-labelled by a person. Say "flagged for
  manual review." 🟡
- **"Stored in SQLite / ChromaDB (on-device)"** (#7) — SQLite ✅ (`behavior.db`); the
  ChromaDB half (style-rule embeddings) is **schema-referenced but not populated** yet. 🟡
- **"Chunked 512 tokens / 50-token overlap"** (#6) — ✅ matches code exactly.
- **"~15 MB" BERT-tiny / "80 MB, 384-dim" MiniLM / "~1.8 GB" Vosk** — sizes are correct for
  those models. ✅ (usage caveats above.)

---

## Part D — Summary scoreboard

| Item | Slide | Reality | Verdict |
|------|-------|---------|---------|
| 1 Intent train 10k | built | template + real-LLM | ✅ |
| 2 Intent test 1k | built | built (target unproven) | ✅ / 🟡 |
| 3 ASR 500 | built | text only, WER pending audio | 🟡 |
| 4 Wake-word ≥50 | built | not captured (needs mic) | ❌ |
| 5 Red-team 50 | built | built (defences absent) | ✅ / 🟡 |
| 6 RAG corpus | built | seed+chunker; path bug, no man-pages | 🟡 |
| 7 Behavior data | built | seed DB only, not runtime | 🟡 |
| Models "Used" | 5 models | only MiniLM truly used | ⚠️ |
| "On-device / yours" | privacy | data local, **LLM cloud** | ⚠️ |

**One-liner:** the **dataset half of this slide is largely real now** — 6 of 7 datasets exist
as files (only wake-word audio is missing), which is a genuine, defensible deliverable. The
**overstatements to fix** are: (1) "Pretrained Models **Used**" — 4 of 5 aren't wired in yet,
(2) targets (#2 >92%, #5 zero-injection) shouldn't be shown as achieved, and (3) "on-device"
is true for data but not for LLM inference. Fix those three and the slide is honest.
