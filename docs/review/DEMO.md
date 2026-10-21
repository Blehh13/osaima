# OSAIMA live demo runbook (10-12 minutes, 14 with the two newer moments)

Audience: faculty examiners. Presenter: one person types, one person speaks.
Everything below uses only features that exist in the repo and phrases that
were run in `ai-core/evaluation/run_eval.py` (results in
`ai-core/evaluation/RESULTS.md`). If you are short of time, drop Moments 10 and 5 first. Rehearse the whole run twice on the exact
machine and network you will use on Saturday.

Times given for each step are the measured numbers from RESULTS.md (Gemini
3.5 Flash-Lite, Ryzen 5 3550H laptop). Live times on Saturday will differ with
the network; do not promise them, show what the screen says.

## 1. Setup checklist (do this the day before, repeat one hour before)

### Option A: the ISO in QEMU (the "it is a real OS" demo)

Needs an x86-64 Linux host with KVM (`/dev/kvm` writable; `run_vm.sh` exits
if not) and about 6 GB free RAM for the guest.

1. Build once (needs root, long): `sudo build-tools/scripts/build_vm_image.sh`
   then `sudo build-tools/scripts/build_iso.sh`. Output: `out/interstellar-os.iso`.
   Do not build on Saturday; use the ISO you already tested.
2. Boot: `ISO=1 build-tools/scripts/run_vm.sh` (add `RES=1920x1080` for a
   projector). The guest sees 1600x900 by default. The window is GTK with
   zoom-to-fit.
3. Wait about a minute. The top bar (waybar) shows `AI: starting` while the
   on-device model loads (model load from the ISO was measured at 58 s), then
   `AI: on this computer`. (`AI: Gemini` appears only if a key was added and Fast mode chosen.)
4. The guest uses QEMU user networking, so it needs the host to be online for
   Gemini. The guest's shell server is also forwarded to
   `http://localhost:8778` and SSH to `localhost:2222` (key in `out/vm/id_osaima`).

### Option A2: the ISO in VirtualBox on a Windows laptop (the presentation setup)

Free: VirtualBox 7.x. Backup if VirtualBox is slow: VMware Workstation Pro (free
for personal use). Neither needs anything installed inside the guest.

Before the day (on the presentation laptop):

1. Windows must not be holding the CPU's virtualization for itself, or VirtualBox
   runs very slowly (a green turtle icon in the VM's status bar). Turn off:
   Windows Security > Device security > Core isolation > Memory integrity, and in
   "Turn Windows features on or off": Hyper-V and Virtual Machine Platform. Restart.
   (If the laptop needs WSL2/Docker, use VMware Workstation instead.)
2. Copy `interstellar-os.iso` (3.9 GB) to the laptop. Any USB stick works (it is
   just under FAT32's 4 GB limit; exFAT/NTFS also fine).
3. New VM: type Linux, version Other Linux (64-bit). Memory 8 GB (6 GB if the
   laptop has 8 GB; then use Fast mode and close other apps). 4 CPUs. No hard disk.
   Display: Graphics Controller **VMSVGA**, video memory 128 MB, 3D acceleration
   **off** (the image has the VMSVGA driver, not the old VBoxVGA one). Storage:
   attach the ISO to the optical drive. Network: NAT (default). EFI on or off.
4. Start it. GRUB boots on its own; the desktop logs in by itself. If the screen
   is small, View > Virtual Screen 1 > 1600x900, or resize the window.
5. First start opens Settings: choose Fast, paste the Gemini key (bring it on a
   note, never inside the ISO), Save. The top bar shows `AI: Gemini`.
6. Rehearse every moment below at least once in this exact VM. Not yet tested by
   the team: booting in VirtualBox (all automated boot tests used QEMU).

Do not run VirtualBox and another hypervisor's VM at the same time.

### Option B: a laptop, nested (fastest to start, no ISO)

Prerequisites on the laptop: `sway`, `waybar`, `mako`, `foot`, `thunar`,
`mousepad`, `firefox`, `bubblewrap`, `poppler-utils`, Python 3 with the router's
dependencies, and the two builds:

```
(cd ai-core/mcp-daemon && cargo build --release)        # mcp-daemon and osaima-jail
(cd ui/osaima-shell/src-tauri && cargo build --release) # osaima-shell
desktop/dev/run-nested.sh                               # --built to use pages baked into the binary
```

`run-nested.sh` starts the intent router, the daemon and the shell server if
they are not already running, then opens sway inside a window on your own
desktop (1600x900). In that nested window the modifier key is Alt, not Super.
Logs go to `$XDG_RUNTIME_DIR/osaima-dev/`. The on-device model server
(`ai-core/llm/start-llm.sh`) is not started by this script; start it yourself
if you want Private mode on a laptop.

### The stack, in the order it must be up

| # | Component | How it starts | Check |
|---|---|---|---|
| 1 | Intent router (Python, MiniLM encoder) | ISO: systemd user service. Nested: `run-nested.sh` | socket `$XDG_RUNTIME_DIR/osaima/router.sock` exists |
| 2 | mcp-daemon (Rust) | same | socket `$XDG_RUNTIME_DIR/osaima/mcp.sock` exists |
| 3 | On-device LLM (llama.cpp) | ISO: `osaima-llm.service` | waybar: not `AI: starting` |
| 4 | Gemini key | Settings app (below) | waybar says `AI: Gemini` |

### Gemini key (never in the repo or the image)

1. Press `Super+,` (nested: `Alt+,`) or click the waybar AI status. Settings opens.
2. Choose **Fast**, paste the key, press Save. The key is checked, then saved to
   `~/.config/osaima/llm.env` (mode 0600) and the daemon switches to cloud mode.
3. Do this before the audience arrives. Never show the key on a projector.

### Demo data (one minute)

Create the same folder the evaluation uses, so the file demos behave as measured:

```
mkdir -p ~/osaima-eval/Archive && cd ~/osaima-eval
head -c 3000000 /dev/zero > report-march.pdf
head -c 6000000 /dev/zero > report-april.pdf
echo "meeting notes" > notes.txt; echo "todo" > todo.txt; head -c 50000 /dev/zero > holiday.jpg
```

For the "Ask OSAIMA" step you need a real, readable PDF in the home folder:
copy `OSAIMA.pdf` from the repo to `~/OSAIMA.pdf`. (The zero-filled files above are
fine for moves and renames but are not readable PDFs.)

### Pre-flight (5 minutes before)

- [ ] Waybar shows `AI: Gemini`.
- [ ] Type `how is my system doing` once; a reply appears in under a second.
- [ ] Type `explain what OSAIMA is` once (warms the cloud connection).
- [ ] Reset the demo folder with the commands above (the demo changes it).
- [ ] Close every window except the assistant. Terminal ready: `Super+Return`.
- [ ] Phone hotspot ready as a network fallback.

## 2. The demo, in order

Keep the assistant window visible. Each reply shows who answered ("on this
computer" or "Gemini") and how long it took; point at that line.

### Moment 1: it is an OS, the assistant is the shell (1 min)

- Do: boot has finished; show the desktop. Point at the top bar (OSAIMA, workspaces, clock, `AI: Gemini`) and the assistant window, which is the only thing open.
- Say: "Interstellar OS is Gentoo-based. The desktop is sway. The assistant is the first thing you see; every other app is something it opens."
- Proves: real window manager, real OS image, not a web page.

### Moment 2: instant commands, no LLM (1 min)

- Type: `set brightness to 80%`, then `is wifi on`, then `how is my system doing`.
- Audience sees: replies in a few hundredths of a second, labelled as answered on this computer. The system reply names the programs using the most memory.
- Proves: speed. Local intent router answers 64.1% of shell-executable requests without any LLM; fast-path latency p50 35.7 ms, p95 43.3 ms (PDR target 200 ms). Works with no internet.
- Be honest: on a VM, brightness and wifi may report that the device is missing. If one fails on the day, say so and move on. (Do not use `lock the screen` as a demo: it locks the screen.)

### Moment 3: real windows (1.5 min)

- Type: `open the file manager`, then `open the terminal`, then `switch to the grid layout`, then `go to workspace 3`, then `go to workspace 1`.
- Audience sees: Thunar and foot appear as real tiled windows; the layout changes; workspace number changes in the top bar.
- Proves: the assistant drives the actual window manager through sway IPC (`desktop.rs`), not panels inside a web page. Apps start only from a fixed table in the daemon, never from model output.

### Moment 4: safety tiers (2 min)

- Type: `what is my kernel version` (LLM writes one read-only command).
  Audience sees: it runs at once, no question asked. Tier: "only looks".
- Type: `install htop`. In the latest build this request is treated as an agent task (the evaluation notes it "now becomes a task"), so you may see a scope card instead of a plain command card. Press **Cancel** or decline. Say: "Even if I approved, the system is read-only inside the sandbox and there is no root; root changes must be done by the user in a real terminal." If you want a plain command card instead, use `delete all the log files in my Downloads folder` below.
- Type: `delete all the log files in my Downloads folder`.
  Audience sees: an approval card showing the command and why it is risky. If the card is red, it asks you to type `confirm`. Cancel.
- Proves: three tiers (only looks / makes changes / cannot be undone). The daemon re-assesses every command itself; the UI cannot lower a tier. Nothing destructive ran.
- Rehearse this once: confirm which tier each phrase gets on your build and say that, not what is written here.

### Moment 5: files, grounded in what is really on disk (1.5 min)

- Type: `find the pdfs bigger than 5 MB in osaima-eval`.
  Audience sees: a list with Open and Show in Files buttons; only report-april.pdf. Press Show in Files: Thunar opens with the file selected.
- Type: `rename todo.txt in osaima-eval to todo-old.txt`.
  Audience sees: approval card naming the files ("rename todo.txt to todo-old.txt"). Approve. The file is renamed.
- Press **Undo** on the card. The name goes back.
- Proves: file actions are typed operations (paths are arguments, never shell text), confined to the home folder, with undo; the model sees the real file names of the folder you mention.

### Moment 6: multi-step plan, one approval, undo the whole plan (1.5 min)

- Type: `in osaima-eval make a folder called Reports and move both report pdfs into it`.
  Audience sees: one card listing each step with its tier, one Approve button; steps run in order with live status; at the end "Undo the whole plan".
- If the assistant offers "Save as a routine" (Moment 6b), save it first. Then press **Undo the whole plan**. Reports is gone, PDFs are back.
- Proves: orchestration. Each step still goes through the daemon's gates; the plan approval covers only the steps shown. First failure stops the rest. Measured: 3.8 s.

### Moment 6b: save the plan as a routine, run it from the rail (1 min)

- Do: after Moment 6 finishes, the assistant offers **Save as a routine** and lists exactly the steps it would save. Name it `tidy reports`, save.
- Reset the folder (the commands in section 1), then either click `tidy reports` under **Your routines** in the rail, or type `run my tidy reports routine`.
  Audience sees: the plan card again, with every step, and an approval request. Approve; both steps run (a "new folder" step that finds the folder already there counts as done).
- Proves: routines. Saved plans replay through the same plan card and the same gates; **every replay asks first**. The offer is not made for plans that used outside text (pages, mail, handed-in documents).
- Limit to say: a routine stores the steps, including file names, not a general rule. The repeat detector also offers to save plans you run several times.

### Moment 7: "Ask OSAIMA" from the file manager (1.5 min)

- Do: in Thunar, go to the home folder, right-click `OSAIMA.pdf`, choose **Ask OSAIMA**.
  Audience sees: the assistant comes forward with "About OSAIMA.pdf" above the input.
- Type: `summarise this in three bullet points`.
  Audience sees: three bullets from the document (3.5 s when tested).
- Proves: files as context. The daemon reads the first 5 pages with `pdftotext` and fences the text as untrusted outside content, so a document cannot give the assistant orders.

### Moment 7b: an agent task (2.5 min)

- Type: `create a small python command-line tool in ~/projects/demo-wordcount that counts the words in a text file, with a unit test, and run the test`
  Audience sees, in order:
  1. A **scope card**: the one folder the task may change (~/projects/demo-wordcount), whether it needs the internet (it should not), and an outline. Read the limits aloud, then approve once.
  2. A **live task card** with each step as it happens (write file, write test, run test) and its output. The assistant stays usable; the task runs in the background and has a Stop button.
  3. A short **summary**: what was built, where, how to run it.
- Then open the folder with the file manager and show the files.
- Proves: an agent loop. The model calls a tool, sees the result, decides the next step (it can fix a failing test). Limits: 30 steps and 20 minutes, 5 minutes per step; Landlock allows writes only in the approved folder; no root (sudo and su are refused); anything rated irreversible still waits for a typed `confirm`.
- Measured: a similar word-count CLI with a unit test took 5 steps in 17 s (commit 3125ae9), and the evaluation's 3 agent tasks passed (RESULTS.md section 7: 2 to 4 steps, 5 to 17 s).
- If it misbehaves: press Stop and say the budget and scope are exactly there for that. Rehearse this one at least three times; model output varies.

### Moment 8: kernel enforcement (1.5 min)

- Open the terminal (`Super+Return`). Run, one per line:

```
osaima-jail -- sh -c 'head -c1 /etc/os-release'      # allowed
osaima-jail -- sh -c 'echo x > ~/probe'              # refused by Landlock
osaima-jail -- sh -c 'sudo -n true'                  # refused (no_new_privs)
osaima-jail -- unshare -r true                       # refused (seccomp)
osaima-jail -- sh -c 'mount -t tmpfs x /mnt'         # refused (seccomp)
```

- Audience sees: the first line prints `osaima-jail: landlock=full seccomp=on` on stderr; reads work, the others fail.
- Proves: limits are enforced by the Linux kernel on every AI-run command and cannot be lifted from inside. 6/6 cases behaved as required in the evaluation.
- Say plainly: the assistant itself is an ordinary process. The kernel limits what it runs.
- On the ISO `osaima-jail` is at `/usr/bin/osaima-jail`. On a laptop use `ai-core/mcp-daemon/target/release/osaima-jail`.

### Moment 9: activity log and the privacy switch (1 min)

- Open the assistant side rail, **Activity**: recent actions from the audit log. Open Settings: press the log check; it reports the hash chain is intact.
- In Settings choose **Private**, Save. Waybar changes to `AI: on this computer`. Say what each mode sends where (Settings text says it: Fast sends what you type to Google; Private sends nothing off the machine; commands run only on this computer in both).
- Switch back to **Fast**. Do not ask a long question in Private mode live; on this hardware the on-device model takes 10-150 s.
- Proves: tamper-evident audit (SHA-256 chain), user control of privacy.

### Moment 10 (optional, if time): learning

- Type: `remember that I prefer short emails`, then `draft a two-line email to my guide asking for a meeting`.
  Audience sees: "Got it, I'll remember...", then a short draft.
- Point at the suggestion chips and learned shortcuts in the rail: they come from the user's own safe, frequent fast-path commands, stored in a local SQLite file.
- Proves: behaviour store (on-device). Limit: it is frequency and stated-preference learning, not learning from edits.

## 3. If something goes wrong

| Symptom | Do this |
|---|---|
| Gemini slow or erroring | Settings, **Private**. Then stay with instant commands (Moments 2, 3, 8); file and plan requests need the LLM and the local model is slow, so say that openly. The daemon tries three Gemini models on the key and then falls back to the local model by itself; the reply line shows who answered. |
| No network | Instant commands still work (router and gates are local). Switch to Private for anything else. Skip Moments 5-7 if the local model is still loading. |
| Waybar says `AI: starting` | The model is loading (about a minute from the ISO). Show Moment 8 while you wait. |
| Waybar says `AI: offline` | Daemon is down. Nested: re-run `desktop/dev/run-nested.sh` (it restarts missing services), logs in `$XDG_RUNTIME_DIR/osaima-dev/`. ISO: `ssh -p 2222 -i out/vm/id_osaima root@localhost`, `systemctl --user status osaima-mcp-daemon` as the `osaima` user. |
| Gemini free-tier rate limit (15 requests/min per model) | Wait a minute, or talk while the daemon rotates to the other Flash-Lite models. Do not retype repeatedly. |
| A file demo does nothing | Re-create the demo folder (section 1). A path outside the home folder is refused by design. |
| Examiner asks for something unplanned | Welcome it; read the approval card aloud. A refusal is also a result. |
| Total failure | Show `ai-core/evaluation/RESULTS.md` and the red-team file `ai-core/security/redteam_results_gemini.json`, and walk through the architecture diagram in `ARCHITECTURE.md`. |

## 4. Things not to do live

- Do not type `lock the screen`, shutdown, reboot, or "close everything" unless you want to demonstrate the approval card (closing every window needs approval; power actions need typed `confirm`).
- Do not claim voice control. The shipped interface is text. Wake-word and speech-recognition tooling exists in `ai-core/asr/` but is not part of this demo.
- Do not run the full evaluation live.
