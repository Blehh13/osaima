import asyncio
import json
from mcp.server import Server
from mcp.server.stdio import stdio_server
from mcp.types import Tool, TextContent

try:
    import pyatspi
except ImportError:
    pyatspi = None

app = Server("osaima-ui-automator")

@app.list_tools()
async def list_tools() -> list[Tool]:
    return [
        Tool(
            name="inspect_window",
            description="Inspect the active window and return a JSON tree of clickable/typable widgets",
            inputSchema={"type": "object", "properties": {}}
        ),
        Tool(
            name="execute_ui_macro",
            description="Execute a rapid sequence of UI actions (clicks, types) locally",
            inputSchema={
                "type": "object",
                "properties": {
                    "actions": {
                        "type": "array",
                        "description": "List of actions. E.g. [{'action': 'click', 'name': 'Submit'}, {'action': 'type', 'name': 'Username', 'value': 'admin'}]",
                        "items": {
                            "type": "object",
                            "properties": {
                                "action": {"type": "string", "enum": ["click", "type"]},
                                "name": {"type": "string", "description": "Accessible name of the widget"},
                                "value": {"type": "string", "description": "Value to type (if action is type)"}
                            },
                            "required": ["action", "name"]
                        }
                    }
                },
                "required": ["actions"]
            }
        )
    ]

def _find_widget_by_name(node, name):
    if not node: return None
    try:
        if node.name and name.lower() in node.name.lower():
            return node
    except:
        pass

    for i in range(node.childCount):
        try:
            child = node.getChildAtIndex(i)
            found = _find_widget_by_name(child, name)
            if found:
                return found
        except:
            pass
    return None

def _get_active_window():
    if not pyatspi:
        return None
    desktop = pyatspi.Registry.getDesktop(0)
    # Finding the active window usually requires checking active states, 
    # but for simplicity we grab the first focused application.
    for i in range(desktop.childCount):
        app = desktop.getChildAtIndex(i)
        for j in range(app.childCount):
            win = app.getChildAtIndex(j)
            state = win.getState()
            if state.contains(pyatspi.STATE_ACTIVE) or state.contains(pyatspi.STATE_FOCUSED):
                return win
    return None

def _dump_tree(node, depth=0):
    if depth > 3 or not node: return []
    res = []
    try:
        role_name = node.getRoleName()
        if role_name in ['push button', 'menu item', 'text', 'check box']:
            res.append({
                "name": node.name,
                "role": role_name,
                "description": node.description
            })
    except:
        pass
        
    try:
        for i in range(node.childCount):
            res.extend(_dump_tree(node.getChildAtIndex(i), depth + 1))
    except:
        pass
    return res

@app.call_tool()
async def call_tool(name: str, arguments: dict) -> list[TextContent]:
    if not pyatspi:
        return [TextContent(type="text", text="Error: pyatspi is not installed on this system. Cannot perform UI automation.")]

    if name == "inspect_window":
        win = _get_active_window()
        if not win:
            return [TextContent(type="text", text="No active window found.")]
        tree = _dump_tree(win)
        return [TextContent(type="text", text=json.dumps(tree, indent=2))]

    elif name == "execute_ui_macro":
        actions = arguments.get("actions", [])
        win = _get_active_window()
        if not win:
            return [TextContent(type="text", text="No active window found.")]

        log = []
        for act in actions:
            target_name = act.get("name")
            action_type = act.get("action")
            
            widget = _find_widget_by_name(win, target_name)
            if not widget:
                log.append(f"Failed to find widget: {target_name}")
                continue
                
            try:
                if action_type == "click":
                    action_intf = widget.queryAction()
                    action_intf.doAction(0)
                    log.append(f"Clicked {target_name}")
                elif action_type == "type":
                    text_intf = widget.queryText()
                    text_intf.setText(act.get("value", ""))
                    log.append(f"Typed into {target_name}")
            except Exception as e:
                log.append(f"Error acting on {target_name}: {e}")
                
        return [TextContent(type="text", text="\n".join(log))]

    raise ValueError(f"Unknown tool: {name}")

async def main():
    async with stdio_server() as (read_stream, write_stream):
        await app.run(read_stream, write_stream, app.create_initialization_options())

if __name__ == "__main__":
    asyncio.run(main())
