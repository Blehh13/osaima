"""Drive the daemon with the official MCP Python SDK over stdio.

Usage: python mcp_sdk_client.py path/to/mcp-daemon
"""

import asyncio
import sys

from mcp import ClientSession, StdioServerParameters
from mcp.client.stdio import stdio_client

EXPECTED_TOOLS = {"get_system_stats", "list_processes", "kill_process", "list_disks"}


async def main(binary: str) -> None:
    params = StdioServerParameters(command=binary, args=["--stdio"])
    async with stdio_client(params) as (read, write):
        async with ClientSession(read, write) as session:
            init = await session.initialize()
            assert init.serverInfo.name == "osaima-mcp-daemon", init.serverInfo
            print(f"negotiated protocol {init.protocolVersion}")

            await session.send_ping()

            tools = await session.list_tools()
            names = {tool.name for tool in tools.tools}
            assert EXPECTED_TOOLS <= names, names

            stats = await session.call_tool("get_system_stats", {})
            assert not stats.isError, stats
            assert stats.structuredContent["memory"]["total_bytes"] > 0

            procs = await session.call_tool("list_processes", {"limit": 5, "sort_by": "memory"})
            assert not procs.isError, procs
            assert 0 < len(procs.structuredContent["processes"]) <= 5

            refused = await session.call_tool("kill_process", {"pid": 1})
            assert refused.isError, "PID 1 must be protected"

    print("MCP SDK interoperability: OK")


if __name__ == "__main__":
    asyncio.run(main(sys.argv[1]))
