/**
 * Parses a textarea's raw text as a JSON *object* (never an array, string,
 * number, or other scalar) — the shape every MCP server config, plugin
 * config, and marketplace source declaration on these pages needs. A
 * generic helper, not owned by any one page (Task 18 fix round 1, Minor:
 * this used to live in `McpServers.tsx` and `Plugins.tsx` imported it from
 * there, which made a generic JSON parser create a page-to-page dependency
 * for no reason).
 *
 * `label` names WHAT must be a JSON object in the error message — e.g.
 * `"服务器配置"`/`"插件配置"`/`"市场来源"`. Consolidating this parser into
 * one generic function (round 1) silently dropped that specificity: every
 * call site got the same bare "配置必须是一个 JSON 对象" regardless of what
 * it was validating, where `McpServers.tsx`'s own original message named
 * itself ("服务器配置必须是一个 JSON 对象"). The meaning survived (a person
 * can still tell an object from a non-object) but the page-specific
 * identification did not — restored via this parameter (Task 18 fix
 * round 2) rather than by each page keeping its own copy of the whole
 * function just to change one noun.
 */
export function parseJsonObjectInput(
  text: string, label = '配置',
): { config: Record<string, unknown> } | { error: string } {
  let parsed: unknown;
  try {
    parsed = JSON.parse(text);
  } catch (cause) {
    return { error: cause instanceof Error ? `不是合法的 JSON：${cause.message}` : '不是合法的 JSON。' };
  }
  if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) {
    return { error: `${label}必须是一个 JSON 对象，例如 {"command":"npx","args":["-y","pkg"]}。` };
  }
  return { config: parsed as Record<string, unknown> };
}
