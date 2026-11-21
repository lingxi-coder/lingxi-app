export const CH_MCP_PREPARE = 'lingxi:mcp-prepare';

/**
 * A server run through npx or uvx installs its package on its first start, which can take minutes, while the
 * runtime gives a server 30 seconds to answer `initialize`. Desktop starts such a server once before saving it,
 * so the runtime's first connect finds the package installed.
 */
export interface McpPrepareRequest {
  command: 'npx' | 'uvx';
  args: string[];
  env: Record<string, string>;
}

/** `ready`: the server answered `initialize`. `exited`: it stopped or could not start first. */
export type McpPrepareStatus = 'ready' | 'exited' | 'timeout';

export interface McpPrepareApi {
  prepare(request: McpPrepareRequest): Promise<McpPrepareStatus>;
}

const MAX_ARGS = 256;
const MAX_ENV = 256;
const MAX_TEXT = 8192;

function text(value: unknown): value is string {
  return typeof value === 'string' && value.length <= MAX_TEXT && !value.includes('\0');
}

/** The request for an MCP configuration that runs a package through npx or uvx, or null for any other. */
export function mcpPrepareRequest(config: unknown): McpPrepareRequest | null {
  if (!config || typeof config !== 'object' || Array.isArray(config)) return null;
  const { type, command, args = [], env = {} } = config as Record<string, unknown>;
  if (type !== undefined && type !== 'stdio') return null;
  if (command !== 'npx' && command !== 'uvx') return null;
  if (!Array.isArray(args) || args.length > MAX_ARGS || !args.every(text)) return null;
  if (!env || typeof env !== 'object' || Array.isArray(env)) return null;
  const entries = Object.entries(env);
  if (entries.length > MAX_ENV || entries.some(([key, value]) => !key || /[=\0]/.test(key) || !text(value))) return null;
  return { command, args: [...args], env: Object.fromEntries(entries) as Record<string, string> };
}
