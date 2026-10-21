#!/usr/bin/env python3
"""
OSAIMA intent router service: the fast path of the NLU cascade (PDR §3.3).

Loads all-MiniLM-L6-v2 (shared with the RAG engine) and the intent head trained
by train_embed_classifier.py once, then answers classification requests over a
Unix socket in ~15 ms, so the daemon only calls the LLM when it must.

Backends: the ONNX export from export_onnx.py (onnxruntime + tokenizers + numpy,
no PyTorch; ~100 MB RSS) when present, else sentence-transformers. OSAIMA_ROUTER_BACKEND
= onnx | torch forces one.

Protocol: one JSON object per line.
  request  {"text": "set a timer for 5 minutes"}
  response {"intent": "timer", "track": "SYSTEM_COMMAND", "confidence": 0.93,
            "fast": {"intent": "timer", "params": {"seconds": 300}, "reply": "Timer set."},
            "ms": 12.4}
`fast` is null when the daemon should use the LLM instead: confidence too low,
a generation intent, an intent the shell cannot run, or a missing parameter.

Socket: $OSAIMA_ROUTER_SOCKET, else $XDG_RUNTIME_DIR/osaima/router.sock, else /tmp.
"""
import json
import os
import socketserver
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

import slots  # noqa: E402
import taxonomy  # noqa: E402

THRESHOLD = float(os.environ.get("OSAIMA_ROUTER_THRESHOLD", "0.7"))
# When the extractor finds a concrete value for these (a registered app, a
# duration, a layout...), that is independent evidence for the intent, so a
# lower classifier confidence is enough.
SLOT_CONFIRMED = float(os.environ.get("OSAIMA_ROUTER_SLOT_THRESHOLD", "0.35"))
SLOT_INTENTS = {"open_app", "focus_app", "layout", "workspace", "timer", "calculator", "search_web"}
# Intents that act immediately and are hard to undo need more certainty.
STRICT = {"power_control": 0.9, "close_all_windows": 0.9, "delete_note": 0.9}
# Fast-path intents: the ones the shell's dispatchIntent() can execute directly.
SHELL_HANDLED = {
    "open_app", "close_window", "close_all_windows", "minimize_all", "focus_app", "layout", "workspace",
    "set_gaps", "system_status", "optimize_cpu", "wake_apps", "clean_storage", "fix_network", "timer",
    "stop_timer", "reminder", "calendar", "calculator", "weather", "search_web", "scratchpad",
    "list_notes", "view_note", "delete_note", "search_notes", "greeting", "help",
    # OS controls, executed by the daemon (system.os_action) via the shell's os_action handler
    *slots.OS_ACTIONS,
}


def socket_path():
    if os.environ.get("OSAIMA_ROUTER_SOCKET"):
        return os.environ["OSAIMA_ROUTER_SOCKET"]
    run = os.environ.get("XDG_RUNTIME_DIR")
    return os.path.join(run, "osaima", "router.sock") if run else "/tmp/osaima_router.sock"


class OnnxModel:
    """MiniLM (mean-pooled, normalised) + logistic-regression head, without PyTorch."""

    def __init__(self, model_dir):
        import numpy as np
        import onnxruntime as ort
        from tokenizers import Tokenizer
        self.np = np
        opts = ort.SessionOptions()
        opts.intra_op_num_threads = int(os.environ.get("OSAIMA_ROUTER_THREADS", min(4, os.cpu_count() or 1)))
        self.session = ort.InferenceSession(os.path.join(model_dir, os.environ.get("OSAIMA_ROUTER_ONNX", "encoder.onnx")), opts,
                                            providers=["CPUExecutionProvider"])
        self.tok = Tokenizer.from_file(os.path.join(model_dir, "tokenizer.json"))
        self.tok.enable_truncation(128)
        head = np.load(os.path.join(model_dir, "head.npz"))
        self.coef, self.intercept, self.classes = head["coef"], head["intercept"], head["classes"]

    def embed(self, text):
        """L2-normalised sentence embedding (shared with the knowledge index)."""
        np = self.np
        enc = self.tok.encode(text)
        feeds = {name: np.array([values], dtype=np.int64) for name, values in
                 (("input_ids", enc.ids), ("attention_mask", enc.attention_mask), ("token_type_ids", enc.type_ids))}
        return self.session.run(["embedding"], feeds)[0][0]

    def probabilities(self, text):
        np = self.np
        logits = self.embed(text)[None, :] @ self.coef.T + self.intercept  # multinomial logistic regression
        e = np.exp(logits - logits.max())
        return (e / e.sum())[0]


class TorchModel:
    def __init__(self, model_dir):
        import joblib
        from sentence_transformers import SentenceTransformer
        bundle = joblib.load(os.path.join(model_dir, "head.joblib"))
        self.head, self.classes = bundle["head"], bundle["head"].classes_
        self.encoder = SentenceTransformer(bundle["model"], device="cpu")

    def embed(self, text):
        return self.encoder.encode([text], normalize_embeddings=True)[0]

    def probabilities(self, text):
        return self.head.predict_proba(self.embed(text)[None, :])[0]


class Router:
    def __init__(self, model_dir=os.path.join(HERE, "intent_embed"), warm=True):
        backend = os.environ.get("OSAIMA_ROUTER_BACKEND")
        if backend is None:
            backend = "onnx" if os.path.exists(os.path.join(model_dir, "encoder.onnx")) else "torch"
        self.backend = backend
        self.model = OnnxModel(model_dir) if backend == "onnx" else TorchModel(model_dir)
        import knowledge
        self.knowledge = knowledge.Index(self.model.embed)
        if warm:
            self.classify("warm up")  # first call pays the lazy-init cost

    def classify(self, text):
        start = time.perf_counter()
        probs = self.model.probabilities(text)
        best = int(probs.argmax())
        intent, conf = str(self.model.classes[best]), float(probs[best])
        # Top guesses in the shell's naming, so the LLM fallback can be given a
        # short prompt covering only these intents.
        top = [str(self.model.classes[i]) for i in probs.argsort()[::-1][:5]]
        candidates = list(dict.fromkeys(taxonomy.SHELL_NAME.get(c, c) for c in top))
        # Confidence per track (summed over its intents): the request *type* is often
        # clear even when the probability is split between similar intents.
        track_conf = {}
        for i, c in enumerate(self.model.classes):
            t = taxonomy.track_of(str(c))
            track_conf[t] = track_conf.get(t, 0.0) + float(probs[i])
        result = {"intent": intent, "track": taxonomy.track_of(intent), "confidence": round(conf, 4),
                  "track_confidence": {t: round(v, 4) for t, v in track_conf.items()},
                  "candidates": candidates, "fast": None}

        floor = SLOT_CONFIRMED if intent in SLOT_INTENTS else STRICT.get(intent, THRESHOLD)
        if intent in SHELL_HANDLED and conf >= floor:
            params = slots.extract(intent, text)
            if params is not None:
                if intent in slots.OS_ACTIONS:
                    shell_intent = "os_action"
                    reply = f"{params['action'].replace('_', ' ').capitalize()}: {params['op']}."
                else:
                    shell_intent = taxonomy.SHELL_NAME.get(intent, intent)
                    reply = slots.reply_for(intent if intent == "help" else shell_intent, params)
                    if shell_intent == "help":
                        shell_intent = "greeting"  # the shell has no help case; the reply carries it
                result["fast"] = {"intent": shell_intent, "params": params, "reply": reply}
        result["ms"] = round((time.perf_counter() - start) * 1000, 2)
        return result


class Handler(socketserver.StreamRequestHandler):
    def handle(self):
        for line in self.rfile:
            try:
                req = json.loads(line)
                if "search" in req:
                    start = time.perf_counter()
                    hits = self.server.router.knowledge.search(str(req["search"]), int(req.get("k", 5)))
                    resp = {"hits": hits, "ms": round((time.perf_counter() - start) * 1000, 2)}
                else:
                    resp = self.server.router.classify(str(req.get("text", "")))
            except Exception as e:  # never kill the service on one bad request
                resp = {"error": str(e)}
            self.wfile.write((json.dumps(resp) + "\n").encode())
            self.wfile.flush()


class Server(socketserver.ThreadingUnixStreamServer):
    daemon_threads = True


def main():
    if len(sys.argv) > 1:  # one-off: python router_service.py "open the terminal"
        print(json.dumps(Router().classify(" ".join(sys.argv[1:])), indent=2))
        return
    path = socket_path()
    os.makedirs(os.path.dirname(path), exist_ok=True)
    if os.path.exists(path):
        os.unlink(path)
    router = Router()
    with Server(path, Handler) as srv:
        srv.router = router
        os.chmod(path, 0o600)
        print(f"OSAIMA router listening on {path} ({router.backend} backend, threshold {THRESHOLD}, "
              f"{len(router.knowledge.chunks)} knowledge chunks)", flush=True)
        srv.serve_forever()


if __name__ == "__main__":
    main()
