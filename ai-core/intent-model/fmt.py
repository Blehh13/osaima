"""Prompt format shared by train.py, bench.py and the daemon (llm.rs fast_intent_messages)."""
import json
import os

HERE = os.path.dirname(os.path.abspath(__file__))
PROMPT_PATH = os.path.join(HERE, "..", "mcp-daemon", "src", "prompts", "intent_fast.txt")


def system_prompt():
    with open(PROMPT_PATH, encoding="utf-8") as f:
        return f.read().strip()


def user_turn(request, now):
    # must match LlmClient::fast_intent_messages in llm.rs
    return f"{request}\n(Now: {now})"


def messages(request, now):
    return [{"role": "system", "content": system_prompt()}, {"role": "user", "content": user_turn(request, now)}]


def target_text(target):
    """Compact JSON, keys in the order the grammar emits them: intent, params, reply."""
    t = {"intent": target["intent"], "params": target.get("params") or {}, "reply": target.get("reply", "")}
    return json.dumps(t, ensure_ascii=False, separators=(",", ":"))
