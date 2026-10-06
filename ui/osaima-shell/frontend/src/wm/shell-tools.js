/**
 * shell-tools.js — tools the shell implements itself and offers to the agent
 * (the agent's own tools act on the operating system; these act on the desktop).
 *
 * Every argument comes from a model, so each one is validated before use.
 */

export const LAYOUTS = ['tile', 'monocle', 'grid', 'spiral', 'float'];

const REVERSIBLE = { readOnlyHint: false, destructiveHint: false };

/**
 * @param {import('./window-manager.js').WindowManager} engine
 * @returns {{ specs: object[], run: (call: {name: string, arguments: object}) => {ok: boolean, output: string} }}
 */
export function shellTools(engine) {
  const specs = () => [
    {
      name: 'shell_open_app',
      title: 'Open a built-in app',
      description:
        "Open one of the shell's built-in apps in a window: " +
        [...engine.appRegistry.values()].map((a) => `${a.id} (${a.title})`).join(', ') +
        '. Use launch_app instead for installed desktop applications such as Firefox.',
      inputSchema: {
        type: 'object',
        properties: { app: { type: 'string', enum: [...engine.appRegistry.keys()] } },
        required: ['app'],
        additionalProperties: false,
      },
      annotations: REVERSIBLE,
    },
    {
      name: 'shell_set_layout',
      title: 'Change window layout',
      description: 'Arrange the windows on the current workspace using a layout.',
      inputSchema: {
        type: 'object',
        properties: { layout: { type: 'string', enum: LAYOUTS } },
        required: ['layout'],
        additionalProperties: false,
      },
      annotations: REVERSIBLE,
    },
    {
      name: 'shell_switch_workspace',
      title: 'Switch workspace',
      description: 'Go to a workspace by its number, starting at 1.',
      inputSchema: {
        type: 'object',
        properties: { number: { type: 'integer', minimum: 1 } },
        required: ['number'],
        additionalProperties: false,
      },
      annotations: REVERSIBLE,
    },
    {
      name: 'shell_notify',
      title: 'Show a notification',
      description: 'Show a short notification on the desktop.',
      inputSchema: {
        type: 'object',
        properties: { message: { type: 'string', maxLength: 200 } },
        required: ['message'],
        additionalProperties: false,
      },
      annotations: REVERSIBLE,
    },
  ];

  const fail = (output) => ({ ok: false, output });

  function run(call) {
    const args = call.arguments ?? {};
    switch (call.name) {
      case 'shell_open_app': {
        if (typeof args.app !== 'string' || !engine.appRegistry.has(args.app)) {
          return fail(`Unknown app. Choose one of: ${[...engine.appRegistry.keys()].join(', ')}.`);
        }
        const win = engine.spawn(args.app);
        return win
          ? { ok: true, output: `Opened ${engine.appRegistry.get(args.app).title}.` }
          : fail(`Could not open ${args.app}.`);
      }
      case 'shell_set_layout': {
        if (!LAYOUTS.includes(args.layout)) return fail(`Layout must be one of: ${LAYOUTS.join(', ')}.`);
        engine.setLayout(args.layout);
        return { ok: true, output: `Layout is now ${args.layout}.` };
      }
      case 'shell_switch_workspace': {
        const count = engine.workspaces.length;
        if (!Number.isInteger(args.number) || args.number < 1 || args.number > count) {
          return fail(`Workspace must be a number from 1 to ${count}.`);
        }
        engine.switchWorkspace(args.number - 1);
        return { ok: true, output: `Switched to workspace ${args.number} (${engine.workspaces[args.number - 1].name}).` };
      }
      case 'shell_notify': {
        if (typeof args.message !== 'string' || !args.message.trim()) return fail('message is required.');
        engine.notify(args.message.trim().slice(0, 200), 'info');
        return { ok: true, output: 'Notification shown.' };
      }
      default:
        return fail(`The shell has no tool named ${call.name}.`);
    }
  }

  return { specs, run };
}
