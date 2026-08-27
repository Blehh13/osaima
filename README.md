# Interstellar OS

An ambitious, modern, and agentic AI-powered Linux distribution based on Gentoo.

## Vision
Interstellar OS is designed to completely rethink the traditional computer interface. Away with decades-old static taskbars and start menus. Interstellar OS introduces a dynamic, agentic user interface designed for the modern era, deeply integrated with AI at the core system level.

## Architecture
- **Gentoo Base**: Leveraging the extreme flexibility of Gentoo Linux and Portage for source-based compilation and fine-grained dependency management.
- **Agentic UI**: A completely reimagined shell environment that learns and adapts, acting more as a proactive assistant than a passive application launcher.
- **Kernel-Level ML Prioritization**: Task scheduling and process prioritization are managed dynamically through Machine Learning models, interfaced safely with the Linux kernel via eBPF.
- **Model Context Protocol (MCP)**: Native system-wide MCP integration via a central daemon, allowing AI services and agents seamless context-awareness of the user's workflow.

## Directory Structure
- `kernel/`: Contains the Linux kernel submodule and custom eBPF schedulers.
- `gentoo/`: The core Gentoo ebuild recipes and Portage submodules, plus our own **fork overlay** (`gentoo/overlay/`) — the Interstellar profile, branding/release package, and ebuilds for the shell and AI core. See [BUILD-DISTRO.md](BUILD-DISTRO.md).
- `ai-core/`: System daemons handling MCP (`mcp-daemon`) and background agentic tasks (`agentic-services`).
- `ui/`: The modern shell environment and compositing interface (`osaima-shell`), including a **Lua-driven tiling window manager** (see [`ui/osaima-shell/README.md`](ui/osaima-shell/README.md)).
- `build-tools/`: Scripts for bootstrapping and generating the distribution ISO.

## Getting Started
- **Try the desktop shell now (Windows/Mac/Linux):** run `run-osaima.bat` (Windows) or serve `ui/osaima-shell/` — see [ui/osaima-shell/README.md](ui/osaima-shell/README.md).
- **Build the OS as a Gentoo fork (Linux/VM):** see [BUILD-DISTRO.md](BUILD-DISTRO.md) — enable the overlay, select the `interstellar/agentic` profile, `emerge interstellar-meta`, then build an ISO.

Interstellar keeps upstream **Gentoo as its upstream** and maintains only its own overlay (`gentoo/overlay/`): profile, branding, and packages. It is a Gentoo-family derivative (`ID_LIKE=gentoo`), not a from-scratch OS.

See [ARCHITECTURE.md](ARCHITECTURE.md) for a one-page diagram tying the distribution and the agentic shell together.
