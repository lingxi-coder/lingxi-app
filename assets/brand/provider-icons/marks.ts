const claude = new URL('./claude-color.svg', import.meta.url).href;
const deepseek = new URL('./deepseek-color.svg', import.meta.url).href;
const gemini = new URL('./gemini-color.svg', import.meta.url).href;
const copilot = new URL('./githubcopilot.svg', import.meta.url).href;
const kimi = new URL('./kimi.svg', import.meta.url).href;
const minimax = new URL('./minimax-color.svg', import.meta.url).href;
const qwen = new URL('./qwen-color.svg', import.meta.url).href;
const openai = new URL('./openai.svg', import.meta.url).href;
const openrouter = new URL('./openrouter.svg', import.meta.url).href;
const xai = new URL('./xai.svg', import.meta.url).href;
const zai = new URL('./zai.svg', import.meta.url).href;

const marks = new Map<string, { source: string; color?: boolean }>([
  ['anthropic', { source: claude, color: true }],
  ['deepseek', { source: deepseek, color: true }],
  ['gemini', { source: gemini, color: true }],
  ['github-copilot', { source: copilot }],
  ['kimi', { source: kimi }],
  ['kimi-code', { source: kimi }],
  ['minimax', { source: minimax, color: true }],
  ['qwen', { source: qwen, color: true }],
  ['openai', { source: openai }],
  ['openrouter', { source: openrouter }],
  ['xai', { source: xai }],
  ['zai', { source: zai }],
  ['glm-coding', { source: zai }],
]);

export function providerBrandMark(id: string) {
  return marks.get(id === 'openai-chatgpt' ? 'openai' : id === 'google' ? 'gemini' : id);
}
