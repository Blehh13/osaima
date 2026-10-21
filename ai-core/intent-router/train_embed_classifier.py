#!/usr/bin/env python3
"""
Intent classifier on frozen sentence embeddings: all-MiniLM-L6-v2 (the model the
RAG engine already loads) + a logistic-regression head. Trains in seconds on a
CPU, and the head is a few hundred KB, so the router and RAG share one model.

Usage:
    python train_embed_classifier.py            # train on train.jsonl, report testset.jsonl
Output: intent_embed/head.joblib  (+ labels), loaded by predict_embed()
"""
import json, os, statistics, time

import joblib
import numpy as np
from sentence_transformers import SentenceTransformer
from sklearn.linear_model import LogisticRegression
from sklearn.metrics import accuracy_score, f1_score

MODEL = "sentence-transformers/all-MiniLM-L6-v2"
OUT = "intent_embed"


def load(path):
    return [json.loads(l) for l in open(path, encoding="utf-8") if l.strip()]


def main():
    enc = SentenceTransformer(MODEL, device="cpu")
    train = load("train.jsonl") + load("val.jsonl")
    known = {r["intent"] for r in train}
    test = [r for r in load("testset.jsonl") if r["intent"] in known]

    t = time.time()
    X = enc.encode([r["utterance"] for r in train], batch_size=256, normalize_embeddings=True)
    head = LogisticRegression(C=20, max_iter=3000)
    head.fit(X, [r["intent"] for r in train])
    print(f"trained on {len(train)} rows in {time.time() - t:.0f}s")

    Xt = enc.encode([r["utterance"] for r in test], batch_size=256, normalize_embeddings=True)
    pred = head.predict(Xt)
    gold = [r["intent"] for r in test]
    edge = [g == p for r, g, p in zip(test, gold, pred) if r.get("edge_case")]

    lat = []
    for r in test[:200]:  # single-request latency, as the daemon would see it
        s = time.perf_counter()
        head.predict(enc.encode([r["utterance"]], normalize_embeddings=True))
        lat.append((time.perf_counter() - s) * 1000)

    print(json.dumps({
        "data": "testset.jsonl", "n": len(test),
        "accuracy": round(accuracy_score(gold, pred), 4),
        "f1_macro": round(f1_score(gold, pred, average="macro"), 4),
        "edge_case_accuracy": round(sum(edge) / len(edge), 4) if edge else None,
        "latency_ms_p50": round(statistics.median(lat), 2),
    }, indent=2))

    # Cascade: answer on the fast path only when confident, otherwise hand the
    # utterance to the LLM. Coverage = share answered without the LLM.
    conf = head.predict_proba(Xt).max(axis=1)
    print("threshold  coverage  fast-path accuracy")
    for th in (0.0, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8):
        sel = conf >= th
        if sel.any():
            acc = np.mean([g == p for g, p, s in zip(gold, pred, sel) if s])
            print(f"  {th:.1f}       {sel.mean():6.1%}    {acc:6.1%}")

    os.makedirs(OUT, exist_ok=True)
    joblib.dump({"model": MODEL, "head": head}, os.path.join(OUT, "head.joblib"))
    print(f"Saved -> {OUT}/head.joblib")


if __name__ == "__main__":
    main()
