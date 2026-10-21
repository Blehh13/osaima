# OSAIMA — Implementation Audit vs. "Algorithm, Implementation & Demonstration" Slide

**Scope:** the 10-step dispatch loop, the implementation stack, the 5 demonstration
scenarios, and the evaluation targets — each checked against the real code.
**Verdict key:** ✅ implemented · 🟡 partial · ❌ not implemented · ⚠️ contradicts the slide

> **Reality in one line:** the slide draws the full PDR algorithm (10 steps). The code
> (`router.rs`) actually does **3 of the 10**: a 4-pattern regex fast-path, else RAG →
> **cloud** LLM → sandboxed bash. Steps 1–2 (wake-word, ASR), 4 (BERT), 5 (structured prompt),
> 7 (risk class), 8 (confirmation), 10 (logging/behavior) are **not in the code**.

---

## Part A — "Core Algorithm — Dispatch Loop" (10 steps)

| # | Slide step | Reality in code | Verdict |
|---|-----------|-----------------|---------|
| 1 | Wake-word detection ("Hey OSAIMA", VAD segments audio) | **No wake-word / VAD code anywhere** | ❌ |
| 2 | ASR transcription (text, confidence, timestamp) | **No ASR** — entry point is a text `utterance` over JSON-RPC | ❌ |
| 3 | Keyword match (regex + trie, ~200 cmds, <50 ms) → D-Bus fast path | `router.rs`: **4 regexes** (`play/resume`, `pause/stop`, `next/skip`, `notify`). No trie, not ~200 commands | 🟡 |
| 4 | Intent classification (BERT-tiny, track/intent) | ✅ **BERT-tiny now trained** (`intent_bert_tiny/`, 40-intent, **91.3% test accuracy**) on the 10k dataset. Not yet wired into the Rust router, but the model exists and works. (A separate cloud-LLM classifier in `llm.rs` also exists.) | ✅ built (wiring pending) |
| 5 | LLM prompt construction (RAG context + behavior style rules + tool-call schema) | Slow path adds **RAG context only**; **no** behavior rules, **no** tool-call schema | 🟡 |
| 6 | LLM inference (generates tool-call JSON) | Generates a **bash command string** (not tool-call JSON), via **cloud Groq/Gemini** | 🟡⚠️ |
| 7 | Risk classification (AST: read-only/reversible/irreversible) | **Not implemented** — no AST, no tiering | ❌ |
| 8 | Confirmation gate (user approval if required) | **Not implemented** — command runs immediately | ❌ |
| 9 | Action executor (D-Bus / AT-SPI / PTY / LSP) | Fast path → D-Bus media/notify; slow path → **sandboxed bash** (`systemd-run`). No AT-SPI/PTY/LSP | 🟡 |
| 10 | Logging & learning (outcome + correction → Behavior Store) | **Not implemented** — no logging, no learning writeback | ❌ |
| — | "Feedback loop for continuous improvement" | Not present (no store to feed back into) | ❌ |

**Actual algorithm, honestly stated:**
```
text utterance (JSON-RPC)
  → 4-regex match?
      yes → D-Bus media/notify (fast path)
      no  → run retriever.py (RAG) → cloud LLM → bash string
            → systemd-run --scope sandbox → return stdout
```

---

## Part B — Implementation Stack

| Slide box | Slide lists | Reality | Verdict |
|-----------|-------------|---------|---------|
| **Core** | Python 3.11 + asyncio | asyncio Python in `mcp-servers/` + RAG; **the daemon itself is Rust/tokio**, not Python | 🟡 |
| **ASR / Audio** | vosk, sounddevice, webrtcvad, openwakeword | **None used** (only inside the dataset scripts you built) | ❌ |
| **LLM** | ollama, llama-cpp-python | ❌ **not used** — daemon uses `reqwest` → **Groq + Gemini cloud** | ⚠️ |
| **RAG** | chromadb, sentence-transformers, langchain loaders | ✅ all three in `indexer.py`/`retriever.py` (MiniLM + Chroma + LangChain) | ✅ |
| **Actions** | dbus-python, pyatspi, pexpect/ptyprocess, pylsp-jsonrpc | ❌ **none of these** — D-Bus is done in **Rust `zbus`** (media/notify only); no pyatspi/pexpect/lsp | ❌ (as listed) |
| **MCP layer (on demand)** | filesystem, process, dbus, terminal, browser (+Gmail/Spotify/Calendar) | ✅ 6 real servers exist (`filesystem/process/network/system_monitor/terminal/ui_automator`); ❌ **no browser/Gmail/Spotify/Calendar**; ⚠️ **daemon never spawns any of them** | 🟡 |
| **System Services** | systemd user services; Unix socket `/run/user/$UID/osaima/*.sock` | 🟡 one systemd unit for the daemon; socket is **`/tmp/osaima_mcp.sock`**, not `/run/user/$UID/...` | 🟡 |
| **Security** | systemd-run --scope, CapabilityBoundingSet="", PrivateTmp, NoNewPrivileges, AppArmor profile, SHA-256 audit log | ✅ the `systemd-run` sandbox (all 3 props) is exactly implemented; ❌ **no AppArmor profile**, ❌ **no audit log** | 🟡 |

---

## Part C — Demonstration Scenarios (5)

| # | Slide scenario | Can it run today? | Why |
|---|---------------|-------------------|-----|
| 1 | Voice → D-Bus: "open the Files app" → GNOME Files < 200 ms | ❌ | No voice (no ASR); D-Bus layer has **no app-launch** — only media play/pause/next + notifications |
| 2 | Voice → LLM → PTY: "set up a Django project … in the live terminal < 8 s" | 🟡 | No voice; no **PTY injection** — the slow path runs bash in a **detached systemd sandbox**, not the user's open terminal. Also cloud-LLM latency, not local |
| 3 | Voice → RAG → LLM: "how do I run the tests?" → reads README → correct command | 🟡 | RAG + LLM path is real, **but** retriever path is hard-coded to `/home/triquetra/...` and index path to `~/Desktop/osaima` → returns empty on this machine until fixed |
| 4 | Behavior learning: same task before/after 5 interactions → more on-style | ❌ | No behavior/style learning implemented (only tonight's seeded DB, unwired) |
| 5 | Security: prompt injection via clipboard → no unauthorized action | ❌ | No prompt-injection defence in code; the 50-prompt red-team set is the *test*, not the *guard* |

**Demoable today (honest set):** text-in → regex → **media control / desktop notification**
(scenario-ish), and text-in → **cloud LLM → sandboxed bash** returning output. If you fix the
RAG paths, scenario 3 becomes a genuine live demo. Scenarios 1, 4, 5 need real code first.

---

## Part D — Evaluation Targets (all are targets, none measured)

| Target | Slide | Reality |
|--------|-------|---------|
| Intent accuracy > 92% | goal | ✅ **MEASURED: 91.3%** (BERT-tiny, 1,000-utterance held-out test, F1-macro 0.912) — 0.7% under target |
| Fast-path latency < 200 ms | goal | Regex is fast, but **never benchmarked**; no timing code |
| Workflow completion < 8 s (TTFT < 2 s) | goal | Uses **cloud** latency, not local 8B; unmeasured |
| Workflow success > 80% | goal | **No test harness / user study** |
| False wake < 1 / hour | goal | **No wake-word** to measure |
| Prompt injections 0 successful | goal | **No defence** implemented to test |

Present these as **objectives**, not results — none has a number behind it yet.

---

## Part E — Word/phrase check
- **Subtitle** "locally, securely, intelligently" — **"locally" is false** (cloud LLM);
  "securely" only partly (sandbox yes; tiers/AppArmor/audit/injection no).
- **"OSAIMA — ON-DEVICE. YOURS."** / **"Speak. Build. Automate. Locally."** — same cloud-LLM
  contradiction; and **"Speak"** implies voice, which isn't implemented.
- **"BERT-tiny (track/intent)"** — not present; routing is regex + cloud-LLM.
- **"LLM inference generates tool-call JSON"** — code generates a **bash string**, not tool-calls.
- **"D-Bus / AT-SPI / PTY / LSP"** — only D-Bus (media/notify) exists.
- **"< 50 ms" keyword match / "< 200 ms"** — plausible for regex but unmeasured.
- **Socket path** on the slide vs code: `/run/user/$UID/osaima/*.sock` (slide) vs
  `/tmp/osaima_mcp.sock` (code).

---

## Part F — Summary scoreboard

| Layer | Slide | Built | Verdict |
|-------|-------|-------|---------|
| Dispatch loop | 10 steps | 3 (regex → RAG → cloud LLM → sandbox) | 🟡 3/10 |
| Wake-word + ASR | Vosk/Whisper/oWW | none | ❌ |
| Intent (BERT) | BERT-tiny | ✅ trained, 91.3% (wiring into router pending) | ✅ built |
| RAG stack | Chroma+MiniLM+LangChain | ✅ real (path bug) | ✅/🟡 |
| Action interfaces | D-Bus+AT-SPI+PTY+LSP | D-Bus media/notify only | 🟡 |
| Sandbox | systemd-run props | ✅ exact | ✅ |
| Risk/Confirm/Audit/AppArmor | all four | none | ❌ |
| MCP servers | 8 incl. cloud | 6 local, unwired | 🟡 |
| Demos | 5 scenarios | ~1.5 runnable (2,3 after path fix) | 🟡 |
| Eval targets | 6 metrics | 0 measured | ⚠️ targets only |

**One-liner:** the algorithm slide is the **aspirational full pipeline**; the code is a
**3-step cloud-LLM prototype** (regex fast-path + RAG-grounded bash generation inside a real
systemd sandbox). The RAG layer and the sandbox are the two parts that genuinely match the
slide. To make the rest true: add ASR + wake-word, train/wire the BERT router, switch LLM to
Ollama, build the risk/confirm/audit/AppArmor security layer, wire the MCP servers and the
behavior store, fix the hard-coded paths, and actually measure the six targets.
