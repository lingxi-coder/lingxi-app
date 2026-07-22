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

/** The curated provider set mirrors the CLI/TUI connect picker. */
export const PROVIDERS: readonly ProviderDefinition[] = [
  {
    id: 'anthropic', label: 'Anthropic', description: 'Claude models', popular: true,
    authMethod: 'api_key', keyLabel: 'Anthropic API key', keyPlaceholder: 'sk-ant-…',
    defaultModel: 'claude-sonnet-5', available: true,
  },
  {
    id: 'openai', label: 'OpenAI', description: 'GPT models', popular: true,
    authMethod: 'api_key', keyLabel: 'OpenAI API key', keyPlaceholder: 'sk-…',
    defaultModel: 'openai/gpt-4o', available: true,
  },
  {
    id: 'deepseek', label: 'DeepSeek', description: 'Chat and Reasoner', popular: true,
    authMethod: 'api_key', keyLabel: 'DeepSeek API key', keyPlaceholder: 'sk-…',
    defaultModel: 'deepseek/deepseek-chat', available: true,
  },
  {
    id: 'gemini', label: 'Google Gemini', description: 'Gemini models', popular: true,
    authMethod: 'api_key', keyLabel: 'Gemini API key', keyPlaceholder: 'AIza…',
    defaultModel: 'gemini/gemini-2.5-flash', available: true,
  },
  {
    id: 'openrouter', label: 'OpenRouter', description: 'One key for many models', popular: false,
    authMethod: 'api_key', keyLabel: 'OpenRouter API key', keyPlaceholder: 'sk-or-…',
    defaultModel: 'openrouter/openai/gpt-4o', available: true,
  },
  {
    id: 'zai', label: 'Z.AI', description: 'GLM models', popular: false,
    authMethod: 'api_key', keyLabel: 'Z.AI API key', keyPlaceholder: '…',
    defaultModel: 'zai/glm-4.5', available: true,
  },
  {
    id: 'glm-coding', label: 'GLM Coding Plan', description: 'Zhipu coding-plan subscription', popular: false,
    authMethod: 'api_key', keyLabel: 'GLM Coding API key', keyPlaceholder: '…',
    defaultModel: 'glm-coding/glm-4.5-air', available: true,
  },
  {
    id: 'github-copilot', label: 'GitHub Copilot', description: 'Copilot subscription', popular: true,
    authMethod: 'token', keyLabel: 'GitHub token', keyPlaceholder: 'gho_…',
    defaultModel: 'github-copilot/gpt-4o', available: true,
  },
  {
    id: 'openai-chatgpt', label: 'OpenAI (ChatGPT)', description: 'ChatGPT Plus / Pro', popular: true,
    authMethod: 'oauth', keyLabel: 'ChatGPT sign-in', keyPlaceholder: '', available: false,
  },
] as const;

export const PROVIDER_IDS = PROVIDERS.map(({ id }) => id);

export function providerById(id: string): ProviderDefinition | undefined {
  return PROVIDERS.find((provider) => provider.id === id);
}
