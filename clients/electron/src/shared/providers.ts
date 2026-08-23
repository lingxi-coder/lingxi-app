export type ProviderAuthMethod = 'api_key' | 'token' | 'oauth' | 'device_code';

export interface ProviderDefinition {
  id: string;
  label: string;
  description: string;
  popular: boolean;
  authMethod: ProviderAuthMethod;
  keyLabel: string;
  keyPlaceholder: string;
  defaultModel?: string;
  available: boolean;
}

/** Providers with a credential flow wired into the Electron build. */
export const PROVIDERS: readonly ProviderDefinition[] = [
  {
    id: 'anthropic', label: 'Anthropic', description: 'Claude models', popular: true,
    authMethod: 'api_key', keyLabel: 'Anthropic API key', keyPlaceholder: 'sk-ant-…',
    defaultModel: 'claude-sonnet-5', available: true,
  },
  {
    id: 'openai', label: 'OpenAI', description: 'GPT models', popular: true,
    authMethod: 'api_key', keyLabel: 'OpenAI API key', keyPlaceholder: 'sk-…',
    defaultModel: 'openai/gpt-5.6-sol', available: true,
  },
  {
    id: 'deepseek', label: 'DeepSeek', description: 'V4 text and vision models', popular: true,
    authMethod: 'api_key', keyLabel: 'DeepSeek API key', keyPlaceholder: 'sk-…',
    defaultModel: 'deepseek/deepseek-v4-flash', available: true,
  },
  {
    id: 'kimi', label: 'Kimi', description: 'Moonshot AI models', popular: true,
    authMethod: 'api_key', keyLabel: 'Kimi API key', keyPlaceholder: 'sk-…',
    defaultModel: 'kimi/kimi-k3', available: true,
  },
  {
    id: 'kimi-code', label: 'Kimi Code', description: 'Coding membership models', popular: true,
    authMethod: 'api_key', keyLabel: 'Kimi Code API key', keyPlaceholder: 'sk-…',
    defaultModel: 'kimi-code/k3', available: true,
  },
  {
    id: 'gemini', label: 'Google Gemini', description: 'Gemini models', popular: true,
    authMethod: 'api_key', keyLabel: 'Gemini API key', keyPlaceholder: 'AIza…',
    defaultModel: 'gemini/gemini-3.7-flash', available: true,
  },
  {
    id: 'openrouter', label: 'OpenRouter', description: 'One key for many models', popular: false,
    authMethod: 'api_key', keyLabel: 'OpenRouter API key', keyPlaceholder: 'sk-or-…',
    defaultModel: 'openrouter/openrouter/auto', available: true,
  },
  {
    id: 'zai', label: 'Z.AI', description: 'GLM models', popular: false,
    authMethod: 'api_key', keyLabel: 'Z.AI API key', keyPlaceholder: '…',
    defaultModel: 'zai/glm-5.3', available: true,
  },
  {
    id: 'glm-coding', label: 'GLM Coding Plan', description: 'Zhipu coding-plan subscription', popular: false,
    authMethod: 'api_key', keyLabel: 'GLM Coding API key', keyPlaceholder: '…',
    defaultModel: 'glm-coding/glm-5.3', available: true,
  },
  {
    id: 'github-copilot', label: 'GitHub Copilot', description: 'Copilot subscription', popular: true,
    authMethod: 'token', keyLabel: 'GitHub token', keyPlaceholder: 'gho_…',
    defaultModel: 'github-copilot/claude-opus-5', available: true,
  },
  // ChatGPT OAuth is intentionally omitted until the Electron credential
  // bridge supports its sign-in flow; CLI/TUI and mobile keep the route.
] as const;

export const PROVIDER_IDS = PROVIDERS.map(({ id }) => id);

export function providerById(id: string): ProviderDefinition | undefined {
  return PROVIDERS.find((provider) => provider.id === id);
}
