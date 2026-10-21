#!/usr/bin/env python3
"""
OSAIMA RAG chunker (PDR §3.5, §5.5): recursive splitter, 512-token chunks with
50-token overlap, code blocks preserved intact.

Dependency-free approximation of LangChain's RecursiveCharacterTextSplitter:
splits on paragraph -> line -> sentence -> word boundaries, keeps fenced code
blocks whole, and uses a ~4-chars-per-token heuristic (no tokenizer needed).

Usage:
    python chunker.py seed_corpus/            # chunk every .md/.txt in a folder
    python chunker.py seed_corpus/ -o chunks.jsonl
Output row: {"id", "source", "chunk_index", "text", "approx_tokens"}
"""
import sys, os, json, re, argparse

CHARS_PER_TOKEN = 4
def toks(s): return max(1, len(s) // CHARS_PER_TOKEN)

FENCE = re.compile(r"```.*?```", re.DOTALL)

def split_preserving_code(text):
    """Yield segments; fenced code blocks come out as single atomic segments."""
    pos = 0
    for m in FENCE.finditer(text):
        if m.start() > pos:
            yield ("text", text[pos:m.start()])
        yield ("code", m.group(0))       # keep code intact
        pos = m.end()
    if pos < len(text):
        yield ("text", text[pos:])

def recursive_split(segment, max_tok=512):
    """Split a plain-text segment on the coarsest boundary that fits."""
    if toks(segment) <= max_tok:
        return [segment]
    for sep in ("\n\n", "\n", ". ", " "):
        if sep in segment:
            parts, buf, out = segment.split(sep), "", []
            for p in parts:
                cand = (buf + sep + p) if buf else p
                if toks(cand) > max_tok and buf:
                    out.append(buf); buf = p
                else:
                    buf = cand
            if buf: out.append(buf)
            # any oversized piece recurses on the next-finer separator
            final = []
            for o in out:
                final.extend(recursive_split(o, max_tok) if toks(o) > max_tok else [o])
            return final
    return [segment]  # unsplittable

def chunk_document(text, max_tok=512, overlap_tok=50):
    pieces = []
    for kind, seg in split_preserving_code(text):
        if kind == "code":
            pieces.append(seg)                       # never split code
        else:
            pieces.extend(recursive_split(seg.strip(), max_tok))
    pieces = [p for p in pieces if p.strip()]

    # add 50-token overlap by prefixing the tail of the previous chunk
    chunks, overlap_chars = [], overlap_tok * CHARS_PER_TOKEN
    for i, p in enumerate(pieces):
        if i > 0 and not pieces[i-1].startswith("```"):
            tail = pieces[i-1][-overlap_chars:]
            p = tail + "\n" + p
        chunks.append(p)
    return chunks

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("path", help="file or folder of .md/.txt to chunk")
    ap.add_argument("-o", "--out", default="chunks.jsonl")
    ap.add_argument("--max-tokens", type=int, default=512)
    ap.add_argument("--overlap", type=int, default=50)
    args = ap.parse_args()

    files = []
    if os.path.isdir(args.path):
        for root, _, names in os.walk(args.path):
            for n in names:
                if n.endswith((".md", ".txt")): files.append(os.path.join(root, n))
    else:
        files = [args.path]

    rows, cid = [], 0
    for fp in sorted(files):
        with open(fp, encoding="utf-8") as f:
            text = f.read()
        for idx, ch in enumerate(chunk_document(text, args.max_tokens, args.overlap)):
            rows.append({"id": cid, "source": os.path.relpath(fp),
                         "chunk_index": idx, "text": ch, "approx_tokens": toks(ch)})
            cid += 1

    with open(args.out, "w", encoding="utf-8") as f:
        for r in rows: f.write(json.dumps(r, ensure_ascii=False) + "\n")
    print(f"{len(files)} docs -> {len(rows)} chunks -> {args.out}")
    if rows:
        print(f"avg tokens/chunk: {sum(r['approx_tokens'] for r in rows)//len(rows)}")

if __name__ == "__main__":
    main()
