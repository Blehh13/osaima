# OSAIMA — Frequently Asked Questions (RAG seed document)

## What is OSAIMA?
OSAIMA (Operating System Adaptive Intelligence Management Architecture) is an
AI-driven way to use a Linux computer. You tell it what you need in plain
language; it works out what that means on this system, shows you the exact
action and how risky it is, and runs it only after you approve. The assistant
is the shell: it is the home screen of Interstellar OS, a Gentoo-based
distribution built for the project.

## Which AI models does OSAIMA use?
Two kinds. Small models always run on the computer itself: an intent router
(a sentence encoder plus a classifier, about 30 ms per request) that handles
common commands instantly, and the same encoder for searching its knowledge
base. A language model handles everything else, such as writing text or
proposing a command. In Fast mode that is Google Gemini Flash-Lite (about one
to two seconds); in Private mode it is Phi-4-mini running on the computer, so
nothing leaves the machine but answers are slower. You choose the mode in the
AI settings (the button at the top right).

## What is sent to the cloud in Fast mode?
Only the request you type and any text it needs (for example the knowledge
snippet it is answering from). Commands always run on your computer, and only
after the risk check and your approval. Instant commands are understood on the
computer and never sent anywhere.

## What stops a dangerous command from running?
Every proposed action is put into a risk tier. Actions that only read
information run when you choose Run. Actions that change files or settings
need your OK. Actions that cannot be undone, such as deleting files, formatting
a disk or shutting down, need you to type "confirm". Commands run in a
bubblewrap sandbox with a time limit, and every action, approved or refused, is
written to a tamper-evident activity log in which each entry carries the hash
of the one before it.

## How does OSAIMA resist prompt injection?
Text that comes from outside (web pages, files, messages) is fenced off and
marked untrusted. Anything proposed because of untrusted text always needs
your approval, hidden instructions are stripped, and a secret canary token
catches a model that has been tricked into leaking its instructions. In a test
of 50 attacks, none ran a harmful action without approval.

## Does OSAIMA learn from me?
Yes, on the computer only. It keeps a local record of the requests you make,
shows the ones you use often as shortcuts, and remembers preferences you state
("remember that I prefer short emails"). Say "forget my preferences" to clear
them.

## How do I run the tests for an OSAIMA subsystem?
The evaluation suite lives in ai-core/evaluation. Run
`python ai-core/evaluation/run_eval.py` with the AI core running; it writes
RESULTS.md with task success, latency and safety results. The red-team test is
`python ai-core/security/redteam_eval.py`.
