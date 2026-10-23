"""Drive osaima-mcp with the official MCP Python SDK over stdio, against a
running mcp-daemon. Reads only, plus one change that must be refused because
the user didn't confirm it. (After Banu's mcp_sdk_client.py on feat/m1-foundation.)

Usage: python osaima_mcp_sdk_client.py path/to/osaima-mcp
"""

import asyncio
import sys

from mcp import ClientSession, StdioServerParameters
from mcp.client.stdio import stdio_client

READ_ONLY = {"get_system_stats", "get_health", "list_windows", "search_files", "get_settings",
             "list_automations", "recent_activity", "understand_request"}
CHANGES = {"control_device", "change_setting", "open_app", "manage_files", "create_automation"}


async def main(binary: str) -> None:
    async with stdio_client(StdioServerParameters(command=binary)) as (read, write):
        async with ClientSession(read, write) as session:
            init = await session.initialize()
            assert init.serverInfo.name == "osaima", init.serverInfo
            print(f"negotiated protocol {init.protocolVersion}")
            await session.send_ping()

            tools = {t.name: t for t in (await session.list_tools()).tools}
            assert READ_ONLY | CHANGES == set(tools), set(tools)
            for name in CHANGES:
                assert tools[name].annotations.destructiveHint, name
            print(f"{len(tools)} tools, {len(CHANGES)} marked destructive")

            stats = await session.call_tool("get_system_stats", {})
            assert not stats.isError, stats
            health = await session.call_tool("get_health", {})
            assert not health.isError and "alerts" in health.structuredContent, health
            found = await session.call_tool("search_files", {"kind": "pdf", "limit": 3})
            assert not found.isError, found
            understood = await session.call_tool("understand_request", {"request": "set brightness to 40%"})
            assert understood.structuredContent["intent"] == "os_action", understood.structuredContent
            print("read-only tools: OK")

            refused = await session.call_tool("control_device", {"action": "volume", "op": "set", "value": 5})
            assert refused.isError and "approv" in refused.content[0].text.lower(), refused
            print("unconfirmed change refused:", refused.content[0].text)

    print("MCP SDK interoperability: OK")


if __name__ == "__main__":
    asyncio.run(main(sys.argv[1]))
