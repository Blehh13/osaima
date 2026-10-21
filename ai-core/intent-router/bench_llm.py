#!/usr/bin/env python3
"""
Benchmark the LLM fallback through the running daemon (system.classify_intent).

Uses held-out test sentences that the router does NOT answer on the fast path
(so they really reach the LLM) and whose gold intent is one the shell's intent
prompt knows. Reports intent accuracy, valid-response rate and latency.

Run once per configuration, e.g. different models behind ai-core/llm/start-llm.sh,
or with the router service stopped to measure the full (unfocused) prompt.

Usage: python bench_llm.py --label phi4-focused [--n 24] [--out bench_llm.jsonl]
"""
import argparse, json, os, random, re, socket, statistics, time

HERE = os.path.dirname(os.path.abspath(__file__))
PROMPT = os.path.join(HERE, "..", "mcp-daemon", "src", "prompts", "intent.txt")


def daemon_socket():
    run = os.environ.get("XDG_RUNTIME_DIR")
    return os.environ.get("OSAIMA_MCP_SOCKET") or (os.path.join(run, "osaima", "mcp.sock") if run else "/tmp/osaima_mcp.sock")


def rpc(method, params, timeout=600):
    s = socket.socket(socket.AF_UNIX)
    s.settimeout(timeout)
    s.connect(daemon_socket())
    s.sendall((json.dumps({"jsonrpc": "2.0", "id": "1", "method": method, "params": params}) + "\n").encode())
    buf = b""
    while not buf.endswith(b"\n"):
        chunk = s.recv(65536)
        if not chunk:
            break
        buf += chunk
    return json.loads(buf)


def pick_queries(n, seed=7):
    """Deterministic sample, so every configuration sees the same sentences."""
    import router_service, taxonomy
    shell_intents = set(re.findall(r"^- ([a-z_]+):", open(PROMPT, encoding="utf-8").read(), re.M))
    router = router_service.Router()
    pool = []
    for line in open(os.path.join(HERE, "testset.jsonl"), encoding="utf-8"):
        r = json.loads(line)
        gold = taxonomy.SHELL_NAME.get(r["intent"], r["intent"])
        if r.get("edge_case") or gold not in shell_intents:
            continue
        if router.classify(r["utterance"])["fast"] is None:
            pool.append({"utterance": r["utterance"], "gold": gold})
    pool = sorted({p["utterance"]: p for p in pool}.values(), key=lambda p: p["utterance"])
    random.Random(seed).shuffle(pool)
    return pool[:n]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--label", required=True)
    ap.add_argument("--n", type=int, default=24)
    ap.add_argument("--queries", default=os.path.join(HERE, "bench_llm_queries.json"))
    ap.add_argument("--out", default=os.path.join(HERE, "bench_llm.jsonl"))
    args = ap.parse_args()

    if not os.path.exists(args.queries):
        json.dump(pick_queries(args.n), open(args.queries, "w"), indent=1)
    queries = json.load(open(args.queries))

    status = rpc("system.llm_status", None)["result"]
    rows = []
    for q in queries:
        t = time.time()
        try:
            resp = rpc("system.classify_intent", {"query": q["utterance"]})
        except Exception as e:
            resp = {"error": {"message": str(e)}}
        dt = time.time() - t
        got = (resp.get("result") or {}).get("intent")
        rows.append({"utterance": q["utterance"], "gold": q["gold"], "got": got, "s": round(dt, 2),
                     "source": (resp.get("result") or {}).get("source"), "error": (resp.get("error") or {}).get("message")})
        print(f"{dt:6.1f}s  {'ok ' if got == q['gold'] else 'BAD'} {q['gold']:<16} {str(got):<16} {q['utterance'][:60]}", flush=True)

    llm_rows = [r for r in rows if r["source"] != "fast"]
    times = sorted(r["s"] for r in llm_rows) or [0]
    summary = {
        "label": args.label, "model": status.get("local", {}).get("models"), "n": len(rows),
        "accuracy": round(sum(r["got"] == r["gold"] for r in rows) / len(rows), 3),
        "valid_rate": round(sum(r["got"] is not None for r in rows) / len(rows), 3),
        "latency_s_p50": statistics.median(times), "latency_s_max": max(times),
    }
    print(json.dumps(summary, indent=2))
    with open(args.out, "a") as f:
        f.write(json.dumps({**summary, "rows": rows}) + "\n")


if __name__ == "__main__":
    main()
