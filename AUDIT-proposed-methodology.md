# OSAIMA — Implementation Audit vs. "Proposed Methodology" Slide

**Scope:** every subsystem/claim on the Proposed Methodology slide, checked word-by-word
against the actual code in this repository.
**Verdict key:** ✅ implemented · 🟡 partial · ❌ not implemented · ⚠️ contradicts the slide

> **UPDATE (2026-09-17):** since this audit, three gaps closed — (1) the **BERT-tiny intent
> router is now trained** (91.3% test acc, `intent_bert_tiny/`); (2) **voice (F9 push-to-talk)
> merged** from Akash's branch (`voxtype.js` wired); (3) the **RAG hard-coded paths are fixed**
> (now use `OSAIMA_ROOT`/relative). Still open: LLM is **still cloud** (Groq/Gemini in `llm.rs`
> — swap to Ollama pending), and the leaked Groq key + `secret.txt` still need removal.

> ⚠️ **CRITICAL — before anything else (secrets committed to the repo):**
> 1. `ai-core/mcp-daemon/src/llm.rs:16` — a **hard-coded Groq API key** (`gsk_…(redacted)`).
> 2. `test_gemini.js:1` — a **hard-coded Google Gemini API key** (`AQ.…(redacted)`).
> 3. `secret.txt` — contains `The secret OS codeword is GALAXY42`.
> These are live credentials in version control. **Revoke/rotate both API keys now**, delete
> them from the code, load from env vars only, and remove `secret.txt`. If this repo was ever
> pushed anywhere, treat both keys as compromised.

---

## 0. The headline contradiction

The slide's tagline is **"Local. Private. Yours."** and it claims an on-device LLM
("Llama-3.1-8B (primary) / Mistral-7B / Phi-3-mini … via Ollama").

**The implemented daemon does the opposite.** `llm.rs` calls **cloud APIs**:
- **Primary:** Groq — `https://api.groq.com/...` (model `qwen/qwen3.8-27b`)
- **Fallback:** Google Gemini — `https://generativelanguage.googleapis.com/...` (model `gemini-3.5-flash`)

So every LLM request (intent classification, command generation, proactive suggestions)
leaves the device. ⚠️ This directly contradicts "Local. Private. Yours." and the
on-device-LLM claim. *(Side note: `gemini-3.5-flash` and `qwen/qwen3.8-27b` are not valid
model IDs — these calls would 404 as written, so the pipeline likely doesn't run end-to-end
without fixing the model names or wiring in Ollama.)*

The **Ollama work you just built** (`generate_dataset_llm.py`, the 8B model) is the thing
that would actually make the slide true — but it is **not wired into the daemon** yet.

---

## 1. System Pipeline (top row of the slide)

Slide shows: `Voice/Text → ASR Daemon → Intent Router → LLM Engine → Action Executor →
OS/Desktop`, with RAG + Behavior feeding the LLM.

| Stage | Slide says | Reality | Verdict |
|-------|-----------|---------|---------|
| Voice / "Hey OSAIMA" | Wake-word + speech in | **No ASR/wake-word code exists** anywhere in the repo | ❌ |
| Text in | — | Text utterance accepted over JSON-RPC (`system.dispatch_intent`) | ✅ |
| ASR Daemon | Vosk/Whisper STT | Not implemented (only the *dataset* scripts you built) | ❌ |
| Intent Router | fast vs LLM path | `router.rs`: 4 regexes vs LLM — real but tiny (see §3) | 🟡 |
| LLM Engine | on-device Llama | **Cloud Groq/Gemini** (see §4) | ⚠️ |
| Action Executor | D-Bus/AT-SPI/PTY/LSP | Only media + notifications D-Bus, + sandboxed bash (see §6) | 🟡 |
| RAG Engine | feeds context | Real MiniLM+Chroma, but broken paths (see §5) | 🟡 |
| Behavior Store | user profile | Not the PDR store; only a 15s suggestion loop (see §7) | 🟡 |

---

## 2. Subsystem 1 — ASR Daemon (Speech-to-Text)
- **Slide:** Vosk (CPU ~500 ms) / Whisper large-v3 (GPU); wake-word "Hey OSAIMA"
  (openWakeWord); PipeWire capture + WebRTC VAD; Whisper fallback if Vosk conf < 0.75.
- **Reality:** ❌ **None of this exists in code.** No Vosk, Whisper, openWakeWord, PipeWire,
  or VAD anywhere. The only ASR-related artifacts are the *benchmark dataset + capture
  script* you built tonight (`ai-core/asr/`), which are test data, not a running daemon.
- **Input (intended):** microphone PCM. **Output (intended):** `{transcript, confidence, ts}`.
- **Used for:** turning voice into text. **Status: vision only.**

## 3. Subsystem 2 — Intent Router (Decision Engine)
- **Slide:** decides in <200 ms; fast path (direct D-Bus) vs slow path (LLM); three tracks
  (System <200 ms / Workflow <8 s / Content <12 s).
- **Reality (`router.rs`):** 🟡 partial and different.
  - **Fast path = 4 hard-coded regexes only:** `play|resume`, `pause|stop`, `next|skip`,
    `notify …`. Everything else falls to the slow path.
  - **Slow path:** fetch RAG context → ask the (cloud) LLM for a bash command → run it
    sandboxed.
  - **No BERT-tiny, no 40-intent taxonomy, no 3-track split, no <200 ms measurement.**
    The "three tracks" on the slide are **not** what the router does.
  - There is a *separate* LLM-based classifier in `llm.rs::classify_intent` with **~28
    desktop intents** (timer, reminder, open_app, layout, workspace, orchestrate,
    optimize_cpu, …) — a different taxonomy again, and different from the PDF's 40 and from
    your dataset's 40.
- **Input:** utterance string. **Output:** `RoutingDecision::FastPathDBus` or `SlowPathLLM`.
- **Tech:** `regex` crate. **Used for:** routing. **Gap:** the BERT-tiny model you have a
  training script for is the missing piece that would make this match the slide.

## 4. Subsystem 3 — LLM Engine (Reasoning Core)
- **Slide:** Llama-3.1-8B / Mistral-7B / Phi-3-mini via **Ollama**; structured system prompt
  with CWD + app context + behavior profile + tool-call JSON schema + safety rules.
- **Reality (`llm.rs`):** ⚠️ **Cloud, not local.**
  - Groq (`qwen/qwen3.8-27b`) primary, Gemini (`gemini-3.5-flash`) fallback. `reqwest` HTTP.
  - System prompt is real but **only for the ~28-intent classifier** and for a "generate a
    bash command" prompt — **no** behavior profile, **no** tool-call schema, **no** RAG in
    the classify prompt (RAG is only added on the bash-generation path), safety rules absent.
  - Hard-coded Groq key (see top warning). Invalid model IDs.
- **Input:** prompt / query string. **Output:** text, or intent JSON.
- **Used for:** classification + command generation. **Gap:** swap to Ollama, fix model IDs,
  add the structured prompt sections.

## 5. Subsystem 4 — RAG Engine (Knowledge Retrieval)
- **Slide:** MiniLM embeddings; ChromaDB vector store; top-5 cosine + cross-encoder re-rank;
  indexes user notes, man pages, project READMEs, local docs.
- **Reality (`rag-engine/indexer.py`, `retriever.py`):** 🟡 real embeddings, three defects.
  - ✅ `all-MiniLM-L6-v2` (384-dim) → **ChromaDB** persistent, cosine (`hnsw:space:"cosine"`),
    512/50 chunking. Genuine vector search.
  - ❌ **top-3, no cross-encoder re-rank** (slide/PDR say top-5 + `ms-marco-MiniLM` re-rank).
  - ✅ **Hard-coded paths FIXED** (Akash merge, 2026-09-17): indexer/retriever/router now use
    `OSAIMA_ROOT` env + relative fallback. *(Was: `~/Desktop/osaima` and
    `/home/triquetra/…`, which broke on `D:\osaima`.)*
  - ⚠️ **Leak risk:** indexer globs `**/*.txt` over the whole base dir — it would embed
    `secret.txt` into the vector DB.
- **Input:** query string (+ indexed corpus). **Output:** `{context:[{text,metadata}]}` JSON.
- **Used for:** grounding the LLM. **Gap:** configurable path, add re-ranker, scope the glob.

## 6. Subsystem 5 — Action Executor (OS Interface)
- **Slide:** bridges AI to real actions via 4 interfaces — ① D-Bus (launch, media, network,
  systemd) ② AT-SPI (widget control) ③ PTY injection (terminal) ④ LSP (IDE code actions).
- **Reality:** 🟡 only a slice of ① exists; ②③④ absent.
  - **`dbus_control.rs` (zbus):** ✅ MPRIS2 media (`play_pause/next/previous/stop`, hard-coded
    to `…spotify`) and ✅ `org.freedesktop.Notifications`. ❌ **No** app-launch, **no**
    NetworkManager, **no** systemd control.
  - **`executor.rs`:** ✅ `run_sandboxed` = `systemd-run --user --scope
    CapabilityBoundingSet= PrivateTmp=yes NoNewPrivileges=yes -- bash -c` (matches PDR §4.2).
    Also `run_trusted` = plain `sh -c` (no sandbox).
  - **AT-SPI ②, PTY ③, LSP ④:** ❌ not present in the daemon.
  - **MCP servers** (`ai-core/mcp-servers/*.py`, real MCP SDK, stdio): `filesystem` (read_file,
    list_directory, search_files), `process` (list_processes, kill_process), `network`
    (get_network_interfaces, ping_host), `system_monitor` (cpu/mem/disk), `terminal`
    (run_command), `ui_automator` (inspect_window, execute_ui_macro). ✅ These exist and map
    to PDR §8.2 — **but the daemon has no MCP host code and never spawns them**, so they run
    only standalone. 🟡 not integrated.
- **Input:** action request (utterance→LLM→bash, or direct media/notify). **Output:** command
  stdout/stderr, or D-Bus effect. **Used for:** executing on the OS.

## 7. Subsystem 6 — Behavior Store (Learning & Adaptation)
- **Slide:** SQLite + ChromaDB; learns writing style, commit format, code-scaffold prefs,
  workflow sequences, app frequency; injected into the LLM as style rules.
- **Reality (`agent.rs`):** 🟡 **not the PDR store.** A 15-second loop reads CPU/mem and asks
  the cloud LLM for a UI suggestion (e.g. "CPU high → open Task Manager?"), caching the last
  5. **No** SQLite, **no** style learning, **no** commit/scaffold prefs, **no** workflow
  detection, **no** app-frequency table, **no** style-rule injection.
  - The actual PDR-style store is what **you seeded tonight** (`ai-core/behavior/schema.sql`,
    `seed_behavior.py`) — not yet wired into the daemon.
  - The web shell has a separate `behavior.js` (localStorage) — demo only.
- **Input:** system stats + past suggestions. **Output:** suggestion JSON. **Used for:**
  proactive nudges.

---

## 8. Security & Safety (Addressing Gap 5)

| Slide claim | Reality | Verdict |
|-------------|---------|---------|
| Risk tiers (read-only / reversible / irreversible) | **Not implemented.** LLM-generated bash runs immediately (sandboxed) with no tier check or confirmation | ❌ |
| systemd sandbox (runs each subsystem in isolation) | `run_sandboxed` sandboxes the **generated command** (not "each subsystem"), correctly | ✅ (command-level) |
| AppArmor confinement | **No AppArmor profile in the repo** | ❌ |
| Prompt-injection defence (untrusted-context tagging, AST whitelist, canary token) | **None implemented.** RAG context is concatenated raw into the prompt; no tagging, no AST check, no canary | ❌ |
| SHA-256 audit log (all actions logged) | **No audit log code** | ❌ |

The 50-prompt red-team set you built (`security/redteam_prompts.jsonl`) is the *test* for
these defences — but the defences themselves are not in the code yet.

---

## 9. Word-by-word check of the slide's other text
- **Subtitle:** "turns you**r** voice into text into real actions" — typo "you" → "your".
  Also currently only *text* input works; *voice* does not (no ASR).
- **"Each runs as a systemd user service"** — only the **MCP daemon** has a systemd unit
  (`osaima-mcp-daemon.service`). ASR/router/LLM/RAG/behavior are **not** separate services;
  they're modules inside the one daemon (or absent).
- **"Vosk (CPU ~500 ms) / Whisper large-v3 (GPU)"** — not in code.
- **"Llama-3.1-8B (primary) … via Ollama"** — code uses Groq/Gemini cloud instead.
- **"Top-5 cosine similarity + cross-encoder re-rank"** — code is top-3, no re-rank.
- **"D-Bus (launch, media, network, systemd)"** — only media + notifications implemented.
- **"AT-SPI / PTY injection / LSP"** — none implemented.
- **"Local. Private. Yours."** — false as built (cloud LLM calls).
- **Date "16-09-2026"** — fine (today).

---

## 10. Summary scoreboard

| Subsystem | Slide | Built | Verdict |
|-----------|-------|-------|---------|
| ASR / wake-word | Vosk/Whisper/openWakeWord | nothing (only datasets) | ❌ |
| Intent Router | BERT-tiny, 3-track, <200 ms | 4 regexes + cloud-LLM classify | 🟡 |
| LLM Engine | on-device Ollama | **cloud Groq/Gemini** | ⚠️ |
| RAG | MiniLM+Chroma, top-5+re-rank | MiniLM+Chroma, top-3, broken paths | 🟡 |
| Action Executor | D-Bus+AT-SPI+PTY+LSP | media+notify D-Bus, sandboxed bash | 🟡 |
| Behavior Store | SQLite style-learning | 15 s cloud-LLM suggestion loop | 🟡 |
| Security | tiers+AppArmor+injection+audit | command sandbox only | ❌ mostly |
| MCP servers | API ecosystem | 6 servers, real, **not wired to daemon** | 🟡 |

**One-liner:** the repo is a **cloud-LLM desktop-assistant prototype** (Groq/Gemini + a small
regex/LLM router + partial D-Bus + real-but-unwired MCP servers + a real-but-mispathed RAG),
**not** the on-device voice-first pipeline the slide describes. Closing the gap = wire in
Ollama (you have it), add ASR + the BERT router + the SQLite behavior store + the risk/audit
security layer, fix hard-coded paths, and remove the committed secrets.
