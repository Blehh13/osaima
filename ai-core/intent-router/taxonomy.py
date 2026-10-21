#!/usr/bin/env python3
"""
OSAIMA unified intent taxonomy: the single source of truth for the intent router.

It merges the two lists that had drifted apart:
  * the PDR's 3-track / 40-intent set the BERT router was trained on, and
  * the intents the desktop shell actually executes (frontend/src/wm/apps.js).

Names follow the shell where it has a handler (open_app, timer, create_note...)
and the PDR otherwise (wifi_control, git_workflow...). OLD_TO_NEW maps every
old PDR label onto this list, so earlier datasets such as testset.jsonl can be
relabelled and reused unchanged.

Tracks (PDR §3.3):
  SYSTEM_COMMAND       fast path: handled without the LLM
  WORKFLOW_GENERATION  slow path: the LLM plans commands / tool calls
  CONTENT_GENERATION   slow path: the LLM writes text

TRAIN_TEMPLATES are for training only. They are written independently of the
held-out test sets; never copy a test phrasing in here.
"""

SYSTEM, WORKFLOW, CONTENT = "SYSTEM_COMMAND", "WORKFLOW_GENERATION", "CONTENT_GENERATION"

# Slot values used to fill templates.
SLOTS = {
    "app": ["Firefox", "the terminal", "VS Code", "the Files app", "the file manager", "Spotify",
            "the calculator", "Slack", "the text editor", "the browser", "Chrome", "the settings",
            "the task manager", "the system monitor", "notes", "the calendar", "Discord", "GIMP",
            "LibreOffice", "Thunderbird", "the music player", "Zoom", "Telegram", "Steam"],
    "dir": ["my projects folder", "Documents", "Downloads", "the home directory", "this folder",
            "the Desktop", "~/code", "Pictures", "my src directory", "the current directory"],
    "lang": ["Python", "JavaScript", "Rust", "Go", "Java", "TypeScript", "C++", "Kotlin"],
    "who": ["my team", "the professor", "my manager", "the client", "Akash", "Pragna", "Banu",
            "HR", "my guide", "the hod", "my friend", "the vendor"],
    "svc": ["docker", "nginx", "postgresql", "the ssh service", "cups", "apache", "redis", "mysql",
            "the printer service", "networkmanager"],
    "n": ["1", "2", "3", "4", "5", "6", "two", "three", "four", "five", "one", "seven", "nine"],
    "dur": ["5 minutes", "10 minutes", "30 seconds", "one minute", "an hour", "25 minutes",
            "90 seconds", "2 hours", "fifteen minutes", "45 mins", "3 min", "20 secs"],
    "time": ["5 pm", "tomorrow morning", "9 am", "tonight", "monday", "in an hour", "noon",
             "next friday", "at 6", "half past four"],
    "task": ["call mom", "submit the report", "take my medicine", "check the build", "pay the electricity bill",
             "drink water", "stand up and stretch", "email the professor", "buy groceries", "water the plants"],
    "event": ["team meeting", "dentist appointment", "project review", "viva", "lunch with Akash",
              "standup", "lab session", "sprint planning", "guide meeting", "demo rehearsal"],
    "math": ["25% of 1450", "12 times 37", "the square root of 144", "3456 divided by 12", "15 plus 27",
             "2 to the power 10", "18 percent tip on 640", "1000 minus 387", "7 factorial", "45 * 3.5"],
    "city": ["Bangalore", "Mumbai", "Delhi", "London", "Chennai", "Mysore", "New York", "Tokyo", "here"],
    "query": ["rust tutorials", "how to install arch linux", "the best laptops for students", "gentoo handbook",
              "llama.cpp tool calling", "weather tomorrow", "python list comprehension", "tauri docs",
              "news about ai", "cheap flights to delhi"],
    "note": ["shopping list", "meeting notes", "ideas", "todo", "viva prep", "recipes", "project plan",
             "lecture notes", "passwords hint", "book list"],
    "layout": ["tile", "monocle", "grid", "spiral", "float", "floating", "tiling"],
    "px": ["0", "4", "8", "10", "12", "16", "20"],
    "pct": ["10", "20", "30", "40", "50", "60", "70", "80", "90", "100"],
    "file": ["the readme", "report.pdf", "my resume", "the config file", "main.rs", "notes.txt",
             "the budget spreadsheet", "the thesis draft", "todo.md", "the slides"],
    "pkg": ["htop", "neofetch", "git", "nodejs", "docker", "vlc", "ffmpeg", "python3-pip", "curl", "tmux"],
    "shellq": ["list the biggest files in my home", "show which process is on port 8080",
               "count the lines in all python files", "compress the logs folder", "check my ip address",
               "print the kernel version", "show the last 20 lines of the syslog", "find duplicate photos",
               "show open network connections", "check if port 22 is open"],
    "topic": ["recursion", "how a hash map works", "what a mutex is", "this error", "big o notation",
              "how dns works", "what ebpf is", "the difference between tcp and udp", "transformers",
              "what this command does"],
    "tlang": ["French", "Hindi", "Kannada", "Spanish", "German", "Tamil", "Japanese"],
}

# intent -> (track, training templates)
INTENTS = {
    # ---------------- windows & apps ----------------
    "open_app": (SYSTEM, [
        "open {app}", "launch {app}", "start {app}", "run {app}", "bring up {app}", "show me {app}",
        "can you open {app}", "i want to use {app}", "{app} please", "open up {app} for me",
        "load {app}", "i need to use {app} now", "start up {app}", "go to {app}"]),
    "close_window": (SYSTEM, [
        "close {app}", "quit {app}", "shut {app}", "close this window", "close the current window",
        "close that", "i'm done with {app}, close it", "terminate {app} window", "exit out of {app}",
        "close the {app} window", "quit this app", "x out of {app}"]),
    "close_all_windows": (SYSTEM, [
        "close all windows", "close everything", "shut all the apps", "clear the desktop",
        "close every open window", "quit all applications", "close all of them", "kill all windows",
        "get rid of all the windows", "start fresh and close everything", "empty the screen"]),
    "minimize_all": (SYSTEM, [
        "minimize all windows", "show the desktop", "hide everything", "minimise everything",
        "hide all the windows", "take me to the desktop", "minimize all", "show desktop please",
        "put all windows away", "i want to see my wallpaper"]),
    "focus_app": (SYSTEM, [
        "switch to {app}", "focus {app}", "go back to {app}", "bring {app} to the front",
        "show the {app} window", "focus on the next window", "switch to the previous window",
        "alt tab to {app}", "make {app} the active window", "jump back to {app}"]),
    "layout": (SYSTEM, [
        "switch to {layout} layout", "use the {layout} layout", "change layout to {layout}",
        "set the window layout to {layout}", "tile the windows", "make this window fullscreen",
        "arrange windows in a grid", "i want {layout} mode", "change the tiling to {layout}",
        "go {layout}", "lay out the windows as {layout}", "put windows in {layout} arrangement"]),
    "workspace": (SYSTEM, [
        "switch to workspace {n}", "go to workspace {n}", "move this window to workspace {n}",
        "desktop {n}", "open workspace {n}", "send this to the {n} desktop", "change to virtual desktop {n}",
        "workspace {n} please", "move me to the next workspace", "put this window on desktop {n}"]),
    "set_gaps": (SYSTEM, [
        "set gaps to {px}", "set window gaps to {px} pixels", "make the gaps {px}px", "remove the gaps",
        "increase the gap between windows", "no gaps please", "gap size {px}", "change the window spacing to {px}",
        "make windows touch each other", "bigger gaps"]),

    # ---------------- system state & maintenance ----------------
    "system_status": (SYSTEM, [
        "how much cpu am i using", "show memory usage", "what is my ram usage", "system status",
        "how is my system doing", "check disk usage", "show system vitals", "is my computer overloaded",
        "what's using all my memory", "show me cpu and ram", "how much storage is left",
        "system health check", "uptime", "show the vitals", "how hot is my cpu"]),
    "battery_status": (SYSTEM, [
        "battery percentage", "how much battery do i have", "is it charging", "battery status",
        "check battery", "how long will my battery last", "what's the charge level",
        "am i plugged in", "battery health", "how much charge is left"]),
    "process_list": (SYSTEM, [
        "show running processes", "list processes", "what programs are running", "show the process list",
        "which apps are running right now", "list all running tasks", "what is running in the background",
        "top processes", "show me running apps", "ps aux"]),
    "process_kill": (SYSTEM, [
        "kill the process using the most cpu", "end the frozen app", "terminate that process",
        "force quit {app}", "kill process {n}", "stop the program that's hanging", "end task {app}",
        "kill the stuck process", "force close the unresponsive app", "kill whatever is eating memory"]),
    "optimize_cpu": (SYSTEM, [
        "optimize cpu", "my computer is lagging", "speed up my system", "freeze heavy apps",
        "free up some cpu", "the system is slow, fix it", "clean memory", "reduce lag",
        "put background apps to sleep", "make the computer faster", "optimise performance",
        "optimze cpu", "everything is so slow"]),
    "wake_apps": (SYSTEM, [
        "wake up the apps", "resume the frozen apps", "unfreeze my apps", "bring back the sleeping apps",
        "restore the paused programs", "wake apps", "unpause everything", "resume background apps"]),
    "clean_storage": (SYSTEM, [
        "clean storage", "empty the trash", "clear the cache", "free up disk space", "delete temporary files",
        "my disk is full, clean it", "clear caches and trash", "clean up junk files", "make some space on the disk",
        "remove temp files"]),
    "fix_network": (SYSTEM, [
        "fix my internet", "the network is slow", "reset networking", "my wifi isn't working",
        "fix network", "internet is down, help", "restart the network", "reconnect the internet",
        "flush dns", "why is my internet so sluggish"]),

    # ---------------- device & OS controls ----------------
    "volume_control": (SYSTEM, [
        "mute", "unmute", "turn the volume up", "volume down", "set volume to {pct} percent",
        "make it louder", "make it quieter", "mute the sound", "volume {pct}", "lower the volume",
        "max volume", "too loud"]),
    "media_playback": (SYSTEM, [
        "pause the music", "play music", "skip this song", "next track", "previous song", "resume the song",
        "stop playing", "play", "pause", "go back a track", "shuffle my playlist", "what song is this"]),
    "brightness_control": (SYSTEM, [
        "brightness up", "turn down the brightness", "set brightness to {pct} percent", "screen is too bright",
        "make the screen dimmer", "increase screen brightness", "brightness {pct}", "it's too dark, brighten the screen",
        "lower brightness", "full brightness"]),
    "screenshot": (SYSTEM, [
        "take a screenshot", "screenshot", "capture the screen", "screenshot this window", "save a screen capture",
        "print screen", "take a screen grab", "capture what's on my screen", "screenshot the selected area"]),
    "lock_screen": (SYSTEM, [
        "lock the screen", "lock my computer", "lock", "lock the laptop", "i'm stepping away, lock it",
        "lock the session", "activate the lock screen", "lock now"]),
    "power_control": (SYSTEM, [
        "shut down", "shutdown the computer", "power off", "restart the system", "reboot", "suspend",
        "hibernate the laptop", "turn off the pc", "log out", "sleep the computer", "restart now",
        "power down please", "exit the os"]),
    "wifi_control": (SYSTEM, [
        "turn on wifi", "turn off wifi", "disconnect wifi", "connect to the home network", "enable wireless",
        "which wifi am i on", "connect to wifi", "wifi off", "switch to the hotspot", "show available networks"]),
    "bluetooth_control": (SYSTEM, [
        "turn on bluetooth", "bluetooth off", "connect my headphones", "pair my mouse", "disable bluetooth",
        "connect to my speaker", "enable bluetooth", "disconnect the earphones", "show bluetooth devices",
        "unpair the keyboard"]),
    "vpn_control": (SYSTEM, [
        "connect to the vpn", "turn off the vpn", "vpn on", "disconnect vpn", "enable the vpn",
        "am i on the vpn", "start the work vpn", "toggle vpn"]),
    "service_control": (SYSTEM, [
        "restart {svc}", "stop {svc}", "start {svc}", "check the status of {svc}", "is {svc} running",
        "enable {svc} on boot", "reload {svc}", "disable {svc}", "status of {svc} service", "kick {svc}"]),
    "mount_control": (SYSTEM, [
        "mount the usb drive", "unmount the external disk", "eject the sd card", "mount my pendrive",
        "safely eject the usb", "open the usb stick", "unmount the drive", "mount the backup disk"]),
    "notification_ctrl": (SYSTEM, [
        "turn on do not disturb", "clear all notifications", "mute notifications", "dnd on",
        "stop the notifications", "dismiss the notifications", "show my notifications", "focus mode on",
        "turn off notification sounds"]),
    "clipboard_control": (SYSTEM, [
        "copy this", "paste", "show clipboard history", "clear the clipboard", "copy that text",
        "what did i copy", "paste what i copied earlier", "empty the clipboard"]),

    # ---------------- files ----------------
    "file_search": (SYSTEM, [
        "find all python files in {dir}", "search for {file}", "locate {file}", "find files named report",
        "where did i save {file}", "search {dir} for pdfs", "find files modified today", "look for {file} in {dir}",
        "which folder has {file}", "find large files in {dir}", "search my files for invoice"]),
    "file_open": (SYSTEM, [
        "open {file}", "show me {file}", "open the file {file}", "open my last document", "view {file}",
        "open {dir}", "show {dir}", "open the folder {dir}", "display {file}", "open recent files"]),

    # ---------------- productivity widgets ----------------
    "timer": (SYSTEM, [
        "set a timer for {dur}", "timer {dur}", "start a {dur} timer", "countdown {dur}",
        "remind me in {dur}", "wake me in {dur}", "set an alarm for {dur} from now", "{dur} timer please",
        "time {dur} for me", "start a pomodoro"]),
    "stop_timer": (SYSTEM, [
        "stop the timer", "cancel the timer", "clear the countdown", "turn off the timer", "stop the alarm",
        "kill the timer", "reset the timer", "no more timer", "end the countdown"]),
    "reminder": (SYSTEM, [
        "remind me to {task}", "remind me to {task} at {time}", "set a reminder to {task}",
        "don't let me forget to {task}", "reminder: {task} {time}", "can you remind me {time} to {task}",
        "i need to {task} {time}, remind me", "add a reminder for {task}"]),
    "calendar": (SYSTEM, [
        "show my calendar", "open the calendar", "what's on my schedule", "what day is it",
        "what's the date today", "do i have anything {time}", "show this week", "what's my agenda",
        "when is the next holiday", "calendar"]),
    "schedule_event": (SYSTEM, [
        "schedule a {event} at {time}", "add {event} to my calendar {time}", "book a {event} for {time}",
        "create an event {event} on {time}", "put {event} in the calendar", "set up a {event} {time}",
        "plan a {event} for {time}", "new calendar event {event} {time}"]),
    "calculator": (SYSTEM, [
        "what is {math}", "calculate {math}", "compute {math}", "how much is {math}", "{math}",
        "solve {math}", "work out {math}", "math: {math}", "quickly calculate {math}"]),
    "weather": (SYSTEM, [
        "what's the weather", "weather in {city}", "will it rain {time}", "how hot is it outside",
        "temperature in {city}", "do i need an umbrella", "forecast for {city}", "is it cold today",
        "weather today", "weather forecast"]),
    "search_web": (SYSTEM, [
        "search for {query}", "google {query}", "look up {query} online", "search the web for {query}",
        "find {query} on the internet", "browse for {query}", "web search {query}", "look online for {query}",
        "open google and search {query}"]),
    "scratchpad": (SYSTEM, [
        "open a scratchpad", "give me a quick notepad", "i need to jot something down", "open scratchpad",
        "quick note pad please", "something to write on", "a blank pad to type in"]),

    # ---------------- notes ----------------
    "list_notes": (SYSTEM, [
        "show my notes", "list all notes", "what notes do i have", "open my notes", "show all my notes",
        "how many notes do i have", "list notes", "my notes please"]),
    "view_note": (SYSTEM, [
        "open the {note} note", "show the note called {note}", "read my {note} note", "view {note}",
        "what's in my {note} note", "display the {note} note", "open note {note}"]),
    "delete_note": (SYSTEM, [
        "delete the {note} note", "remove the note {note}", "trash my {note} note", "get rid of the {note} note",
        "delete note {note}", "erase the {note} note"]),
    "search_notes": (SYSTEM, [
        "search my notes for {query}", "find notes about {note}", "which note mentions {query}",
        "look through my notes for {note}", "search notes {note}", "do i have a note about {note}"]),

    # ---------------- conversation ----------------
    "greeting": (SYSTEM, [
        "hi", "hello", "hey osaima", "good morning", "good evening", "hey there", "yo", "hello there",
        "namaste", "hi osaima, how are you", "thanks", "thank you"]),
    "help": (SYSTEM, [
        "what can you do", "help", "how do i use this", "show me what you can do", "list your commands",
        "what are your features", "i'm lost, help me", "what kind of things can i ask you"]),

    # ---------------- workflow generation (LLM plans) ----------------
    "execute_command": (WORKFLOW, [
        "{shellq}", "run a command to {shellq}", "use the terminal to {shellq}", "install {pkg}",
        "can you {shellq}", "update the system", "upgrade all packages", "download {query} with wget",
        "run {pkg} for me", "execute {shellq}", "install {pkg} on my system", "remove {pkg}"]),
    "orchestrate": (WORKFLOW, [
        "close everything and open {app}", "open {app} and search for {query}", "close {app} then open {app}",
        "minimize all and then open the calendar", "open the terminal and run htop",
        "switch to workspace {n} and launch {app}", "clean storage and then check system status",
        "set a timer for {dur} and open {app}", "close all windows, then lock the screen",
        "open {app}, then open notes and start a timer"]),
    "project_scaffold": (WORKFLOW, [
        "set up a {lang} project", "scaffold a new {lang} app", "create a django project called myapp",
        "start a new react project with my defaults", "make a boilerplate {lang} project in {dir}",
        "initialize a cargo project", "new {lang} project with tests and a readme", "bootstrap a fastapi service"]),
    "git_workflow": (WORKFLOW, [
        "commit everything and push", "create a branch called feature", "push my changes to github",
        "pull the latest changes", "stage all files and commit", "merge main into my branch",
        "git push", "rebase on master", "undo my last commit", "show the git log and push"]),
    "env_setup": (WORKFLOW, [
        "set up a virtual environment", "create a venv and activate it", "make a conda env with numpy",
        "install the requirements into a new venv", "configure a python environment for this project",
        "set up node with nvm", "prepare the dev environment"]),
    "package_install": (WORKFLOW, [
        "install the dependencies", "install the project requirements", "add fastapi to the project",
        "npm install", "pip install the requirements", "add {pkg} as a dependency", "install pytorch with cuda",
        "get all the packages this project needs"]),
    "run_tests": (WORKFLOW, [
        "run the tests", "run pytest", "run the test suite", "test this project", "run cargo test",
        "check if the tests pass", "run unit tests with coverage", "execute the tests and show failures"]),
    "deploy_workflow": (WORKFLOW, [
        "deploy this", "build the docker image and run it", "deploy to the staging server", "start the dev server",
        "push this to production", "run the app on port 8000", "ship it to the server", "launch the web server"]),
    "file_batch_op": (WORKFLOW, [
        "rename all images in {dir}", "convert every png in {dir} to jpg", "organize {dir} by file type",
        "move all pdfs from {dir} to Documents", "delete every log file in {dir}", "resize all photos in {dir}",
        "zip everything in {dir}", "sort {dir} into folders"]),
    "build_compile": (WORKFLOW, [
        "compile the project", "build the {lang} code", "build the release binary", "run make",
        "cargo build --release", "compile and watch for changes", "build it", "rebuild everything"]),

    # ---------------- content generation (LLM writes) ----------------
    "create_note": (CONTENT, [
        "create a note called {note}", "make a new note", "write a note with a pasta recipe",
        "new note: {task}", "take a note that i need to {task}", "write down {task} in a note",
        "create a note titled {note} about {topic}", "note this down: {task}", "start a {note} note",
        "jot down a recipe for biryani", "make a note of the meeting points"]),
    "edit_note": (CONTENT, [
        "edit the {note} note", "add {task} to my {note} note", "update my {note} note",
        "change the {note} note to say {task}", "append {task} to {note}", "rewrite my {note} note",
        "expand my {note} note with more detail"]),
    "email_draft": (CONTENT, [
        "draft an email to {who}", "write an email to {who} about the deadline", "reply to {who}",
        "compose a mail to {who} asking for leave", "email {who} about the project status",
        "write a formal email to {who}", "send {who} a mail saying i'll be late", "draft a reply declining the meeting"]),
    "commit_message": (CONTENT, [
        "write a commit message", "generate a commit message for these changes", "summarize my diff as a commit",
        "what should my commit message be", "commit message in my usual style", "describe these changes for git"]),
    "code_generate": (CONTENT, [
        "write a {lang} function to reverse a string", "generate a {lang} script that renames files",
        "write code for a linked list in {lang}", "create a {lang} class for a bank account",
        "give me {lang} code to read a csv", "write a function that checks for primes", "generate a bash script to back up {dir}",
        "code a simple web server in {lang}"]),
    "code_refactor": (CONTENT, [
        "refactor this function", "clean up this code", "make this code more readable", "optimize this function",
        "split this into smaller functions", "improve this code", "simplify this logic", "refactor the {lang} module"]),
    "docstring_gen": (CONTENT, [
        "write a docstring for this function", "add type hints", "document this class", "add comments to this code",
        "generate documentation for this module", "write a jsdoc for this", "explain this function in a docstring"]),
    "summarize_text": (CONTENT, [
        "summarize this document", "give me a summary of {file}", "tldr", "summarize this page",
        "what are the key points of {file}", "shorten this text", "make a brief summary of the readme",
        "summarize the meeting notes"]),
    "explain_concept": (CONTENT, [
        "explain {topic}", "what is {topic}", "how does {topic} work", "teach me {topic}",
        "can you explain {topic} simply", "i don't understand {topic}", "describe {topic}", "why does this error happen"]),
    "translate_text": (CONTENT, [
        "translate this to {tlang}", "say this in {tlang}", "how do you say hello in {tlang}",
        "translate the paragraph into {tlang}", "convert this text to {tlang}", "{tlang} translation of this please",
        "what is thank you in {tlang}"]),
}

# Old PDR labels -> unified labels, for relabelling earlier datasets.
OLD_TO_NEW = {
    "app_launch": "open_app", "app_close": "close_window", "window_focus": "focus_app",
    "window_tile": "layout", "window_move_ws": "workspace", "system_stats": "system_status",
    # unchanged names
    **{k: k for k in [
        "volume_control", "media_playback", "brightness_control", "screenshot", "lock_screen",
        "power_control", "wifi_control", "bluetooth_control", "vpn_control", "service_control",
        "process_list", "process_kill", "battery_status", "file_search", "file_open",
        "clipboard_control", "notification_ctrl", "mount_control", "project_scaffold", "git_workflow",
        "env_setup", "package_install", "run_tests", "deploy_workflow", "file_batch_op", "build_compile",
        "email_draft", "commit_message", "code_generate", "code_refactor", "docstring_gen",
        "summarize_text", "explain_concept", "translate_text"]},
}

# Unified label -> the intent name the shell's dispatchIntent() handles, where they differ.
SHELL_NAME = {"power_control": "shutdown",
              **{k: "generate_text" for k in ("email_draft", "commit_message", "code_generate", "code_refactor",
                                              "docstring_gen", "summarize_text", "explain_concept", "translate_text")}}


def track_of(intent):
    return INTENTS[intent][0]


assert set(OLD_TO_NEW.values()) <= set(INTENTS), set(OLD_TO_NEW.values()) - set(INTENTS)
