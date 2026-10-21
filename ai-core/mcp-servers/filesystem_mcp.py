import asyncio
import os
import glob
from mcp.server import Server
from mcp.server.stdio import stdio_server
from mcp.types import Tool, TextContent

app = Server("osaima-filesystem")

@app.list_tools()
async def list_tools() -> list[Tool]:
    return [
        Tool(
            name="read_file",
            description="Read the contents of a text file",
            inputSchema={
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Absolute path to the file"}
                },
                "required": ["path"]
            }
        ),
        Tool(
            name="list_directory",
            description="List contents of a directory",
            inputSchema={
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Absolute path to the directory"}
                },
                "required": ["path"]
            }
        ),
        Tool(
            name="search_files",
            description="Search for files by glob pattern",
            inputSchema={
                "type": "object",
                "properties": {
                    "pattern": {"type": "string", "description": "Glob pattern e.g. /home/user/*.txt"}
                },
                "required": ["pattern"]
            }
        )
    ]

@app.call_tool()
async def call_tool(name: str, arguments: dict) -> list[TextContent]:
    if name == "read_file":
        path = os.path.expanduser(arguments.get("path"))
        try:
            with open(path, "r", encoding="utf-8") as f:
                content = f.read()
            return [TextContent(type="text", text=content[:10000])] # Limit to 10k chars
        except Exception as e:
            return [TextContent(type="text", text=f"Error reading file: {e}")]
            
    elif name == "list_directory":
        path = os.path.expanduser(arguments.get("path"))
        try:
            items = os.listdir(path)
            res = []
            for item in items:
                full = os.path.join(path, item)
                typ = "DIR" if os.path.isdir(full) else "FILE"
                res.append(f"[{typ}] {item}")
            return [TextContent(type="text", text="\n".join(res))]
        except Exception as e:
            return [TextContent(type="text", text=f"Error listing directory: {e}")]
            
    elif name == "search_files":
        pattern = os.path.expanduser(arguments.get("pattern"))
        try:
            matches = glob.glob(pattern, recursive=True)
            return [TextContent(type="text", text="\n".join(matches[:100]))]
        except Exception as e:
            return [TextContent(type="text", text=f"Error searching files: {e}")]

    raise ValueError(f"Unknown tool: {name}")

async def main():
    async with stdio_server() as (read_stream, write_stream):
        await app.run(read_stream, write_stream, app.create_initialization_options())

if __name__ == "__main__":
    asyncio.run(main())
