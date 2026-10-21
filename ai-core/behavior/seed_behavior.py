#!/usr/bin/env python3
"""
OSAIMA Behavior Store — seed generator (PDR §3.7).

Real behavior data accrues at runtime as the user uses the OS. For demos,
benchmarks, and unit tests we seed a synthetic-but-plausible profile: ~30 days
of interactions for a developer persona, distilled style rules, a detected
workflow, and app-frequency counters.

Builds behavior.db from schema.sql in the same directory.

    python seed_behavior.py            # -> behavior.db
    python seed_behavior.py --report   # print learned summary
"""
import sqlite3, os, json, random, argparse
from datetime import datetime, timedelta

random.seed(11)
HERE = os.path.dirname(os.path.abspath(__file__))
DB = os.path.join(HERE, "behavior.db")
SCHEMA = os.path.join(HERE, "schema.sql")

APPS = ["VS Code", "the terminal", "Firefox", "Slack", "the Files app", "Spotify"]
INTENTS = [
    ("SYSTEM_COMMAND","app_launch","read_only","success"),
    ("SYSTEM_COMMAND","system_stats","read_only","success"),
    ("WORKFLOW_GENERATION","git_workflow","reversible","success"),
    ("WORKFLOW_GENERATION","run_tests","reversible","success"),
    ("CONTENT_GENERATION","commit_message","read_only","success"),
    ("CONTENT_GENERATION","email_draft","read_only","success"),
    ("SYSTEM_COMMAND","process_kill","irreversible","cancelled"),
]

STYLE_RULES = [
    ("email","User prefers concise bullet points over long prose in emails.",
     "- rewrote 3 paragraphs into 4 bullets"),
    ("commit","User writes commit subjects in imperative mood under 50 chars, no period.",
     "changed 'Added login' to 'Add login'"),
    ("commit","User prefixes commits with a conventional-commit type (feat:, fix:, chore:).",
     "prepended 'fix: ' to the subject"),
    ("code","User prefers explicit type hints and Google-style docstrings.",
     "added -> None and Args/Returns sections"),
    ("general","User prefers metric units and 24-hour time.",
     "changed '3pm' to '15:00'"),
]

def build_interactions(n_days=30):
    rows, now = [], datetime.now()
    for d in range(n_days):
        for _ in range(random.randint(8, 20)):
            tr, it, risk, out = random.choice(INTENTS)
            ts = now - timedelta(days=d, hours=random.randint(0,23), minutes=random.randint(0,59))
            corr = "adjusted wording" if (it in ("email_draft","commit_message") and random.random()<0.3) else None
            rows.append((ts.isoformat(timespec="seconds"),
                         f"({it}) synthetic utterance", tr, it,
                         f"tool:{it}", risk, out, corr))
    return rows

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--report", action="store_true")
    args = ap.parse_args()

    if args.report:
        con = sqlite3.connect(DB); cur = con.cursor()
        print("Interactions:", cur.execute("SELECT COUNT(*) FROM interactions").fetchone()[0])
        print("\nTop intents:")
        for intent, c in cur.execute(
                "SELECT intent, COUNT(*) c FROM interactions GROUP BY intent ORDER BY c DESC"):
            print(f"  {intent:16} {c}")
        print("\nStyle rules learned:")
        for cat, rule in cur.execute("SELECT category, rule FROM style_rules"):
            print(f"  [{cat}] {rule}")
        print("\nMost-used apps:")
        for app, n in cur.execute("SELECT app, launches FROM app_frequency ORDER BY launches DESC"):
            print(f"  {app:16} {n}")
        con.close(); return

    if os.path.exists(DB): os.remove(DB)
    con = sqlite3.connect(DB)
    with open(SCHEMA, encoding="utf-8") as f: con.executescript(f.read())
    cur = con.cursor()

    inter = build_interactions()
    cur.executemany(
        "INSERT INTO interactions(ts,utterance,track,intent,action_taken,risk_tier,outcome,user_correction)"
        " VALUES (?,?,?,?,?,?,?,?)", inter)

    now = datetime.now().isoformat(timespec="seconds")
    cur.executemany(
        "INSERT INTO style_rules(ts,category,rule,source_diff,weight,embedding_id) VALUES (?,?,?,?,?,?)",
        [(now, c, r, d, round(random.uniform(1.0,2.5),2), f"emb_{i}")
         for i,(c,r,d) in enumerate(STYLE_RULES)])

    cur.execute(
        "INSERT INTO workflows(name,step_sequence,occurrences,last_seen,automatable) VALUES (?,?,?,?,?)",
        ("python project bootstrap",
         json.dumps(["project_scaffold","env_setup","package_install","git_workflow"]),
         7, now, 1))

    # roll app_frequency up from the interaction launches
    freq = {a: random.randint(20, 200) for a in APPS}
    cur.executemany(
        "INSERT INTO app_frequency(app,launches,last_used,peak_hour) VALUES (?,?,?,?)",
        [(a, n, now, random.choice([9,10,11,14,15,21])) for a, n in freq.items()])

    con.commit()
    print(f"Seeded behavior.db: {len(inter)} interactions, {len(STYLE_RULES)} style rules, "
          f"1 workflow, {len(APPS)} apps")
    con.close()

if __name__ == "__main__":
    main()
