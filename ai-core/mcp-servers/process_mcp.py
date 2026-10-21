import asyncio
import psutil
import json
from mcp.server import Server
from mcp.server.stdio import stdio_server
from mcp.types import Tool, TextContent

app = Server("osaima-process-manager")

@app.list_tools()
async def list_tools() -> list[Tool]:
    return [
        Tool(
            name="list_processes",
            description="List top running processes sorted by CPU or Memory",
            inputSchema={
                "type": "object",
                "properties": {
                    "sort_by": {"type": "string", "enum": ["cpu", "memory"], "description": "Sort by cpu or memory"},
                    "limit": {"type": "integer", "description": "Number of processes to return (default 10)", "default": 10}
                },
                "required": ["sort_by"]
            }
        ),
        Tool(
            name="kill_process",
            description="Kill a process by its PID",
            inputSchema={
                "type": "object",
                "properties": {
                    "pid": {"type": "integer", "description": "The Process ID to kill"}
                },
                "required": ["pid"]
            }
        )
    ]

@app.call_tool()
async def call_tool(name: str, arguments: dict) -> list[TextContent]:
    if name == "list_processes":
        sort_by = arguments.get("sort_by", "cpu")
        limit = arguments.get("limit", 10)
        
        procs = []
        for p in psutil.process_iter(['pid', 'name', 'cpu_percent', 'memory_percent']):
            try:
                procs.append(p.info)
            except (psutil.NoSuchProcess, psutil.AccessDenied):
                pass
                
        if sort_by == "cpu":
            procs.sort(key=lambda x: x['cpu_percent'] or 0.0, reverse=True)
        else:
            procs.sort(key=lambda x: x['memory_percent'] or 0.0, reverse=True)
            
        return [TextContent(type="text", text=json.dumps(procs[:limit], indent=2))]
        
    elif name == "kill_process":
        pid = arguments.get("pid")
        try:
            p = psutil.Process(pid)
            p.terminate()
            return [TextContent(type="text", text=f"Terminated process {pid}")]
        except Exception as e:
            return [TextContent(type="text", text=f"Failed to terminate {pid}: {e}")]
            
    raise ValueError(f"Unknown tool: {name}")

async def main():
    async with stdio_server() as (read_stream, write_stream):
        await app.run(read_stream, write_stream, app.create_initialization_options())

if __name__ == "__main__":
    asyncio.run(main())
