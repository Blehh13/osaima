# Predictive models (on-device, pure Rust)

Everything here trains and runs inside the daemon (`ai-core/mcp-daemon`), with no network, no new crates and no pretrained weights. The models learn from the user's own interaction log and health readings.

| model | file | trained on | output |
|---|---|---|---|
| Next-action predictor | `src/predictor.rs` | `interactions` table | "you usually X now", with confidence and a one-line reason |
| Forecasts (disk, battery, thermal) | `src/forecast.rs` | rolling health history | "Disk full in about N days" etc., with evidence numbers |
| Anomaly detector | `src/forecast.rs` | rolling health history | "CPU unusually high for this hour", with z-score |

RPC: `system.insights` returns `{predictions, forecasts, anomalies, model_info}`; `system.suggestions` carries the same predictions, forecasts and anomalies so the assistant side rail shows them.

## 1. Next-action predictor

**Task.** Rank the next request, given local time of day, weekday, the previous one and two requests (a session ends after 30 idle minutes) and the apps launched in the last hour (a proxy for "open apps"; the log does not record closes).

**Model.** A stacked mixture of five distributions over the user's candidate commands:

1. prior: relative frequency;
2. time of day: circular Gaussian kernel density (sigma 30 min) over the times the command was used, recency-weighted (half-life 21 days), same weekday class counted fully and the other class at 0.3;
3. previous request: P(command | previous), Dirichlet back-off to the prior;
4. previous two requests: P(command | prev2, prev1), back-off to (3);
5. open apps: P(command | app launched in the last hour), back-off to the prior.

The five mixture weights are fitted by exhaustive search on a 0.1 grid, maximising held-out log-likelihood on the 60-80 % slice of the history (time order), then shrunk 15 % toward defaults so a lucky slice cannot zero a component. Counts update in O(1) per logged request; weights are refitted every 200 requests. Fit on ~260 events takes about 4 ms; one prediction about 0.01 ms.

**Explainability.** Each prediction carries the component that lifts it most above its base rate, and it is only used as a reason if it has real support (at least 2 observations in that context): "after you open the terminal you usually open the browser (18 of 25 times)" (illustrative). Predictions whose only driver is the global prior are dropped, as are those below 20 % confidence and the command just executed.

**Safety (unchanged from the SQL rule).** Only fast-path (on-device router) requests of at most 60 characters; never LLM plans, power actions, `shutdown`, `close_all_windows`, `clean_storage`, `delete_note`, `orchestrate` or `agent`. LLM-path requests are used as context only.

**Cold start.** Until the history has 40 safe requests, `predictions()` falls back to the original SQL heuristic, which stays in the code as `predictions_heuristic` and as the evaluation baseline.

### Evaluation

Synthetic but realistic histories (`src/predictor_eval.rs`): four personas (developer, student, office worker, irregular night owl), each a set of routine chains with start-time jitter (sd 10-90 min), skipped days and skipped steps, weekday/weekend and day-of-week routines, 4-8 random one-off requests per day from a Zipf-like pool of 12 commands, and 15 % of one-offs going through the LLM path. 28 days, 10 seeds per persona. Time-ordered split: 60 % train, 20 % tune weights, 20 % report; each reported request is predicted before it is learned. Baseline: the deployed SQL rule (commands used on at least 3 of the last 28 days within 45 minutes of now and not in the last 2 hours; empty answer counts as a miss).

Reproduce: `cargo test --release predictor_eval -- --nocapture --test-threads=1`.

| persona | held-out requests | model top-1 | model top-3 | SQL rule top-1 | SQL rule top-3 | confident coverage | precision when confident |
|---|---|---|---|---|---|---|---|
| developer | 46 | 45.2 ± 7.1 | 62.6 ± 3.9 | 38.4 ± 5.1 | 48.7 ± 5.1 | 63.9 | 55.8 |
| student | 52 | 40.3 ± 8.5 | 58.9 ± 8.4 | 37.4 ± 6.7 | 45.6 ± 6.5 | 51.8 | 54.7 |
| office worker | 38 | 49.0 ± 7.6 | 69.3 ± 5.6 | 42.1 ± 4.5 | 46.3 ± 3.9 | 89.2 | 51.0 |
| night owl (irregular) | 56 | 32.6 ± 4.8 | 53.8 ± 6.7 | 22.7 ± 4.0 | 39.9 ± 6.0 | 66.6 | 44.0 |
| **all** | | **41.8** | **61.2** | **35.2** | **45.1** | | |

(Percent; mean ± sd over seeds. Chance is about 1 in 16.)

Ablation, pooled personas:

| components | top-1 | top-3 |
|---|---|---|
| prior only (most frequent request) | 14.8 | 37.7 |
| prior + time of day | 23.2 | 55.8 |
| prior + previous 1-2 requests | 33.2 | 54.5 |
| prior + open apps | 22.0 | 45.0 |
| all except open apps | 42.4 | 61.2 |
| all five (deployed) | 41.8 | 61.2 |

Learning curve, pooled:

| days of history | model top-1 | model top-3 | SQL rule top-1 | SQL rule top-3 |
|---|---|---|---|---|
| 7 | 23.8 | 40.9 | 8.1 | 8.9 |
| 14 | 31.5 | 52.3 | 20.2 | 23.9 |
| 28 | 41.8 | 61.2 | 35.2 | 45.1 |
| 56 | 42.6 | 60.9 | 36.3 | 46.7 |

**Reading the numbers.** The model beats the rule by about 6-7 points top-1 and 16 points top-3, and the gap is largest with little history (the rule needs three distinct days before it says anything). Both time and sequence matter; sequence is the stronger single signal. The open-apps component adds nothing in this data (the generator has no app-dependent behaviour beyond what sequence already captures), so it is kept for real use but is not shown to help here.

### Limits

- The histories are synthetic. They were written by us, so the model's structure (chains, Gaussian jitter) matches how the data was made; real users are messier. Absolute accuracy on a real person is unknown until enough real history exists.
- The ceiling is low by design: 4-8 random requests a day are unpredictable, so top-1 around 40 % is not a failure of the model.
- The development machine's real `behavior.db` (1600 rows, 4 days) is evaluation-harness traffic with timestamps squeezed into a few days, not habits. Run against a copy, the model trains (tuned, 84 % top-1 on its last 20 %, which simply reflects that the harness replays prompts in a fixed order) and, correctly, makes no habit predictions at the probed times; the SQL baseline scores 0 there. These numbers should not be quoted as evidence of accuracy.
- "Open apps" is approximated by apps launched in the last hour; closing is not logged.
- Time of day uses the daemon's local clock; travel across time zones shifts habits until they are relearned.
- Mixture weights are fitted on ~20 % of the history; with fewer than about 10 usable validation samples the defaults are kept.

## 2. Health forecasting and anomaly detection

**History.** Every 20 s reading goes into a 3-hour in-memory ring; one every 5 minutes goes into a 30-day ring (8640 points) appended to `health_history.jsonl` beside the behaviour database (compacted when it exceeds the bound by 720 lines) and replayed at start-up. Metrics: CPU temperature, CPU pressure, memory used, disk used and total, battery percent and charging state.

**Forecasts (Theil-Sen: median of pairwise slopes, at most 120 points).**
- Disk: used bytes over the last 14 days (needs at least 2 days and 24 points). A sudden drop of at least 1 GB or 1 % of the disk (a cleanup) starts a new regime and only the data after it is fitted, because a step breaks even a robust slope when half of all pairs straddle it. Reports when the slope exceeds 0.05 GB/day, at least 60 % of pair slopes agree, and the disk would fill within 90 days. Level: bad at 3 days or less, warn at 14, otherwise info.
- Battery: percent over the current unbroken discharge (at least 10 minutes), reports time left when draining at least 1 %/h and under 8 h remain.
- Thermal: temperature over the last 15 minutes (at least 10 minutes of samples), reports when warming at 0.15 C/min or more, currently 65-90 C, and the 90 C throttling zone is under 30 minutes away at this slope.

**Anomalies.** Per metric (CPU pressure, memory, temperature) and local hour of day, an exponentially weighted mean and variance (alpha = max(1/n, 0.02)) is learned from the 5-minute history. A reading (median of the last three 20 s samples) is flagged at z at least 3 (warn at 5), with a floor on the deviation (8 points CPU, 4 points memory, 3 C temperature) and an absolute minimum (25 % CPU, 50 % memory, 60 C), and only once the hour has 36 samples (about 3 days). Flags show the value, the normal for this hour and z.

**Alert shape.** `{id, kind, level (info|warn|bad), title, detail, advice, evidence{...}, source:"model"}`, the same fields as the threshold alerts; the rail renders them in the same `as-alert` style with the detail line as evidence.

### Evaluation (unit tests with known answers, `src/forecast.rs`)

| test | truth | result |
|---|---|---|
| Theil-Sen on an exact line; with 20 % outliers of +5000 | slope 3 | exact; within 0.2 |
| Disk, 12 days, +2 GB/day with noise and a 15 GB cleanup on day 6 | days left = free / 2 | within 15 % (fails at 84 vs 25 days if the cleanup is not handled) |
| Disk flat, or only 100 points | no alert | no alert |
| Battery 60 % draining 20 %/h, integer percent | time left = percent / 20 | within 0.25 h |
| Battery charging, or draining 1 % per 27 min | no alert | no alert |
| Thermal 70 C warming 1 C/min with noise | minutes = 90 - T | within 2.5 min |
| Thermal steady at 80 C, or cool 40 C warming slowly | no alert | no alert |
| Anomaly: CPU 60 % at hour 10 is normal, 60 % at 03:00 is not | flagged only at 03:00 | yes; no false alarm at any of the 24 hours on normal readings |
| Memory 95 %, temperature 88 C at 03:00 | both flagged | yes |
| History persists, reloads, ring bounded | 3 points / 8640 cap | yes |

### Limits

- Evaluated only on synthetic series with a known generator; there is no real multi-week health recording from a real laptop yet, so false-alarm rates on real hardware are untested. The anomaly threshold is deliberately conservative to compensate.
- Linear trends only: a disk that fills in bursts, or a battery whose drain changes with load, will be extrapolated linearly. Battery time left is based on the last discharge only (up to 3 hours of samples).
- The thermal forecast assumes the throttling point is 90 C; real limits differ per CPU. Fan curves make a rising temperature plateau, so it can over-warn.
- The anomaly model has no knowledge of weekday versus weekend (hour-of-day only) and needs about three days per hour before it speaks.
- Disk used bytes come from `statvfs` on the home filesystem only.
