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
- `gentoo/`: The core Gentoo ebuild recipes and Portage package manager submodules.
- `ai-core/`: System daemons handling MCP (`mcp-daemon`) and background agentic tasks (`agentic-services`).
- `ui/`: The modern shell environment and compositing interface (`osaima-shell`), including a **Lua-driven tiling window manager** (see [`ui/osaima-shell/README.md`](ui/osaima-shell/README.md)).
- `build-tools/`: Scripts for bootstrapping and generating the distribution ISO.

## Getting Started
*(Instructions for compiling the system and bootstrapping the environment will be added here as the build tools are developed.)*
