#!/usr/bin/env python3
"""
Gemini as the teacher, used sparingly: one call labels a whole batch of requests.

The key is read from ~/.config/osaima/llm.env (GEMINI_API_KEY) into memory only;
it is never printed, logged or written anywhere. Free tier: 15 requests/min per
model, so calls are spaced and a busy model falls through to the next one.
"""
import json
import os
import time
import urllib.error
import urllib.request

URL = "https://generativelanguage.googleapis.com/v1beta/openai/chat/completions"
MODELS = [m for m in os.environ.get("OSAIMA_TEACHER_MODELS", "gemini-3.5-flash,gemini-3.5-flash-lite").split(",") if m]
MIN_GAP = 4.5  # seconds between calls to one model: stays under 15/min
_last = {}


def _key():
    key = os.environ.get("GEMINI_API_KEY")
    if key:
        return key
    path = os.path.join(os.environ.get("XDG_CONFIG_HOME", os.path.expanduser("~/.config")), "osaima", "llm.env")
    with open(path) as f:
        for line in f:
            if line.startswith("GEMINI_API_KEY="):
                return line.split("=", 1)[1].strip().strip('"')
    raise SystemExit("GEMINI_API_KEY not found in llm.env")


def chat(messages, model=None, json_mode=True, max_tokens=16000, timeout=180):
    """One chat completion; returns (text, model, seconds). Tries each model in turn."""
    key = _key()
    models = [model] if model else MODELS
    errors = []
    for attempt in range(6):
        for m in models:
            wait = MIN_GAP - (time.time() - _last.get(m, 0))
            if wait > 0:
                time.sleep(wait)
            body = {"model": m, "messages": messages, "temperature": 0.2, "max_tokens": max_tokens}
            if json_mode:
                body["response_format"] = {"type": "json_object"}
            req = urllib.request.Request(URL, data=json.dumps(body).encode(), method="POST",
                                         headers={"Content-Type": "application/json", "Authorization": "Bearer " + key})
            start = time.time()
            _last[m] = start
            try:
                with urllib.request.urlopen(req, timeout=timeout) as r:
                    d = json.load(r)
                text = d["choices"][0]["message"].get("content") or ""
                return text, m, time.time() - start
            except urllib.error.HTTPError as e:
                errors.append(f"{m}: HTTP {e.code}")
            except Exception as e:  # timeouts, resets
                errors.append(f"{m}: {type(e).__name__}")
        time.sleep(15 * (attempt + 1))
    raise RuntimeError("teacher failed: " + "; ".join(errors[-6:]))


def parse_json(text):
    t = text.strip()
    if t.startswith("```"):
        t = t.strip("`")
        t = t[t.find("{"):]
    return json.loads(t[t.find("{"): t.rfind("}") + 1])


if __name__ == "__main__":
    text, m, s = chat([{"role": "user", "content": 'Reply with {"ok": true}'}])
    print(m, round(s, 2), text)
