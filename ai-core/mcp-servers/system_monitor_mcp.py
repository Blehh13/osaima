import asyncio
import psutil
from mcp.server import Server
from mcp.server.stdio import stdio_server
from mcp.types import Tool, TextContent

app = Server("osaima-system-monitor")

@app.list_tools()
async def list_tools() -> list[Tool]:
    return [
        Tool(
            name="get_cpu_usage",
            description="Get the current CPU usage percentage",
            inputSchema={"type": "object", "properties": {}}
        ),
        Tool(
            name="get_memory_usage",
            description="Get the current memory usage (RAM) in percent",
            inputSchema={"type": "object", "properties": {}}
        ),
        Tool(
            name="get_disk_usage",
            description="Get the disk usage percentage for the root partition",
            inputSchema={"type": "object", "properties": {}}
        ),
    ]

@app.call_tool()
async def call_tool(name: str, arguments: dict) -> list[TextContent]:
    if name == "get_cpu_usage":
        cpu = psutil.cpu_percent(interval=0.5)
        return [TextContent(type="text", text=f"CPU Usage: {cpu}%")]
    
    elif name == "get_memory_usage":
        mem = psutil.virtual_memory()
        return [TextContent(type="text", text=f"Memory Usage: {mem.percent}% ({mem.used / (1024**3):.2f} GB / {mem.total / (1024**3):.2f} GB)")]
    
    elif name == "get_disk_usage":
        disk = psutil.disk_usage('/')
        return [TextContent(type="text", text=f"Disk Usage: {disk.percent}% ({disk.used / (1024**3):.2f} GB / {disk.total / (1024**3):.2f} GB)")]
    
    raise ValueError(f"Unknown tool: {name}")

async def main():
    async with stdio_server() as (read_stream, write_stream):
        await app.run(read_stream, write_stream, app.create_initialization_options())

if __name__ == "__main__":
    asyncio.run(main())
