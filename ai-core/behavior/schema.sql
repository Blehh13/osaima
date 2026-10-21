-- OSAIMA Behavior Store schema (PDR §3.7, §5.7)
-- On-device SQLite at ~/.local/share/osaima/behavior.db
-- Learns who the user is and how they work; feeds style rules + next-app
-- prediction into the LLM system prompt. Style-rule EMBEDDINGS live in ChromaDB;
-- this file holds the relational tables.

PRAGMA journal_mode = WAL;

-- Every utterance -> action -> outcome, append-only (never updated in place).
CREATE TABLE IF NOT EXISTS interactions (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    ts              TEXT    NOT NULL,               -- ISO-8601
    utterance       TEXT    NOT NULL,
    track           TEXT    NOT NULL,               -- SYSTEM_COMMAND | WORKFLOW_GENERATION | CONTENT_GENERATION
    intent          TEXT    NOT NULL,
    action_taken    TEXT,                           -- serialized tool call / command
    risk_tier       TEXT    CHECK(risk_tier IN ('read_only','reversible','irreversible')),
    outcome         TEXT    CHECK(outcome IN ('success','failed','cancelled')),
    user_correction TEXT                            -- what the user changed, if anything
);

-- Learned natural-language style rules (email tone, commit format, code scaffold).
-- embedding_id references the vector stored in the ChromaDB 'style_rules' collection.
CREATE TABLE IF NOT EXISTS style_rules (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    ts            TEXT NOT NULL,
    category      TEXT NOT NULL,                     -- email | commit | code | general
    rule          TEXT NOT NULL,                     -- 'User prefers concise bullet points over prose'
    source_diff   TEXT,                              -- the user edit this rule was distilled from
    weight        REAL DEFAULT 1.0,                  -- reinforced when re-observed
    embedding_id  TEXT
);

-- Detected repeated multi-step patterns (edit-distance clustering of sequences).
CREATE TABLE IF NOT EXISTS workflows (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    name         TEXT,                               -- 'python project bootstrap'
    step_sequence TEXT NOT NULL,                     -- JSON array of intents in order
    occurrences  INTEGER DEFAULT 1,
    last_seen    TEXT,
    automatable  INTEGER DEFAULT 0                   -- surfaced to user as a one-click macro
);

-- Frequency heuristics for proactive suggestions + next-app prediction.
CREATE TABLE IF NOT EXISTS app_frequency (
    app          TEXT PRIMARY KEY,
    launches     INTEGER DEFAULT 0,
    last_used    TEXT,
    peak_hour    INTEGER                             -- 0-23, most common launch hour
);

CREATE INDEX IF NOT EXISTS idx_interactions_intent ON interactions(intent);
CREATE INDEX IF NOT EXISTS idx_interactions_ts     ON interactions(ts);
CREATE INDEX IF NOT EXISTS idx_style_category      ON style_rules(category);
