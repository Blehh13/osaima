# OSAIMA distilled intent model: status and results

**Status (2026-10-22): dataset, training, export, serving and the daemon path are built and tested.
The fine-tuned model is not trained yet.** This machine has no GPU (Ryzen 5 3550H, 8 threads, 9 GB, and
it was shared with other jobs, so everything here ran at `nice 19` on 2 threads). One CPU training step
of Qwen2.5-0.5B (2 examples) took 27–53 s, so 3 epochs would take days. Training runs on the RTX 4060
laptop or Colab (`train_colab.ipynb`). Until a trained model meets the bar below, the local-first route
is **off by default** (`OSAIMA_FAST_INTENT`).

## Pipeline

| Step | File | What it does |
|---|---|---|
| Dataset | `build_dataset.py`, `teacher.py` | Router data → (request → compact intent JSON) in the daemon's shape |
| Train | `train.py`, `train_colab.ipynb` | LoRA r=16 on all linear layers. CUDA bf16/fp16 or CPU fp32, optional Unsloth. Only answer tokens count toward the loss. Merges the adapter at the end |
| Export | `export.sh` | `convert_hf_to_gguf.py` → f16 → `llama-quantize` Q4_K_M (398 MB for 0.5B) |
| Serve | `../llm/start-intent-llm.sh` | Second llama-server on 127.0.0.1:8081, ctx 2048, model always loaded |
| Daemon | `mcp-daemon/src/llm.rs` `fast_intent*`, `main.rs` `local_first` | Short prompt plus a JSON-schema grammar, so output is always valid JSON with a known intent |
| Benchmark | `bench.py`, `compare.py` | Accuracy and latency on `data/test.jsonl` |

### Dataset (`data/`, rebuilt by `python build_dataset.py`; teacher answers are cached in `cache/`)

* **Requests.** Training requests come from `intent-router/{train,val,dataset_llm}.jsonl`: 36 per router label
  across 66 labels, de-duplicated, with any text that appears in the test set removed. Test requests are
  6 per label from `testset.jsonl`, the independently written paraphrase set. The router never trained on it.
* **Gemini-written requests** cover what the router data lacks: automations (time and event triggers),
  desktop settings, typed file operations, multi-step `orchestrate`, `task`, chat, unclear input and
  questions. Each batch is split 85/15 into train/test.
* **Labels.**
  * **Rules (1277).** `slots.py`, the router's own extractor, labels the shell-handled and OS-control intents.
  * **Teacher (1457 + 740 written).** Gemini labels everything else. Each call carries the daemon's full
    `intent.txt` and 40 requests. That took 62 calls in total: 57 answered by `gemini-3.5-flash-lite` and 5 by
    `gemini-3.5-flash`, the model tried first, which was often busy.
* **Totals.** Train has 2964 examples (1100 rule, 1238 teacher, 626 written). Test has 510 (177 rule, 219
  teacher, 114 written). Both splits cover all 39 intents.
* **Target format.** Each target is `{"intent","params","reply"}` with a reply of 15 words at most.
  * `generate_text` carries only a title. The writer model writes the text afterwards.
  * Notes whose content must be composed get `"write": true` instead of content.
  * Measured with Qwen's tokenizer: the system prompt is 351 tokens and an answer averages 29 tokens
    (95th percentile 55).

## Benchmark (held-out `data/test.jsonl`)

Scoring:
* **intent:** the intent name matches.
* **intent+params:** after normalising numbers and case, the params match too. Free text the user did not
  dictate is not compared: titles, plans, goals, shell commands and notification wording.

Latency is the wall time of one request at a time on this CPU, at `nice 19` with 2 threads, while other
jobs were running (load 3–13). An idle machine with 7 threads will be faster.

| Backend | n | intent | intent+params | valid JSON | p50 | p90 |
|---|---|---|---|---|---|---|
| Router alone (MiniLM, fast path) | 510 | 23.9% | 23.9% | answers 29% | **41 ms** | 47 ms |
| ↳ when it answers | 148 | 82.4% | 82.4% | — | | |
| Qwen2.5-0.5B **not fine-tuned**, short prompt + grammar | 510 | 27.3% | 13.7% | 100% | **1.65 s** | 2.27 s |
| **Distilled Qwen2.5-0.5B (LoRA)** | 510 | *to run after training* | | | ≈1.65 s (same size) | |
| Phi-4-mini, full prompt (llama.cpp CPU) | 15 | 40.0% | 33.3% | 100% | 47 s | 244 s |
| Gemini 3.5 Flash-Lite, full prompt | 40 | 72.5% | 67.5% | 100% | 4.6 s | 7.2 s |

Rows scored on the same requests (`python compare.py ...`):

| Same 15 requests | intent | intent+params | p50 |
|---|---|---|---|
| Phi-4-mini full prompt | 40.0% | 33.3% | 47 s |
| Gemini Flash-Lite | 46.7% | 46.7% | 4.5 s |
| Qwen 0.5B not fine-tuned | 33.3% | 20.0% | 1.6 s |
| Router alone | 20.0% | 20.0% | 41 ms |

| Same 40 requests | intent | intent+params | p50 |
|---|---|---|---|
| Gemini Flash-Lite | 72.5% | 67.5% | 4.6 s |
| Qwen 0.5B not fine-tuned | 30.0% | 12.5% | 1.7 s |
| Router alone | 27.5% | 27.5% | 42 ms |

How to read these numbers:

* **The test set is hard on purpose.** The router counts an abstention as a miss, and 71% of the set is
  requests the router is not built to settle.
* **The labels favour Gemini.** Most teacher labels come from Gemini Flash-Lite, the same model as the Gemini
  row. The other 35% of labels come from the router's own dataset, where even Gemini agrees on only about 67%.
  Some of those labels are debatable: "Just shut everything down, I need space" is labelled `minimize_all`.
  Read the distilled model's score as agreement with this teacher and these rules.
* **Latency is the clear win.** The 0.5B model with the 351-token prompt and the grammar answers in about
  1.6 s on 2 niced threads. Phi-4-mini with the 3,900-token prompt takes 30–245 s per request on the same
  threads, and its first, cold request took 389 s. Fine-tuning does not change the model's size, so the
  distilled model's latency will match the untuned row.
* **Accuracy is the open question.** The untuned row shows what the short prompt does without training: about
  27% intent accuracy. Training must lift that to at least 90%.

## Default routing and the bar

Code path when `OSAIMA_FAST_INTENT=on` (environment variable or `~/.config/osaima/llm.env`):

1. The intent router (about 40 ms) answers what it can.
2. Otherwise the request goes to the **distilled model** on port 8081, with a grammar-constrained JSON answer.
   This covers both the classify and the command tasks.
3. Only long-form text (`generate_text`, or a note with `write: true`) goes to the writer. That is Gemini when
   the user turned on **Fast mode** (`OSAIMA_LLM_MODE=cloud`, now saved by Settings), otherwise Phi-4-mini on
   the device.
4. If the intent model is off or unreachable, or gives an answer that is missing a required parameter, the
   request falls back to the old full-prompt path. Requests that carry outside content (clipboard, selected
   files, diagnostics) always use the full prompt and its injection guard.

With the setting on, a Gemini key alone no longer makes cloud the default: Fast mode must be chosen.

**The bar:** at least 90% intent accuracy on `data/test.jsonl` and p50 under 2 s on CPU. If the trained model
meets it, set `DEFAULT_FAST_INTENT = true` in `mcp-daemon/src/llm.rs` and start `start-intent-llm.sh` with the
desktop. If 0.5B falls short, train `--model Qwen/Qwen2.5-1.5B-Instruct` (about 1.1 GB in Q4_K_M, roughly 2–3× slower).

## Reproduce

```bash
cd ai-core/intent-model
python3 build_dataset.py                       # cached; calls Gemini only for missing labels
# GPU machine (or train_colab.ipynb):
python train.py --epochs 3 --batch 16          # add --unsloth if installed
./export.sh out/qwen0.5b osaima-intent-qwen0.5b
cp out/qwen0.5b/osaima-intent-qwen0.5b-Q4_K_M.gguf ~/.local/share/osaima/models/
../llm/start-intent-llm.sh &
python3 bench.py distilled --name distilled-qwen0.5b
python3 compare.py phi4-mini-full distilled-qwen0.5b router gemini-flash-lite
```

## What the ISO needs

| File | Size | Notes |
|---|---|---|
| `osaima-intent-qwen0.5b-Q4_K_M.gguf` in `/usr/share/osaima/models` (or the live medium) | ~398 MB | Release asset, not in git |
| `ai-core/llm/start-intent-llm.sh` | 2 KB | Run next to `start-llm.sh`, port 8081 |
| the existing `llama-server` build | — | Already shipped for Phi-4-mini |
| `OSAIMA_FAST_INTENT=on` (or the default flipped) | — | |

At run time it adds about 0.5 GB of resident memory (weights plus a 2048-token KV cache).

## Daemon run: 15 real requests (sanity model, not fine-tuned)

`mcp-daemon` (debug build) with `OSAIMA_FAST_INTENT=on`, the router service, the 6-step sanity model on 8081 and Phi-4-mini on 8080 as the writer, no Gemini key. The path works end to end: router hits return in under 100 ms, everything else returns grammar-valid JSON from the intent model in about 2 s (the first request took 8.7 s, a cold prompt cache), and long-form text goes to Phi-4-mini. The intents are mostly wrong, as expected from an untrained model; this is the run to repeat with the trained GGUF.

| request | intent (params) | route | ms |
|---|---|---|---|
| turn on night light | os_action {"action": "night_light"} | llm/local-intent/fast_intent | 8693 |
| set the volume to 35 percent | os_action {"action": "volume", "op": "set", "value": 35.0} | fast | 76 |
| every weekday at 7am set brightness to 60 | set_gaps {"pixels": 60} | llm/local-intent/fast_intent | 2296 |
| when my battery drops below 15% turn on do not disturb | os_action {"action": "power", "op": "low"} | llm/local-intent/fast_intent | 2562 |
| move ~/Downloads/report.pdf to my Documents folder | workspace {"number": 3} | llm/local-intent/fast_intent | 2098 |
| find pdfs bigger than 20 mb | files {"op": "search/list/open/reveal/copy/move/rename/trash/mkdir/undo/list | llm/local-intent/fast_intent | 3058 |
| close everything and open the terminal | close_all_windows {} | llm/local-intent/fast_intent | 1652 |
| restart the nginx service | os_action {"action": "restart"} | llm/local-intent/fast_intent | 1808 |
| set up a python flask project in ~/projects/blog with a virtualenv | create_note {"title": "Blog Project"} | llm/local-intent/fast_intent | 2467 |
| remind me to drink water | reminder {"text": "drink water"} | fast | 69 |
| schedule a meeting with Akash tomorrow at 3 pm | task {"goal": "schedule_meeting_with_akash_at_3_pm"} | llm/local-intent/fast_intent | 2569 |
| what's the weather like | weather {} | fast | 89 |
| tell me a joke | generate_text {"title": "Funny Joke"} | llm/local/fast_intent | 230242 |
| asdf qwer zxcv | layout {"layout": "monocle"} | llm/local-intent/fast_intent | 2182 |
| explain what a kernel module is | generate_text {"title": "Definition of Kernel Module"} | llm/local/write | 45365 |
