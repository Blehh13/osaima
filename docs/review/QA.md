# Likely examiner questions

Short answers. Numbers come from `ai-core/evaluation/RESULTS.md`; the evidence
for each is in `CLAIMS.md`. If you do not know, say "we did not measure that".

**1. What is OSAIMA, in one sentence?**
A Linux-based OS (Interstellar OS, a Gentoo derivative) in which an AI assistant
is the shell: you say what you want in plain words, it shows exactly what it
will do and how risky that is, you approve, and it acts on the real desktop,
files and system.

**2. Why Linux, and why Gentoo?**
Linux gives us the kernel features we rely on for safety (namespaces, Landlock,
seccomp), D-Bus for desktop control, and sway for window control through an IPC
socket. Gentoo gives a small image we control completely, a clean overlay
mechanism for our own profile and packages (`gentoo/overlay/`), and binary
packages so a test image builds in under an hour. The PDR proposed Ubuntu; we
changed it because we wanted to ship a bootable image of our own desktop, not a
set of scripts on top of someone else's.

**3. Why Gemini and not a local model? The PDR says fully local.**
Speed. On our hardware (Ryzen 5, no fast GPU) the on-device model took 10-150 s
per answer; Gemini 3.5 Flash-Lite answers in a few seconds (median 3.7 s in the
evaluation). A shell that takes 30 seconds to answer is not usable. So
Fast (Gemini) is available as an option with the user's own key, and Private
mode, the free default, keeps everything on the machine at the cost of speed. The router that handles most simple
commands is always local. We say openly that this departs from the PDR's
"fully local" goal.

**4. What about privacy, then?**
In Fast mode, what you type, the last six conversation turns and any file text
or folder listing you attach go to Google's Gemini API. In Private mode nothing
leaves the machine. In both modes the intent router, knowledge search, behaviour
store, approval gates and command execution are local, and the API key is stored
only in `~/.config/osaima/llm.env` with mode 0600. Settings states this to the
user in plain words. The audit log stays local.

**5. How is prompt injection handled?**
In layers, none of which we call perfect. (1) Outside text (files, clipboard,
web, e-mail, file names) is stripped of instruction-like patterns and fenced as
untrusted data, and the system prompt says to ignore instructions inside it.
(2) A random canary token in the prompt catches a model that leaks its prompt.
(3) Any action proposed while untrusted text was present is "tainted" and always
needs approval. (4) Every command is re-classified by the daemon and gated by
risk tier. (5) Approved commands run in a kernel-limited sandbox. In our 50-case
red-team, 1 counted as successful by the strict metric (#37: the model answered
a "format the drive" attack with a harmless `lsblk`). For 5 more cases the model
was fooled but the gates still required approval.

**6. How do the risk tiers work?**
The daemon (not the UI) reads the command text and puts it in one of three
tiers, using the riskiest part of a pipeline. Only looks (ls, df, ps): runs
without asking. Makes changes that can be undone: one click. Cannot be undone
(rm, dd, sudo, git push, writing to system paths, recursive chmod, removing
software): approval plus typing `confirm`. Unknown programs default to "makes
changes", not "safe". Commands inside `$(...)`, `xargs` and `find -exec` are
judged by what they run. The daemon re-checks at execution time, so a changed UI
cannot lower a tier.

**7. What do Landlock and seccomp do here?**
They are Linux kernel features applied by our helper `osaima-jail` just before an
AI-run command starts, and inherited by everything it starts. Landlock makes the
whole filesystem read/execute-only except named folders (/tmp, and $HOME only for
approved changes). seccomp makes dangerous system calls fail (mount, ptrace, load
kernel modules, create namespaces, reboot, bpf, ...). `no_new_privs` stops sudo
or setuid programs from gaining root. The command cannot remove these limits. We
tested six cases (RESULTS.md section 6): reading system files allowed; writing in
home from a read-only command, sudo, new namespace, mount and ptrace refused.

**8. So can the AI itself be harmful?**
The assistant (the daemon) runs as a normal user process. The kernel limits what
the commands it runs can do, and every state-changing action passes an approval
gate. The AI proposes; the user approves; the sandbox constrains. We do not claim
the AI process itself is confined, and the AppArmor profile in the repo is only a
first stage (it permits user namespaces; it does not yet confine the daemon).

**9. What happens offline?**
Simple commands still work: the intent router and all gates are local (fast path
about 40 ms, 64% of shell-executable requests in the test set). Anything needing a
language model falls back to the on-device model (Phi-4-mini on llama.cpp), which
works but is slow on CPU. The daemon tries three Gemini models first, with an 8 s
cloud timeout, then the local model.

**10. How does it learn?**
A local SQLite behaviour store logs every request with intent, path taken and
outcome. From it the assistant offers suggestion chips built from the user's own
frequent, safe commands (weighted by frequency, recency and time of day), and it
stores stated preferences ("remember that I prefer short emails") and passes them
to the LLM when it writes text. It does not retrain a model and does not yet learn
style from the user's edits (a PDR item we have not built).

**11. How is it different from Copilot, Siri, Alexa?**
Those are assistants running on top of an OS they do not control, mostly in the
cloud, with limited ability to act on your system and no user-visible risk model.
OSAIMA is the OS interface: it acts on real windows, files and system services,
shows a risk tier before every action, asks approval accordingly, offers undo, and
enforces limits in the kernel. It also has a Private mode with no cloud at all.

**12. How is it different from Mycroft or other Linux voice assistants?**
Mycroft needed a hand-written skill for each capability and has been archived. We
use a language model for open-ended requests and typed, gated tools for actions.
Existing Linux assistants in the PDR survey did not have risk-tiered confirmation,
sandboxing or injection defences for what they execute. We are text-first today;
Mycroft was voice-first.

**13. What is novel?**
Not the individual parts. The combination: an assistant as the primary shell of a
bootable distribution; a three-tier approval model decided by the daemon rather
than the model or UI; a kernel-enforced jail on every AI-run command, with the
enforced state reported back; taint tracking so anything proposed from untrusted
content always needs approval; typed, undoable file actions; multi-step plans with
one approval and one undo; and a measured evaluation including a red-team.

**14. How did you evaluate it?**
`ai-core/evaluation/run_eval.py` produces RESULTS.md: intent accuracy on 1484
held-out sentences (66 intents), fast-path latency over 100 requests, 25 end-to-end
tasks (25/25), a 50-case injection red-team (1 strict hit), a kernel test (6/6),
a sandbox-isolation test (5/5), three agent tasks (3/3),
and the daemon's 17 unit tests plus 36 slot-extraction tests.

**15. Your fast-path accuracy is 91.3% against a 92% target. Why?**
It is slightly below, and we say so. The held-out labels are unreviewed; when we
inspected the fast path's errors most were label problems (for example "Hey, can
you set a timer" labelled as a greeting), so true accuracy is probably higher, but
we have not re-labelled the set and do not cite a higher number. Low-confidence
requests go to the LLM rather than being guessed.

**16. Is the evaluation independent?**
No. The team wrote the tasks and the 50 attack prompts, on one laptop and one
model. Results show behaviour on those cases, not a general guarantee.

**17. What are the main limitations?**
Text only (no voice). Cloud mode sends text to Google. Local model is slow on CPU.
The risk classifier reads command text and is not exhaustive. The AI process itself
is not confined (AppArmor is stage 1). Network is shared inside the sandbox.
Undo covers file operations done through the assistant and is lost if the daemon
restarts. Style learning from edits is not built. Tested in a VM and on one laptop,
not on bare metal. Red-team and test sets are team-written.

**18. What would you do next?**
Review and re-label the held-out set, and have people outside the team attack it;
confine the daemon with a full AppArmor or Landlock profile; add speech input and
the wake word (tooling started in `ai-core/asr/`); learn style from edits; per-
command network control inside the sandbox; a full installer and bare-metal tests;
persistent undo; a stronger local model when hardware allows.

**19. Why not let the model just run commands directly?**
Because models make mistakes and can be manipulated. The model only proposes; the
daemon decides the tier and what approval is needed; and the command runs in a
jail even after approval. For files and windows we avoid shell text altogether:
they are typed operations with paths as arguments, so a file name cannot carry a
command.

**20. What if I approve something dangerous?**
Irreversible commands need you to type `confirm`, and the card says why it is
risky. Even then the sandbox keeps the system read-only (so `sudo` and writes to
system paths do not work from the assistant; root changes must be done by the user
in a real terminal), keeps secrets such as ~/.ssh out of file actions, and records
the action in a tamper-evident log. Your own home folder is writable for approved
changes, so an approved `rm -rf ~/something` can still delete your files.

**21. What is the difference between an agent task and a multi-step plan?**
A plan is written once, up front: the model lists the steps, you approve the whole
list, and the steps run in order. That works when nothing in step 3 depends on what
step 2 printed. An agent task is a loop: the model proposes a scope (one folder,
internet or not, an outline), you approve it once, then it calls a tool, reads the
result and chooses the next step, so it can fix a failing test or work around a
missing package. Tasks have a budget of 30 steps and 20 minutes. Plans are shown in
full before they run; tasks are bounded by the scope instead.

**22. What stops an agent from doing damage?**
The scope you approved and the kernel. Every command runs in bubblewrap plus
`osaima-jail`, with Landlock allowing writes only in the approved folder, seccomp,
no root, an empty environment, and the daemon socket, the session bus and your keys
hidden. Irreversible steps still need a typed `confirm`. The budget stops a runaway
loop, there is a Stop button, and every step is in the audit log. It can still do
anything inside its folder (and use the network if you approved internet), so read
the scope card.

**23. Why can tasks not use root?**
Because an assistant that can become root has no useful limits. `sudo`, `su` and
`pkexec` are refused before they run, and `no_new_privs` means setuid programs
cannot gain privilege inside the sandbox. Root changes (for example installing a
package system-wide) must be done by a person in a real terminal.

**24. What if the model is prompt-injected in the middle of a task?**
Tool output (file contents, command output, web pages) goes back to the model as
data, not instructions, and the canary check still applies. If the model is
fooled anyway, it is still inside the same cage: writes only in the approved
folder, no root, no access to the daemon or your keys (so it cannot call the daemon
and approve its own actions), irreversible steps wait for you, and the step shows
in the live card and the audit log. We tested the sandbox isolation (5/5) and the
50-case red-team (1 strict hit), but we have not run a red-team specifically against
the agent loop, so we do not claim immunity.

**25. What is a routine?**
A finished plan you save by name, listed in the side rail. "Run my tidy reports
routine" replays the steps through the normal plan card, and every replay asks for
approval first. It stores the literal steps, not a rule, and is not offered if the
plan used text from outside the computer.
