/**
 * How a Bot's engine learns of the Bot router: one entry among the MCP servers
 * of the global configuration file of the Bot's home. The entry holds no secret.
 * It names two environment variables, and the engine fills them in from the
 * environment of each session, where the host puts the router's address and the
 * token for that session. Every other key of the file stays as it is. Free of
 * Electron.
 */

import { lstatSync, readFileSync } from 'node:fs';

import { writeFileAtomic } from './files.js';
import { ROUTER_SERVER_NAME, ROUTER_TOKEN_ENV, ROUTER_URL_ENV } from './routerTools.js';

/** The entry as the engine reads it. */
export function routerServerEntry(): Record<string, unknown> {
  return {
    type: 'http',
    url: `\${${ROUTER_URL_ENV}}`,
    headers: { Authorization: `Bearer \${${ROUTER_TOKEN_ENV}}` },
  };
}

function plainObject(value: unknown): Record<string, unknown> | undefined {
  return typeof value === 'object' && value !== null && !Array.isArray(value) ? value as Record<string, unknown> : undefined;
}

/**
 * Make the router's entry the one named {@link ROUTER_SERVER_NAME} among the
 * file's `mcpServers`, writing the file (all at once, or not at all) only if it
 * is not so already. A file that is a link, or does not hold a JSON object, is
 * left alone and refused: what a Bot's configuration points at is not something
 * to follow or to guess.
 */
export function ensureRouterEntry(configPath: string): void {
  let text: string | undefined;
  try {
    if (lstatSync(configPath).isSymbolicLink()) throw new Error(`${configPath} is a symbolic link`);
    text = readFileSync(configPath, 'utf8');
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw error;
  }
  let config: Record<string, unknown> = {};
  if (text !== undefined && text.trim() !== '') {
    let parsed: unknown;
    try { parsed = JSON.parse(text); } catch { throw new Error(`${configPath} is not valid JSON`); }
    const object = plainObject(parsed);
    if (object === undefined) throw new Error(`${configPath} is not a JSON object`);
    config = object;
  }
  const servers = config['mcpServers'] === undefined ? {} : plainObject(config['mcpServers']);
  if (servers === undefined) throw new Error(`${configPath}: mcpServers must be an object`);
  const entry = routerServerEntry();
  if (JSON.stringify(servers[ROUTER_SERVER_NAME]) === JSON.stringify(entry)) return;
  writeFileAtomic(configPath, `${JSON.stringify({ ...config, mcpServers: { ...servers, [ROUTER_SERVER_NAME]: entry } }, null, 2)}\n`);
}
