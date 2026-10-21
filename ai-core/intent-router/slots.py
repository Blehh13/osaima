#!/usr/bin/env python3
"""
Parameter extraction for the fast path: turns (intent, utterance) into the
params dict the shell's dispatchIntent() expects (frontend/src/wm/apps.js).

Returns None when a required parameter cannot be found, so the caller hands the
utterance to the LLM instead of guessing.
"""
import ast
import operator
import re

# Shell app ids (appRegistry) and the words people use for them.
APPS = {
    "terminal": ["terminal", "konsole", "console", "shell", "command line", "cli", "foot"],
    "browser": ["browser", "firefox", "chrome", "chromium", "web browser", "internet", "web"],
    "files": ["files", "file manager", "file explorer", "explorer", "nautilus", "dolphin", "folders"],
    "monitor": ["monitor", "system monitor", "task manager", "taskmanager", "htop", "activity monitor",
                "resource monitor"],
    "config": ["settings", "config", "configuration", "preferences", "control panel", "wm config"],
    "about": ["about", "system info", "about this computer"],
    "assistant": ["assistant", "ai", "chat", "osaima"],
    "calendar": ["calendar", "schedule"],
    "notes": ["notes", "notepad", "note app", "notes app"],
    "editor": ["text editor", "mousepad", "editor"],
    "office": ["libreoffice", "office", "word processor", "writer", "spreadsheet", "presentation", "impress", "documents app"],
    "player": ["mpv", "video player", "media player", "music player", "player"],
    "archives": ["xarchiver", "archive manager", "zip tool", "archives"],
}

LAYOUTS = {
    "tile": ["tile", "tiling", "tiled", "side by side"],
    "monocle": ["monocle", "fullscreen", "full screen", "maximize", "maximise", "one window"],
    "grid": ["grid"],
    "spiral": ["spiral", "fibonacci"],
    "float": ["float", "floating", "free"],
}

NUMBER_WORDS = {
    "zero": 0, "one": 1, "a": 1, "an": 1, "two": 2, "three": 3, "four": 4, "five": 5, "six": 6,
    "seven": 7, "eight": 8, "nine": 9, "ten": 10, "eleven": 11, "twelve": 12, "fifteen": 15,
    "twenty": 20, "thirty": 30, "forty": 40, "forty-five": 45, "fifty": 50, "sixty": 60, "ninety": 90,
    "first": 1, "second": 2, "third": 3, "fourth": 4, "fifth": 5, "half": 0.5, "couple": 2,
}
UNIT_SECONDS = {"s": 1, "sec": 1, "secs": 1, "second": 1, "seconds": 1,
                "m": 60, "min": 60, "mins": 60, "minute": 60, "minutes": 60,
                "h": 3600, "hr": 3600, "hrs": 3600, "hour": 3600, "hours": 3600}

FILLER = r"(?:please|now|for me|thanks|thank you|asap|right now|quickly|real quick)"


def _clean(text):
    return re.sub(r"\s+", " ", text.lower().strip(" .!?")).strip()


def _strip_filler(text):
    return re.sub(rf"(?:\s+{FILLER})+$", "", re.sub(rf"^(?:{FILLER}\s+)+", "", text)).strip(" ,.!?")


def _find_alias(text, table):
    """Longest alias wins, matched on word boundaries."""
    best = None
    for key, aliases in table.items():
        for a in aliases:
            if re.search(rf"\b{re.escape(a)}\b", text) and (best is None or len(a) > best[1]):
                best = (key, len(a))
    return best[0] if best else None


def _number(token):
    token = token.strip().lower()
    try:
        return float(token)
    except ValueError:
        return NUMBER_WORDS.get(token)


def parse_duration(text):
    """'5 minutes', 'an hour and 30 seconds', '90s', 'half an hour', 'pomodoro' -> seconds."""
    t = _clean(text)
    if "pomodoro" in t:
        return 1500
    if re.search(r"\bhalf an? hour\b", t):
        return 1800
    total = 0
    for num, unit in re.findall(r"(\d+(?:\.\d+)?|[a-z-]+)\s*(seconds?|secs?|minutes?|mins?|hours?|hrs?|[smh])\b", t):
        n = _number(num)
        if n is not None:
            total += n * UNIT_SECONDS[unit]
    return int(total) or None


# ---------------- calculator ----------------
_OPS = {ast.Add: operator.add, ast.Sub: operator.sub, ast.Mult: operator.mul, ast.Div: operator.truediv,
        ast.Pow: operator.pow, ast.Mod: operator.mod, ast.USub: operator.neg, ast.UAdd: operator.pos}


def _safe_eval(node):
    if isinstance(node, ast.Expression):
        return _safe_eval(node.body)
    if isinstance(node, ast.Constant) and isinstance(node.value, (int, float)):
        return node.value
    if isinstance(node, ast.BinOp) and type(node.op) in _OPS:
        left, right = _safe_eval(node.left), _safe_eval(node.right)
        if isinstance(node.op, ast.Pow) and abs(right) > 100:
            raise ValueError("exponent too large")
        return _OPS[type(node.op)](left, right)
    if isinstance(node, ast.UnaryOp) and type(node.op) in _OPS:
        return _OPS[type(node.op)](_safe_eval(node.operand))
    if isinstance(node, ast.Call) and getattr(node.func, "id", None) in ("sqrt", "factorial") and len(node.args) == 1:
        import math
        arg = _safe_eval(node.args[0])
        if node.func.id == "factorial" and (arg != int(arg) or arg > 50):
            raise ValueError("factorial out of range")
        return math.sqrt(arg) if node.func.id == "sqrt" else math.factorial(int(arg))
    raise ValueError("unsupported expression")


def calculate(text):
    t = _clean(text)
    t = re.sub(r"^(?:what(?:'s| is)|calculate|compute|how much is|solve|work out|math:?|quickly calculate)\s+", "", t)
    t = re.sub(r"(\d+(?:\.\d+)?)\s*(?:%|percent)\s*(?:tip\s+)?(?:of|on)\s*(\d+(?:\.\d+)?)", r"(\1/100*\2)", t)
    t = re.sub(r"(?:the\s+)?square root of\s*(\d+(?:\.\d+)?)", r"sqrt(\1)", t)
    t = re.sub(r"(\d+)\s*factorial|(\d+)!", lambda m: f"factorial({m.group(1) or m.group(2)})", t)
    for words, sym in [(r"to the power(?: of)?|raised to", "**"), (r"multiplied by|times|into|x", "*"),
                       (r"divided by|over", "/"), (r"plus|added to|and", "+"), (r"minus|less", "-"),
                       (r"mod(?:ulo)?", "%")]:
        t = re.sub(rf"\b(?:{words})\b", f" {sym} ", t)
    t = t.replace("^", "**").replace("×", "*").replace("÷", "/")
    if not re.fullmatch(r"[\d\s.+\-*/%()a-z_]+", t) or not re.search(r"\d", t):
        return None
    try:
        value = _safe_eval(ast.parse(t.strip(), mode="eval"))
    except (SyntaxError, ValueError, ZeroDivisionError, TypeError, OverflowError):
        return None
    if isinstance(value, float) and value.is_integer():
        value = int(value)
    elif isinstance(value, float):
        value = round(value, 6)
    return {"expression": re.sub(r"\s+", " ", t).strip(), "result": value}


# ---------------- per-intent extractors ----------------
def _after(text, patterns):
    for p in patterns:
        m = re.search(p, text)
        if m and m.group(1).strip():
            return _strip_filler(m.group(1))
    return None


def _note_title(text):
    t = _clean(text)
    title = _after(t, [r"note (?:called|titled|named)\s+['\"]?(.+?)['\"]?$",
                       r"(?:the|my)\s+(.+?)\s+note\b", r"\bnote\s+['\"]?(.+?)['\"]?$",
                       r"(?:open|view|read|show|delete|remove|erase|trash)\s+(?:the\s+)?(.+)$"])
    return title


def extract(intent, utterance):
    t = _clean(utterance)
    if intent in ("open_app", "focus_app"):
        app = _find_alias(t, APPS)
        return {"app": app} if app else None
    if intent == "close_window":
        app = _find_alias(t, APPS)
        return {"app": app} if app else {}  # no app: the shell closes the focused window
    if intent == "layout":
        layout = _find_alias(t, LAYOUTS)
        return {"layout": layout} if layout else None
    if intent == "workspace":
        m = re.search(r"(?:workspace|desktop|space)\s+(?:number\s+)?(\d+|[a-z]+)", t) or \
            re.search(r"\b(\d+|first|second|third|fourth|fifth)\b(?:\s+(?:workspace|desktop))", t)
        n = _number(m.group(1)) if m else None
        return {"number": int(n)} if n and n >= 1 else None
    if intent == "set_gaps":
        if re.search(r"\b(?:no|remove|zero|without)\b.*\bgaps?\b|\bgaps?\b.*\b(?:off|none)\b|touch each other", t):
            return {"pixels": 0}
        m = re.search(r"(\d+)\s*(?:px|pixels?)?", t)
        return {"pixels": int(m.group(1))} if m else None
    if intent == "timer":
        seconds = parse_duration(t)
        return {"seconds": seconds} if seconds else None
    if intent == "reminder":
        text = _after(t, [r"remind(?:er)?\s+me\s+(?:to|about)\s+(.+)", r"(?:reminder|remember)\s*(?:to|for|:)\s*(.+)",
                          r"forget to\s+(.+)", r"remind me\s+(.+)"])
        return {"text": text} if text else None
    if intent == "calculator":
        return calculate(t)
    if intent == "search_web":
        q = _after(t, [r"(?:search|look)\s+(?:the web|online|the internet)?\s*(?:for|up)\s+(.+?)(?:\s+(?:online|on the (?:web|internet)))?$",
                       r"google\s+(.+)", r"look\s+up\s+(.+?)(?:\s+online)?$", r"find\s+(.+?)\s+on the (?:internet|web)",
                       r"web search\s+(.+)", r"browse for\s+(.+)", r"search\s+(.+)"])
        return {"query": q} if q else None
    if intent in ("view_note", "delete_note"):
        title = _note_title(t)
        return {"title": title} if title else None
    if intent == "search_notes":
        q = _after(t, [r"notes?\s+(?:for|about|mentioning|on)\s+(.+)", r"(?:mentions|about)\s+(.+)", r"search notes\s+(.+)"])
        return {"query": q} if q else None
    if intent in OS_ACTIONS:
        return _os_action(intent, t)
    # Intents the shell runs without parameters.
    return {}


# Router intents executed by the daemon's system.os_action (D-Bus etc.).
OS_ACTIONS = {"volume_control": "volume", "media_playback": "media", "brightness_control": "brightness",
              "lock_screen": "lock_screen", "wifi_control": "wifi", "bluetooth_control": "bluetooth",
              "power_control": "power"}


def _percent(t):
    m = re.search(r"(\d{1,3})\s*(?:%|percent|per cent)?", t)
    return float(m.group(1)) if m else None


def _first(t, table):
    """First op whose pattern matches, in table order (more specific first)."""
    for op, pattern in table:
        if re.search(pattern, t):
            return op
    return None


def _os_action(intent, t):
    action = OS_ACTIONS[intent]
    if action == "lock_screen":
        return {"action": action, "op": "lock"}
    if action == "volume":
        op = _first(t, [("unmute", r"\bunmute\b"), ("mute", r"\bmute\b|\bsilence\b"),
                        ("set", r"\b(?:max|full)\b"), ("set", r"\d"),
                        ("down", r"\b(?:down|lower|quieter|decrease|reduce|too loud|softer)\b"),
                        ("up", r"\b(?:up|raise|louder|increase|crank|can't hear|cannot hear)\b")])
        value = 100.0 if re.search(r"\b(?:max|full)\b", t) else _percent(t)
        if op is None or (op == "set" and value is None):
            return None
        return {"action": action, "op": op, **({"value": value} if value is not None else {})}
    if action == "brightness":
        op = _first(t, [("set", r"\b(?:max|full)\b"), ("set", r"\d"),
                        ("down", r"\b(?:down|lower|dim|dimmer|decrease|reduce|too bright)\b"),
                        ("up", r"\b(?:up|raise|brighter|brighten|increase|too dark)\b")])
        value = 100.0 if re.search(r"\b(?:max|full)\b", t) else _percent(t)
        if op is None or (op == "set" and value is None):
            return None
        return {"action": action, "op": op, **({"value": value} if value is not None else {})}
    if action == "media":
        op = _first(t, [("next", r"\b(?:next|skip)\b"), ("previous", r"\b(?:previous|back a|go back|last song|prev)\b"),
                        ("pause", r"\bpause\b"), ("stop", r"\bstop\b"), ("play", r"\b(?:play|resume|unpause)\b")])
        return {"action": action, "op": op or "play_pause"}
    if action in ("wifi", "bluetooth"):
        op = _first(t, [("status", r"^(?:is|are|am)\b|\b(?:which|status|am i|is it|show|list|available)\b"),
                        ("off", r"\b(?:off|disable|disconnect|kill|drop|stop)\b"),
                        ("on", r"\b(?:on|enable|start|turn on|switch on)\b")])
        return {"action": action, "op": op} if op else None  # "pair my mouse" etc. needs the LLM
    if action == "power":
        op = _first(t, [("reboot", r"\b(?:reboot|restart)\b"), ("suspend", r"\b(?:suspend|sleep|hibernate)\b"),
                        ("shutdown", r"\b(?:shut ?down|power (?:off|down)|turn off the (?:pc|computer|laptop)|exit the os)\b")])
        return {"action": action, "op": op} if op else None
    return None


REPLIES = {
    "open_app": "Opening {app}.", "focus_app": "Switching to {app}.", "close_window": "Closing it.",
    "layout": "Switched to {layout} layout.", "workspace": "Going to workspace {number}.",
    "set_gaps": "Set gaps to {pixels}px.", "timer": "Timer set.", "stop_timer": "Timer stopped.",
    "reminder": "I'll remind you to {text}.", "calculator": "{expression} = {result}",
    "search_web": "Searching the web for \"{query}\".", "view_note": "Here's your \"{title}\" note.",
    "delete_note": "Deleted the \"{title}\" note.", "search_notes": "Searching your notes for \"{query}\".",
    "list_notes": "Here are your notes.", "close_all_windows": "Closing all windows.",
    "minimize_all": "Showing the desktop.", "system_status": "Here's how your system is doing.",
    "calendar": "Here is the calendar.", "weather": "Here's the weather.", "scratchpad": "Here's a scratchpad.",
    "optimize_cpu": "Freezing heavy background apps.", "wake_apps": "Waking your apps back up.",
    "clean_storage": "Cleaning up storage.", "fix_network": "Resetting the network.",
    "greeting": "Hello! What would you like me to do?",
    "help": "I can open apps, manage windows and workspaces, set timers and reminders, do quick maths, "
            "search the web, manage notes, check your system, and run or plan tasks for you.",
    "shutdown": "Shutting down the system...",
    "os_action": "Done.",
}


def reply_for(intent, params):
    try:
        return REPLIES.get(intent, "").format(**params)
    except (KeyError, IndexError):
        return REPLIES.get(intent, "")


if __name__ == "__main__":
    checks = [
        ("open_app", "fire up the terminal please", {"app": "terminal"}),
        ("open_app", "launch firefox", {"app": "browser"}),
        ("open_app", "open VS Code", None),
        ("timer", "set a timer for 5 minutes", {"seconds": 300}),
        ("timer", "countdown an hour and 30 seconds", {"seconds": 3630}),
        ("timer", "half an hour timer", {"seconds": 1800}),
        ("layout", "switch to grid layout", {"layout": "grid"}),
        ("layout", "make this window fullscreen", {"layout": "monocle"}),
        ("workspace", "go to workspace three", {"number": 3}),
        ("set_gaps", "no gaps please", {"pixels": 0}),
        ("set_gaps", "set window gaps to 12 pixels", {"pixels": 12}),
        ("calculator", "what is 25% of 1450", {"expression": "(25/100*1450)", "result": 362.5}),
        ("calculator", "12 times 37", {"expression": "12 * 37", "result": 444}),
        ("calculator", "the square root of 144", {"expression": "sqrt(144)", "result": 12}),
        ("calculator", "what is the meaning of life", None),
        ("search_web", "search the web for rust tutorials", {"query": "rust tutorials"}),
        ("search_web", "google cheap flights to delhi please", {"query": "cheap flights to delhi"}),
        ("reminder", "remind me to call mom at 5 pm", {"text": "call mom at 5 pm"}),
        ("view_note", "open the shopping list note", {"title": "shopping list"}),
        ("delete_note", "delete the note called ideas", {"title": "ideas"}),
        ("close_window", "close this window", {}),
        ("volume_control", "turn the volume up", {"action": "volume", "op": "up"}),
        ("volume_control", "set volume to 40 percent", {"action": "volume", "op": "set", "value": 40.0}),
        ("volume_control", "unmute", {"action": "volume", "op": "unmute"}),
        ("volume_control", "it's too loud", {"action": "volume", "op": "down"}),
        ("brightness_control", "make the screen dimmer", {"action": "brightness", "op": "down"}),
        ("brightness_control", "full brightness", {"action": "brightness", "op": "set", "value": 100.0}),
        ("media_playback", "skip this song", {"action": "media", "op": "next"}),
        ("media_playback", "pause the music", {"action": "media", "op": "pause"}),
        ("wifi_control", "turn off wifi", {"action": "wifi", "op": "off"}),
        ("wifi_control", "which wifi am i on", {"action": "wifi", "op": "status"}),
        ("wifi_control", "is wifi on", {"action": "wifi", "op": "status"}),
        ("bluetooth_control", "is bluetooth enabled?", {"action": "bluetooth", "op": "status"}),
        ("bluetooth_control", "pair my mouse", None),
        ("power_control", "restart the computer", {"action": "power", "op": "reboot"}),
        ("lock_screen", "lock my laptop", {"action": "lock_screen", "op": "lock"}),
    ]
    bad = 0
    for intent, text, want in checks:
        got = extract(intent, text)
        if got != want:
            bad += 1
            print(f"FAIL {intent}: {text!r} -> {got} (want {want})")
    print(f"{len(checks) - bad}/{len(checks)} slot checks passed")
