# Linux Developer Cheatsheet (RAG seed document)

## Process and resource inspection
- List processes by CPU: `ps aux --sort=-%cpu | head`
- Kill a process: `kill -9 <PID>` (irreversible tier — confirm first)
- Live system monitor: `htop` or read `/proc/meminfo` and `/proc/cpuinfo`
- Disk usage by directory: `du -sh *` or `ncdu`
- Battery percentage: read `/sys/class/power_supply/BAT0/capacity`

## systemd services
- Status: `systemctl status <unit>`
- Restart: `systemctl restart <unit>`
- User services live in `~/.config/systemd/user/` and start with
  `systemctl --user enable --now <unit>`

## Git workflow
- Stage and commit: `git add -A && git commit -m "message"`
- New branch: `git switch -c feature/name`
- Push: `git push -u origin HEAD` (irreversible tier if force-pushing)
- Undo last commit but keep changes: `git reset --soft HEAD~1`

## Python environments
- Create a venv: `python -m venv .venv`
- Activate: `source .venv/bin/activate`
- Install from lockfile: `pip install -r requirements.txt`

## Search and files
- Fast content search: `rg "TODO"` (ripgrep)
- Find files modified today: `find . -mtime -1 -name "*.py"`
- Locate by name: `locate <name>` (uses the mlocate database)
