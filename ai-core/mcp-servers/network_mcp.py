import asyncio
import subprocess
from mcp.server import Server
from mcp.server.stdio import stdio_server
from mcp.types import Tool, TextContent

app = Server("osaima-network")

@app.list_tools()
async def list_tools() -> list[Tool]:
    return [
        Tool(
            name="get_network_interfaces",
            description="Get a list of network interfaces and their IP addresses",
            inputSchema={"type": "object", "properties": {}}
        ),
        Tool(
            name="ping_host",
            description="Ping a host to check connectivity",
            inputSchema={
                "type": "object",
                "properties": {
                    "host": {"type": "string", "description": "Hostname or IP to ping"}
                },
                "required": ["host"]
            }
        )
    ]

@app.call_tool()
async def call_tool(name: str, arguments: dict) -> list[TextContent]:
    if name == "get_network_interfaces":
        try:
            result = subprocess.run(["ip", "-br", "addr", "show"], capture_output=True, text=True)
            return [TextContent(type="text", text=result.stdout)]
        except Exception as e:
            return [TextContent(type="text", text=f"Error getting network interfaces: {e}")]
            
    elif name == "ping_host":
        host = arguments.get("host")
        try:
            result = subprocess.run(["ping", "-c", "4", host], capture_output=True, text=True)
            return [TextContent(type="text", text=result.stdout)]
        except Exception as e:
            return [TextContent(type="text", text=f"Error pinging host: {e}")]
            
    raise ValueError(f"Unknown tool: {name}")

async def main():
    async with stdio_server() as (read_stream, write_stream):
        await app.run(read_stream, write_stream, app.create_initialization_options())

if __name__ == "__main__":
    asyncio.run(main())
