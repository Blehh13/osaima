#!/usr/bin/env python3
"""
End-to-end check of the fast path on the held-out set: how many requests the
router answers without the LLM, and how often that answer has the right intent.

Usage: python eval_cascade.py [--slot-threshold 0.35] [--threshold 0.7]
"""
import argparse, json, os, statistics


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--threshold", type=float)
    ap.add_argument("--slot-threshold", type=float)
    ap.add_argument("--data", default="testset.jsonl")
    args = ap.parse_args()
    if args.threshold is not None:
        os.environ["OSAIMA_ROUTER_THRESHOLD"] = str(args.threshold)
    if args.slot_threshold is not None:
        os.environ["OSAIMA_ROUTER_SLOT_THRESHOLD"] = str(args.slot_threshold)
    import router_service, taxonomy

    router = router_service.Router()
    rows = [json.loads(l) for l in open(args.data, encoding="utf-8") if l.strip()]
    fast, right, ms = 0, 0, []
    handled = [r for r in rows if r["intent"] in router_service.SHELL_HANDLED]
    for r in rows:
        res = router.classify(r["utterance"])
        ms.append(res["ms"])
        if res["fast"]:
            fast += 1
            right += res["intent"] == r["intent"]
    print(json.dumps({
        "rows": len(rows),
        "rows_whose_intent_the_shell_handles": len(handled),
        "answered_on_fast_path": fast,
        "fast_path_share_of_all": round(fast / len(rows), 4),
        "fast_path_share_of_shell_handled": round(fast / len(handled), 4),
        "fast_path_accuracy": round(right / fast, 4) if fast else None,
        "wrong_fast_answers": fast - right,
        "latency_ms_p50": round(statistics.median(ms), 2),
    }, indent=2))


if __name__ == "__main__":
    main()
