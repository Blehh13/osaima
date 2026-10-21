import asyncio
import subprocess
from mcp.server import Server
from mcp.server.stdio import stdio_server
from mcp.types import Tool, TextContent

app = Server("osaima-terminal")

@app.list_tools()
async def list_tools() -> list[Tool]:
    return [
        Tool(
            name="run_command",
            description="Run a shell command safely",
            inputSchema={
                "type": "object",
                "properties": {
                    "command": {"type": "string", "description": "The bash command to run"}
                },
                "required": ["command"]
            }
        )
    ]

@app.call_tool()
async def call_tool(name: str, arguments: dict) -> list[TextContent]:
    if name == "run_command":
        command = arguments.get("command")
        
        # Here we could enforce security rules, like prefixing with systemd-run
        # For MCP, we will execute it using standard subprocess but capture output
        try:
            result = subprocess.run(
                ["bash", "-c", command],
                capture_output=True,
                text=True,
                timeout=30
            )
            out = result.stdout
            if result.stderr:
                out += "\n[STDERR]\n" + result.stderr
            return [TextContent(type="text", text=out[:10000])]
        except subprocess.TimeoutExpired:
            return [TextContent(type="text", text="Command timed out after 30 seconds")]
        except Exception as e:
            return [TextContent(type="text", text=f"Error executing command: {e}")]
            
    raise ValueError(f"Unknown tool: {name}")

async def main():
    async with stdio_server() as (read_stream, write_stream):
        await app.run(read_stream, write_stream, app.create_initialization_options())

if __name__ == "__main__":
    asyncio.run(main())
