#!/usr/bin/env python3
"""
OSAIMA knowledge index: the retrieval half of RAG (PDR §3.5).

Chunks the user's documents and the project docs, embeds them with the same
all-MiniLM-L6-v2 encoder the intent router already holds in memory, and keeps
them as a plain matrix on disk. For a few thousand chunks exact cosine search
takes milliseconds, so no vector database is needed on the ISO.

    python knowledge.py build [DIR ...]      # default: ~/Documents + project docs
    python knowledge.py search "how do I run the tests"

The router service loads the index and answers {"search": "...", "k": 5}.
"""
import argparse
import json
import os
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.abspath(os.path.join(HERE, "..", ".."))
INDEX_DIR = os.environ.get("OSAIMA_KNOWLEDGE_DIR",
                           os.path.join(os.environ.get("XDG_DATA_HOME", os.path.expanduser("~/.local/share")), "osaima", "knowledge"))
DEFAULT_SOURCES = [os.path.expanduser("~/Documents"), os.path.join(REPO, "ai-core", "rag-engine", "seed_corpus"),
                   os.path.join(REPO, "README.md"), os.path.join(REPO, "ARCHITECTURE.md"),
                   os.path.join(REPO, "BUILD-DISTRO.md"), os.path.join(REPO, "ui", "osaima-shell", "README.md")]
EXTENSIONS = (".md", ".txt", ".rst")
SKIP_DIRS = {"node_modules", ".git", "target", "venv", ".venv", "__pycache__", "kernel", "portage"}
CHUNK_WORDS, OVERLAP_WORDS = 220, 40   # ~300 tokens: small enough to keep CPU prompts short
MAX_FILE_BYTES = 2_000_000


def iter_files(sources):
    for src in sources:
        if os.path.isfile(src) and src.endswith(EXTENSIONS):
            yield src
        elif os.path.isdir(src):
            for root, dirs, files in os.walk(src):
                dirs[:] = [d for d in dirs if d not in SKIP_DIRS and not d.startswith(".")]
                for f in files:
                    p = os.path.join(root, f)
                    if f.endswith(EXTENSIONS) and os.path.getsize(p) <= MAX_FILE_BYTES:
                        yield p


def chunk(text):
    """Split on headings/paragraphs, then pack into overlapping ~CHUNK_WORDS windows."""
    blocks = [b.strip() for b in re.split(r"\n(?=#{1,6} )|\n\s*\n", text) if b.strip()]
    words, out = [], []
    for b in blocks:
        words.extend(b.split())
        while len(words) >= CHUNK_WORDS:
            out.append(" ".join(words[:CHUNK_WORDS]))
            words = words[CHUNK_WORDS - OVERLAP_WORDS:]
    if words:
        out.append(" ".join(words))
    return out


class Index:
    def __init__(self, embed, path=INDEX_DIR):
        import numpy as np
        self.np, self.embed, self.path = np, embed, path
        self.vectors, self.chunks = None, []
        if os.path.exists(os.path.join(path, "vectors.npy")):
            self.vectors = np.load(os.path.join(path, "vectors.npy"))
            self.chunks = json.load(open(os.path.join(path, "chunks.json"), encoding="utf-8"))

    def build(self, sources):
        np = self.np
        chunks = []
        for f in iter_files(sources):
            try:
                text = open(f, encoding="utf-8", errors="ignore").read()
            except OSError:
                continue
            if f.startswith(REPO + os.sep):
                title = "osaima/" + os.path.relpath(f, REPO)
            elif f.startswith(os.path.expanduser("~")):
                title = "~/" + os.path.relpath(f, os.path.expanduser("~"))
            else:
                title = f
            chunks += [{"source": title, "text": c} for c in chunk(text)]
        vecs = np.stack([self.embed(c["text"]) for c in chunks]) if chunks else np.zeros((0, 384), np.float32)
        os.makedirs(self.path, exist_ok=True)
        np.save(os.path.join(self.path, "vectors.npy"), vecs.astype(np.float32))
        json.dump(chunks, open(os.path.join(self.path, "chunks.json"), "w", encoding="utf-8"))
        self.vectors, self.chunks = vecs, chunks
        return len({c["source"] for c in chunks}), len(chunks)

    def search(self, query, k=5, min_score=0.3):
        if self.vectors is None or not len(self.chunks):
            return []
        scores = self.vectors @ self.embed(query)  # embeddings are L2-normalised: dot = cosine
        top = scores.argsort()[::-1][:k]
        return [{"source": self.chunks[i]["source"], "text": self.chunks[i]["text"], "score": round(float(scores[i]), 3)}
                for i in top if scores[i] >= min_score]


def main():
    ap = argparse.ArgumentParser()
    sub = ap.add_subparsers(dest="cmd", required=True)
    b = sub.add_parser("build")
    b.add_argument("dirs", nargs="*")
    s = sub.add_parser("search")
    s.add_argument("query")
    s.add_argument("-k", type=int, default=5)
    args = ap.parse_args()

    sys.path.insert(0, HERE)
    import router_service
    model = router_service.Router(warm=False).model
    index = Index(model.embed)
    if args.cmd == "build":
        files, chunks = index.build(args.dirs or DEFAULT_SOURCES)
        print(f"indexed {files} files, {chunks} chunks -> {INDEX_DIR}")
    else:
        for h in index.search(args.query, args.k):
            print(f"{h['score']:.2f}  {h['source']}\n      {h['text'][:160]}...")


if __name__ == "__main__":
    main()
