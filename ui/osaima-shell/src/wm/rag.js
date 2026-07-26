/**
 * rag.js — a self-contained Retrieval-Augmented Generation engine.
 *
 * Matches the OSAIMA PDR "RAG Engine" subsystem (§3.5): index knowledge sources,
 * embed them, retrieve the most relevant chunks for a query, and feed them to the
 * answer step. The PDR's production stack uses sentence-transformers + ChromaDB;
 * here we implement the same pipeline (ingest → chunk → embed → retrieve →
 * re-rank → answer) with a dependency-free **TF-IDF vector space** so it runs in
 * the bundler-less browser shell. The embedding function is isolated behind
 * `embed()`, so a real MiniLM model can be dropped in later without touching the
 * retrieval or answer logic.
 *
 *   ingest(doc)  → chunk into passages, tokenize
 *   buildIndex() → compute IDF across the corpus
 *   query(q, k)  → cosine-similarity retrieve top-k, then keyword re-rank
 *   answer(q)    → compose an extractive answer + cite sources
 */

const STOPWORDS = new Set(
  ('a an the of to in on at for and or but is are was were be been being this that '
  + 'these those it its with as by from your you i we they he she do does did how '
  + 'what when where why which who can could should would will shall may might must '
  + 'not no yes if then else than so such about into over under up down out').split(' '),
);

// Crude stemmer: fold plurals / -ing so "closes"/"close", "windows"/"window",
// "running"/"run" match. Applied to both documents and queries for consistency.
function stem(t) {
  if (t.length > 5 && t.endsWith('ing')) return t.slice(0, -3);
  if (t.length > 4 && t.endsWith('ed')) return t.slice(0, -2);
  if (t.length > 3 && t.endsWith('es')) return t.slice(0, -2);
  if (t.length > 3 && t.endsWith('s') && !t.endsWith('ss')) return t.slice(0, -1);
  return t;
}

function tokenize(text) {
  return String(text)
    .toLowerCase()
    .replace(/[^a-z0-9\s.+#-]/g, ' ')
    .split(/\s+/)
    .filter((t) => t.length > 1 && !STOPWORDS.has(t))
    .map(stem);
}

// Build a term-frequency map for a token list.
function termFreq(tokens) {
  const tf = new Map();
  for (const t of tokens) tf.set(t, (tf.get(t) || 0) + 1);
  return tf;
}

export class RagEngine {
  constructor() {
    this.docs = [];       // { id, title, source }
    this.chunks = [];     // { id, docId, title, source, text, tf, len }
    this.idf = new Map(); // term -> inverse document frequency
    this._seq = 0;
  }

  // Split text into passages (~45 words), then index each as a chunk.
  ingest({ title, source = 'user', text }) {
    const docId = ++this._seq;
    this.docs.push({ id: docId, title, source });

    const words = String(text).replace(/\s+/g, ' ').trim().split(' ');
    const CHUNK = 45;
    const OVERLAP = 10;
    for (let i = 0; i < words.length; i += (CHUNK - OVERLAP)) {
      const slice = words.slice(i, i + CHUNK).join(' ').trim();
      if (!slice) continue;
      const tokens = tokenize(slice);
      if (tokens.length === 0) continue;
      this.chunks.push({
        id: this.chunks.length,
        docId, title, source,
        text: slice,
        tf: termFreq(tokens),
        len: tokens.length,
      });
      if (i + CHUNK >= words.length) break;
    }
    return docId;
  }

  // Compute IDF over all chunks (call after ingesting the corpus).
  buildIndex() {
    const N = this.chunks.length || 1;
    const df = new Map();
    for (const c of this.chunks) {
      for (const term of c.tf.keys()) df.set(term, (df.get(term) || 0) + 1);
    }
    this.idf.clear();
    for (const [term, d] of df) {
      this.idf.set(term, Math.log(1 + N / d));
    }
  }

  // TF-IDF weight of a term inside a chunk (or a raw tf map).
  _weight(term, tf, len) {
    const idf = this.idf.get(term) || Math.log(1 + this.chunks.length);
    return (tf / (len || 1)) * idf;
  }

  // Retrieve top-k chunks for a query by cosine similarity, then keyword re-rank.
  query(q, k = 3) {
    const qTokens = tokenize(q);
    if (qTokens.length === 0) return [];
    const qtf = termFreq(qTokens);
    const qLen = qTokens.length;

    // Precompute query vector norm.
    let qNorm = 0;
    const qvec = new Map();
    for (const [term, f] of qtf) {
      const w = this._weight(term, f, qLen);
      qvec.set(term, w);
      qNorm += w * w;
    }
    qNorm = Math.sqrt(qNorm) || 1;

    const scored = this.chunks.map((c) => {
      let dot = 0, cNorm = 0;
      for (const [term, f] of c.tf) {
        const w = this._weight(term, f, c.len);
        cNorm += w * w;
        if (qvec.has(term)) dot += w * qvec.get(term);
      }
      cNorm = Math.sqrt(cNorm) || 1;
      let score = dot / (qNorm * cNorm);
      // Re-rank: small boost for exact query-term overlap count (precision).
      const overlap = [...qtf.keys()].filter((t) => c.tf.has(t)).length;
      score += overlap * 0.02;
      return { chunk: c, score };
    });

    return scored
      .filter((s) => s.score > 0)
      .sort((a, b) => b.score - a.score)
      .slice(0, k)
      .map((s) => ({ title: s.chunk.title, source: s.chunk.source, text: s.chunk.text, score: s.score }));
  }

  // Compose an extractive answer from the retrieved context + cite sources.
  answer(q) {
    const hits = this.query(q, 3);
    if (hits.length === 0) {
      return { answer: "I don't have anything in my knowledge base about that yet.", sources: [] };
    }
    // Extractive: lead with the best passage, mention supporting sources.
    const best = hits[0];
    const answer = best.text;
    return { answer, sources: hits };
  }

  stats() {
    return { docs: this.docs.length, chunks: this.chunks.length, terms: this.idf.size };
  }
}

// Seed knowledge base — stands in for the PDR's indexed man pages, project
// READMEs and user notes. Ingested at boot so the demo has something to retrieve.
export const SEED_KNOWLEDGE = [
  {
    title: 'OSAIMA — Project Overview',
    source: 'notes',
    text: `OSAIMA is an AI-driven, context-aware operating system interface built on Linux.
      It replaces the traditional GUI and CLI with a generative AI agent that listens, reasons and acts.
      Core technologies: on-device speech recognition, a local large language model, retrieval augmented
      generation for context, behavior learning that adapts to the user, and OS-level control over D-Bus.
      The mission is a privacy preserving, fully local, voice-first Linux interface where the AI agent is
      the primary interaction primitive, not a widget or a shortcut but the OS shell itself.`,
  },
  {
    title: 'Window Manager — Lua config',
    source: 'readme',
    text: `The Interstellar shell window manager is configured in a live Lua file called wm.lua.
      You can set the layout with wm.layout, change gaps between windows with wm.gaps, set the master
      ratio with wm.master_ratio, define workspaces with wm.workspaces, and bind keys with wm.bind.
      Available layouts are tile, monocle, grid, spiral and float. Press Alt plus Tab to cycle layouts.
      Edit the config live in the config app and press Reload or Ctrl Enter to apply instantly.`,
  },
  {
    title: 'Keybindings',
    source: 'readme',
    text: `Alt plus Return opens a terminal. Alt plus a opens the AI assistant. Alt plus e opens files.
      Alt plus p opens the system monitor. Alt plus z opens the task manager. Alt plus c opens the config editor.
      Alt plus r opens the knowledge base. Alt plus b opens the behavior profile. Alt plus q closes the focused
      window. Alt plus j and Alt plus k move focus. Alt plus 1 to 5 switch workspaces. Alt plus Space toggles floating.`,
  },
  {
    title: 'Linux basics — files and processes',
    source: 'manpage',
    text: `To list files use ls. To search file contents use grep or ripgrep. To find files by name use find or fd.
      To see running processes use ps or top. To end a process use kill followed by the process id. To check disk
      usage use du or df. To change file permissions use chmod. Make a script executable with chmod plus x.
      To see memory usage read proc meminfo. To see cpu information read proc cpuinfo.`,
  },
  {
    title: 'Running the project',
    source: 'readme',
    text: `To run the shell for the real OS use npm install and then npm run dev which starts tauri dev.
      For a quick browser-only demo without the Rust backend, serve the folder with python http server on a port
      and open the index page. When the Tauri backend and MCP daemon are not present the shell falls back to mock
      system stats so the interface is always demoable. The window manager and all apps run with no backend.`,
  },
];
