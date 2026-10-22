# osaima-schedd: a learning CPU scheduler

When many programs want the CPU, the kernel shares it out fairly. Fair is not what a
desktop wants: a video player or a game needs a steady share on time, an editor needs
to answer the moment a key is pressed, and a compile or an encode only cares about the
total. `osaima-schedd` watches how every program behaves, decides what kind it is, and
sets its CPU priority to match, so the desktop stays smooth while something heavy runs.

It is the project's "eBPF ML scheduler": eBPF supplies the measurements, a trained
model makes the decision, and the kernel's own scheduler carries it out.

```
 eBPF telemetry ──► features ──► classifier ──► policy ──► priority
 (scheduler         (5 numbers   (trained        (what each   (nice value and
  tracepoints)       per window)  model, or rules) kind gets)   scheduling policy)
```

## How it works

1. **Telemetry.** `bpf/telemetry.bpf.c` hooks the kernel's `sched_wakeup` and
   `sched_switch` tracepoints and records, per thread, the time on a CPU, the time spent
   waiting for one, how often it was given the CPU, and whether it gave the CPU up by
   itself (blocked) or lost it (preempted). It is built against the kernel's own type
   information (CO-RE), so one object runs on any kernel with BTF. Without it, the
   daemon reads the kernel's `/proc` accounting, which carries similar numbers (it also
   falls back to `/proc` by itself when the eBPF program cannot be loaded).
2. **Features.** Every few seconds, the change in the counters becomes five numbers per
   process: CPU used (in cores), voluntary and involuntary switches per second, the mean
   length of a run, and the time spent waiting relative to the time spent running.
3. **Classifier.** A logistic regression, trained on measurements of real programs
   (see Training), says whether the process is **interactive** (editors, shells),
   **media** (video, audio, games: steady work against a deadline), **batch**
   (compilers, encoders: wants all the CPU it can get) or **background** (almost idle).
   Without a model file, hand-written rules give the answer instead.
4. **Policy.** Interactive and media work gets a better nice value; batch work gets a
   worse one and `SCHED_BATCH`; background work gets `SCHED_IDLE` and runs only when
   nothing else wants the CPU. The table is in `src/policy.rs` and can be overridden
   with `--policy FILE`.

## What keeps it safe

- Only processes of ordinary users are touched (`--min-uid`, default 1000): never root's,
  never kernel threads, never the daemon itself, never anything given with `--ignore`.
- Never the desktop's own plumbing either (the message bus, sound, the compositor, portals,
  the lock screen; `SESSION_PLUMBING` in `src/engine.rs`). These sleep almost all the time
  and look like background work, but the whole desktop waits on them; an early VM test
  showed the session bus being given `SCHED_IDLE`, which under load would stall everything.
- A process is judged only after it has a baseline reading, and a change needs the same
  verdict several windows in a row (`--hysteresis`, default 3): one busy second changes
  nothing.
- A verdict the model is not sure about (`--min-confidence`, default 0.6) changes nothing.
- Every original setting is remembered and put back when the daemon stops (SIGTERM or
  SIGINT) or when a process's priority is no longer managed.
- A process the kernel refuses to change is left alone from then on.
- `--dry-run` decides and reports without changing anything.

## Using it

```bash
cargo build --release
sudo target/release/osaima-schedd run --dry-run -v        # see what it would do
sudo target/release/osaima-schedd run                      # do it
osaima-schedd --help
```

It runs as root, because lowering a nice value (favouring a process) needs privilege. On
the OSAIMA image it is a system service, `osaima-schedd.service`.

## Building and testing

```bash
cargo test                                   # unit tests (fake collectors and actuators, real child processes)

# eBPF telemetry, and the tests that need root:
sudo apt-get install clang llvm libbpf-dev libelf-dev zlib1g-dev linux-tools-generic
make -C bpf BPFTOOL=$(find /usr/lib -maxdepth 3 -name bpftool -type f | head -1)
sudo -E env "PATH=$PATH" OSAIMA_BPFTOOL=... OSAIMA_BPF_OBJECT=$PWD/bpf/telemetry.bpf.o \
    cargo test --test privileged -- --ignored --nocapture --test-threads=1
```

CI runs all of it on every push (`Scheduler daemon` and `Scheduler daemon (eBPF, root)`),
on a runner whose kernel has BTF and `sched_ext`.

## Training

`train/workload.py` imitates the four kinds of program: a burst of work after waiting for
input; a fixed-rate loop with a deadline; a flat-out computation with the odd pause; a
rare wake-up that does almost nothing. `train/collect.py` runs several of each at once,
so they compete for the CPU as real programs do, and measures them with the daemon's own
`sample` command, so training and use see the same numbers. `train/train.py` fits the
model, compares it with the built-in rules by 5-fold cross-validation, and writes
`model.json`, a report, and worked examples that a Rust test checks, so Rust and Python
are known to agree. The `Train the process classifier` workflow runs the whole thing on a
CI runner.

The shipped model (`model/`) was trained on 1440 measurements, 360 of each kind, taken on
a 4-CPU GitHub runner in rounds that cycle between a quiet machine, a busy one (one extra
CPU hog per CPU) and a crowded one (three per CPU). By 5-fold cross-validation:

| | Accuracy |
|---|---|
| Trained model | 91.4% |
| Built-in rules | 84.2% |

Both are perfect on interactive and background programs; the hard part is telling a
frame loop from a compiler on a crowded machine, where a frame loop that has fallen
behind never sleeps and looks like a compiler. The rules never take a compiler for a
frame loop, but take 216 of 360 frame-loop windows for compilers (they would slow a video
down); the model keeps 269 of 360, and takes 33 of 360 compiler windows for frame loops.
`model/report.md` has the tables and weights. `tests/golden.rs` checks that the Rust
daemon computes the same verdicts and probabilities as the trained scikit-learn model,
and that the Rust and Python copies of the rules agree.

An earlier model, trained only on a moderately busy machine, scored 98.6%, but on the
crowded benchmark below it took 7 of 12 CPU hogs for interactive programs: the high
score came from an easy test, which is why the training now includes crowded rounds.

## Benchmark

`bench/run.sh` runs a 60 fps style task (6 ms of CPU every 16.7 ms) alone, against three
CPU hogs per CPU, and against the same hogs with the daemon running, and counts the frames
that finish more than a frame late. The `Scheduler benchmark` workflow runs it. On a
4-CPU GitHub runner, with 12 hogs (run 39688081379):

| Run | Frames done (of 1500) | Median | 95th | 99th | Worst | Late frames |
|---|---|---|---|---|---|---|
| idle (no hogs) | 1500 | 6.08 ms | 6.08 ms | 6.09 ms | 6.12 ms | 0.0% |
| loaded, no scheduler | 1259 | 18.61 ms | 24.95 ms | 26.6 ms | 43.91 ms | 79.2% |
| loaded + scheduler (rules) | 1500 | 6.07 ms | 8.33 ms | 10.96 ms | 21.2 ms | 0.1% |
| loaded + scheduler (trained model) | 1500 | 6.07 ms | 8.23 ms | 9.57 ms | 13.88 ms | 0.0% |

Times are how long a frame took from when it was due. Without the daemon four in five
frames are late and a quarter of the frames are never drawn; with it, the frame task runs
almost as if the machine were idle. Both the rules and the model judged all 12 hogs batch
(nice 10, `SCHED_BATCH`) and the frame task media (nice -3); the model's worst frame is
better. This is one run on a shared CI machine, and the hogs are synthetic: it shows the
mechanism works, not how much a particular desktop gains.

## What it does not do

- It does not replace the kernel's scheduler; it steers it with priorities. A scheduler
  written as a `sched_ext` BPF program could go further, and the runner kernels support
  it; that is future work.
- The training data is synthetic. The programs imitate real behaviour but are not real
  programs; a classifier trained on measurements of real desktops would be better.
- It judges whole processes, not individual threads, so a program with one busy thread
  and one interactive thread gets one verdict.
- It adjusts per-thread priorities only; it does not touch cgroups, I/O or memory.
