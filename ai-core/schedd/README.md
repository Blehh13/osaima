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
   daemon reads the kernel's `/proc` accounting, which carries similar numbers.
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

The shipped model (`model/`) was trained on 1200 measurements, 300 of each kind, taken on
a 4-CPU GitHub runner. By 5-fold cross-validation:

| | Accuracy |
|---|---|
| Trained model | 98.6% |
| Built-in rules | 72.1% |

The rules fail exactly where it matters: when programs compete for the CPU, a hog that
keeps getting preempted looks like steady deadline work, and the rules call 275 of 300
batch programs "media" and would favour them. The model gets 295 of them right; its
only real confusion is 12 of 300 media windows taken for batch (`model/report.md` has the
full tables and the weights). `tests/golden.rs` checks that the Rust daemon computes the
same verdicts and probabilities as the trained scikit-learn model.

## Benchmark

`bench/run.sh` runs a 60 fps style task (6 ms of CPU every 16.7 ms) alone, against three
CPU hogs per CPU, and against the same hogs with the daemon running, and counts the frames
that finish more than a frame late. The `Scheduler benchmark` workflow runs it.

## What it does not do

- It does not replace the kernel's scheduler; it steers it with priorities. A scheduler
  written as a `sched_ext` BPF program could go further, and the runner kernels support
  it; that is future work.
- The training data is synthetic. The programs imitate real behaviour but are not real
  programs; a classifier trained on measurements of real desktops would be better.
- It judges whole processes, not individual threads, so a program with one busy thread
  and one interactive thread gets one verdict.
- It adjusts per-thread priorities only; it does not touch cgroups, I/O or memory.
