#!/usr/bin/env python3
"""
Evaluate a trained intent model on a labelled JSONL set (default: the held-out
paraphrase set, testset.jsonl) and report accuracy, macro-F1, CPU latency and
the most confused intent pairs. This is the PDR §9.1 number, since testset.jsonl
uses phrasings the model never trained on.

Usage:
    python evaluate.py                        # intent_bert_tiny on testset.jsonl
    python evaluate.py --data test.jsonl      # in-distribution split, for comparison
    python evaluate.py --no-edge              # exclude the hand-written edge cases
"""
import argparse, json, statistics, time
from collections import Counter


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--model", default="intent_bert_tiny")
    ap.add_argument("--data", default="testset.jsonl")
    ap.add_argument("--no-edge", action="store_true")
    ap.add_argument("--json", metavar="PATH", help="also write the metrics here")
    args = ap.parse_args()

    import torch
    from transformers import AutoTokenizer, BertForSequenceClassification
    from sklearn.metrics import accuracy_score, f1_score

    torch.set_num_threads(1)  # latency of one request on one core, as the daemon would see it
    tok = AutoTokenizer.from_pretrained(args.model)
    model = BertForSequenceClassification.from_pretrained(args.model).eval()
    id2label = model.config.id2label

    rows = [json.loads(l) for l in open(args.data, encoding="utf-8") if l.strip()]
    if args.no_edge:
        rows = [r for r in rows if not r.get("edge_case")]
    known = set(model.config.label2id)
    unknown = Counter(r["intent"] for r in rows if r["intent"] not in known)
    rows = [r for r in rows if r["intent"] in known]

    preds, lat = [], []
    with torch.no_grad():
        for r in rows:
            t = time.perf_counter()
            enc = tok(r["utterance"], return_tensors="pt", truncation=True, max_length=32)
            logits = model(**enc).logits
            lat.append((time.perf_counter() - t) * 1000)
            preds.append(id2label[int(logits.argmax(-1))])

    gold = [r["intent"] for r in rows]
    acc = accuracy_score(gold, preds)
    f1 = f1_score(gold, preds, average="macro")
    lat.sort()
    edge = [(g, p) for r, g, p in zip(rows, gold, preds) if r.get("edge_case")]
    metrics = {
        "data": args.data, "n": len(rows), "accuracy": round(acc, 4), "f1_macro": round(f1, 4),
        "edge_case_accuracy": round(sum(g == p for g, p in edge) / len(edge), 4) if edge else None,
        "latency_ms_p50": round(statistics.median(lat), 2),
        "latency_ms_p95": round(lat[int(len(lat) * 0.95) - 1], 2),
    }
    print(json.dumps(metrics, indent=2))
    if unknown:
        print(f"skipped {sum(unknown.values())} rows with intents the model does not know: {dict(unknown)}")
    confused = Counter((g, p) for g, p in zip(gold, preds) if g != p).most_common(8)
    if confused:
        print("most confused (gold -> predicted):")
        for (g, p), n in confused:
            print(f"  {n:4d}  {g} -> {p}")
    if args.json:
        json.dump(metrics, open(args.json, "w"), indent=2)


if __name__ == "__main__":
    main()
