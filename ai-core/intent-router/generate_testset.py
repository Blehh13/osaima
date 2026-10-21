#!/usr/bin/env python3
"""
OSAIMA Intent Router — HELD-OUT test set (PDR §9.1: 1,000 utterances, target >92% acc).

Deliberately uses DIFFERENT surface phrasings / prefixes from generate_dataset.py
so the test set is a genuine held-out distribution, not a memorised copy of train.
Same 3-track + 40-intent taxonomy. 900 clean + 100 manual-style edge cases.

Output: testset.jsonl   schema {id, utterance, track, intent, edge_case}
"""
import json, random, csv
random.seed(2025)  # different seed from the training generator

# Same taxonomy, NEW paraphrase templates (held-out surface forms)
INTENTS = {
    "app_launch":        ("SYSTEM_COMMAND", ["fire up {app}", "get {app} on screen", "I need {app} open"]),
    "app_close":         ("SYSTEM_COMMAND", ["exit {app}", "get rid of {app}", "dismiss {app}"]),
    "window_focus":      ("SYSTEM_COMMAND", ["jump to the next window", "cycle to the window before this", "move focus over"]),
    "window_tile":       ("SYSTEM_COMMAND", ["arrange the windows side by side", "maximise this one", "put this on the right half"]),
    "window_move_ws":    ("SYSTEM_COMMAND", ["throw this to workspace four", "shift this window to desktop one", "take me to workspace five"]),
    "volume_control":    ("SYSTEM_COMMAND", ["silence the sound", "crank the volume", "drop the volume to twenty percent"]),
    "media_playback":    ("SYSTEM_COMMAND", ["stop the track", "resume playback", "jump to the next tune"]),
    "brightness_control":("SYSTEM_COMMAND", ["dim the display", "max out the brightness", "lower the screen brightness a bit"]),
    "screenshot":        ("SYSTEM_COMMAND", ["grab a screenshot", "snap the current screen", "take a picture of this window"]),
    "lock_screen":       ("SYSTEM_COMMAND", ["secure the screen", "put the machine on lock", "lock up"]),
    "power_control":     ("SYSTEM_COMMAND", ["power off the machine", "do a full restart", "put it to sleep"]),
    "wifi_control":      ("SYSTEM_COMMAND", ["switch wifi on", "drop the wireless connection", "join the office network"]),
    "bluetooth_control": ("SYSTEM_COMMAND", ["switch bluetooth on", "kill bluetooth", "pair my earbuds"]),
    "vpn_control":       ("SYSTEM_COMMAND", ["bring the vpn up", "drop the vpn", "flip the vpn"]),
    "service_control":   ("SYSTEM_COMMAND", ["bounce {svc}", "shut {svc} down", "is {svc} running"]),
    "process_list":      ("SYSTEM_COMMAND", ["what processes are active", "give me the process list", "show me everything running"]),
    "process_kill":      ("SYSTEM_COMMAND", ["force quit the hung program", "stop the runaway process", "nuke that task"]),
    "system_stats":      ("SYSTEM_COMMAND", ["what's my cpu load", "how much memory is free", "where's my disk space going"]),
    "battery_status":    ("SYSTEM_COMMAND", ["how's my battery doing", "is the laptop charging", "battery level please"]),
    "file_search":       ("SYSTEM_COMMAND", ["hunt down every python file in {dir}", "where is my resume file", "look for anything called report"]),
    "file_open":         ("SYSTEM_COMMAND", ["pull up the readme", "bring my notes on screen", "reopen the file I just closed"]),
    "clipboard_control": ("SYSTEM_COMMAND", ["put this on the clipboard", "drop in what I copied", "what's in my clipboard"]),
    "notification_ctrl": ("SYSTEM_COMMAND", ["enable do not disturb", "wipe my notifications", "silence the alerts"]),
    "mount_control":     ("SYSTEM_COMMAND", ["bring up the usb stick", "safely remove the external drive", "pop out the sd card"]),
    "project_scaffold":  ("WORKFLOW_GENERATION", ["spin up a {lang} project the way I like it", "bootstrap a flask app named api", "start a fresh next.js app with my setup"]),
    "git_workflow":      ("WORKFLOW_GENERATION", ["add commit and push all of this", "cut a new feature branch", "commit my work the way I usually word it"]),
    "env_setup":         ("WORKFLOW_GENERATION", ["spin up a venv and turn it on", "build a conda environment with scipy", "make an environment and pull the requirements"]),
    "package_install":   ("WORKFLOW_GENERATION", ["pull in this project's dependencies", "add requests and flask here", "get torch installed with gpu support"]),
    "run_tests":         ("WORKFLOW_GENERATION", ["kick off the test suite", "run pytest and tell me what breaks", "execute the tests with a coverage report"]),
    "deploy_workflow":   ("WORKFLOW_GENERATION", ["containerise this and launch it", "ship this to staging", "boot the dev server on port five thousand"]),
    "file_batch_op":     ("WORKFLOW_GENERATION", ["number all the photos in order", "turn every png in {dir} into jpg", "sort {dir} into folders by type"]),
    "build_compile":     ("WORKFLOW_GENERATION", ["compile this {lang} codebase", "produce the release build", "build it and rebuild on every change"]),
    "email_draft":       ("CONTENT_GENERATION", ["put together an email to {who} about the deadline", "reply to {who} turning down the meeting", "write {who} a status update"]),
    "commit_message":    ("CONTENT_GENERATION", ["come up with a commit message for this", "phrase a commit the way I normally do", "turn this diff into a commit line"]),
    "code_generate":     ("CONTENT_GENERATION", ["give me a {lang} function that reverses a string", "cook up a file-renaming script", "write me a {lang} linked list class"]),
    "code_refactor":     ("CONTENT_GENERATION", ["tidy up this function", "restructure this code", "break this big function apart"]),
    "docstring_gen":     ("CONTENT_GENERATION", ["write a docstring for this", "annotate these parameters with types", "add documentation to this class"]),
    "summarize_text":    ("CONTENT_GENERATION", ["condense this document", "hit me with the key points of this readme", "give me the gist of this file"]),
    "explain_concept":   ("CONTENT_GENERATION", ["walk me through how a hash map works", "what's this error telling me", "break down what this command is doing"]),
    "translate_text":    ("CONTENT_GENERATION", ["put this into French", "render this paragraph in Hindi", "how do I say this in Spanish"]),
}
assert len(INTENTS) == 40

APPS=["Firefox","the terminal","VS Code","the Files app","GNOME Settings","Spotify","Slack"]
DIRS=["my projects folder","Documents","Downloads","the home directory","~/code"]
LANGS=["Python","JavaScript","Rust","Go","TypeScript"]
PEOPLE=["my team","the professor","my manager","the client","the group"]
SERVICES=["docker","nginx","postgresql","the ssh service","cups"]
PREFIXES=["","","","osaima ","hey ","can you ","would you "]

EDGE=[("handle this one","CONTENT_GENERATION","code_refactor"),
      ("get it going","WORKFLOW_GENERATION","deploy_workflow"),
      ("wrap it up","WORKFLOW_GENERATION","git_workflow"),
      ("shut it","SYSTEM_COMMAND","app_close"),
      ("take it down","SYSTEM_COMMAND","service_control"),
      ("clean this","CONTENT_GENERATION","code_refactor"),
      ("save this out","CONTENT_GENERATION","code_generate"),
      ("ping them","CONTENT_GENERATION","email_draft"),
      ("drop it","SYSTEM_COMMAND","process_kill"),
      ("hook me up","SYSTEM_COMMAND","wifi_control")]

def fill(t):
    return (t.replace("{app}",random.choice(APPS)).replace("{dir}",random.choice(DIRS))
             .replace("{lang}",random.choice(LANGS)).replace("{who}",random.choice(PEOPLE))
             .replace("{svc}",random.choice(SERVICES)))

def main():
    rows=[]; names=list(INTENTS)
    for i in range(900):
        it=random.choice(names); tr,tmpl=INTENTS[it]
        rows.append({"id":i,"utterance":(random.choice(PREFIXES)+fill(random.choice(tmpl))).strip(),
                     "track":tr,"intent":it,"edge_case":False})
    for j in range(100):
        u,tr,it=EDGE[j%len(EDGE)]
        rows.append({"id":900+j,"utterance":u,"track":tr,"intent":it,"edge_case":True})
    # Relabel the PDR-40 names onto the unified taxonomy (taxonomy.py), then add the
    # LLM-written examples for intents this file has no templates for.
    import os, taxonomy
    for r in rows:
        r["intent"] = taxonomy.OLD_TO_NEW[r["intent"]]
        r["track"] = taxonomy.track_of(r["intent"])
    if os.path.exists("heldout_llm.jsonl"):
        for l in open("heldout_llm.jsonl", encoding="utf-8"):
            if l.strip():
                h = json.loads(l)
                rows.append({"id": 0, "utterance": h["utterance"], "track": h["track"],
                             "intent": h["intent"], "edge_case": False})
    else:
        print("note: heldout_llm.jsonl missing; run generate_heldout_llm.py to cover the newer intents")
    random.shuffle(rows)
    for i,r in enumerate(rows): r["id"]=i
    with open("testset.jsonl","w",encoding="utf-8") as f:
        for r in rows: f.write(json.dumps(r,ensure_ascii=False)+"\n")
    from collections import Counter
    print(f"Wrote {len(rows)} rows -> testset.jsonl")
    print("Tracks:",dict(Counter(r['track'] for r in rows)),"| edges:",sum(r['edge_case'] for r in rows),
          "| intents:",len({r['intent'] for r in rows}))

if __name__=="__main__": main()
