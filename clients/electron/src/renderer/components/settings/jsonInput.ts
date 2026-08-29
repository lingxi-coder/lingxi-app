/**
 * Parses a textarea's raw text as a JSON *object* (never an array, string,
 * number, or other scalar) — the shape every MCP server config, plugin
 * config, and marketplace source declaration on these pages needs. A
 * generic helper, not owned by any one page (Task 18 fix round 1, Minor:
 * this used to live in `McpServers.tsx` and `Plugins.tsx` imported it from
 * there, which made a generic JSON parser create a page-to-page dependency
 * for no reason).
 */
export function parseJsonObjectInput(text: string): { config: Record<string, unknown> } | { error: string } {
  let parsed: unknown;
  try {
    parsed = JSON.parse(text);
  } catch (cause) {
    return { error: cause instanceof Error ? `不是合法的 JSON：${cause.message}` : '不是合法的 JSON。' };
  }
  if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) {
    return { error: '配置必须是一个 JSON 对象，例如 {"command":"npx","args":["-y","pkg"]}。' };
  }
  return { config: parsed as Record<string, unknown> };
}
