export type ProviderAuthMethod = 'api_key' | 'token' | 'oauth' | 'device_code';

export interface ProviderDefinition {
  id: string;
  label: string;
  description: string;
  popular: boolean;
  authMethod: ProviderAuthMethod;
  keyLabel: string;
  keyPlaceholder: string;
  /** Built-in API base used by the engine's provider preset. */
  defaultApiBase: string;
  /** Official page where the user can create or manage this credential. */
  credentialManagementUrl: string;
  defaultModel?: string;
  available: boolean;
  /**
   * Whether this provider exposes a hosted audio-transcription (speech-to-text)
   * endpoint reachable with the same credential configured for chat, so the
   * desktop's voice input can call it. Required on every entry (not
   * optional) so a new provider cannot land without an explicit answer.
   *
   * Default posture is `false`. Flip a provider to `true` ONLY when you have
   * verified, documented evidence of a real hosted transcription endpoint —
   * never infer it from a vendor's reputation or the provider's name. See the
   * per-provider comment below for the evidence behind every value.
   *
   * Under-claiming (marking a capable provider `false`) is the safe
   * direction: the UI reports a capability as missing that actually exists.
   * Over-claiming sends a user's audio to an endpoint that 404s and reports
   * that as a transcription failure — a configured Anthropic key, for
   * example, gives chat, not transcription, because Anthropic's API has no
   * transcription endpoint at all.
   */
  transcriptionCapable: boolean;
}

/** Providers with a credential flow wired into the Electron build. */
export const PROVIDERS: readonly ProviderDefinition[] = [
  {
    id: 'anthropic', label: 'Anthropic', description: 'Claude models', popular: true,
    authMethod: 'api_key', keyLabel: 'Anthropic API key', keyPlaceholder: 'sk-ant-…',
    defaultApiBase: 'https://api.anthropic.com',
    credentialManagementUrl: 'https://console.anthropic.com/settings/keys',
    defaultModel: 'claude-sonnet-5', available: true,
    // Anthropic's public API surface is chat/messages only — no audio input,
    // no transcription operation of any kind.
    transcriptionCapable: false,
  },
  {
    id: 'openai', label: 'OpenAI', description: 'GPT models', popular: true,
    authMethod: 'api_key', keyLabel: 'OpenAI API key', keyPlaceholder: 'sk-…',
    defaultApiBase: 'https://api.openai.com/v1',
    credentialManagementUrl: 'https://platform.openai.com/api-keys',
    defaultModel: 'openai/gpt-5.6-sol', available: true,
    // OpenAI hosts a dedicated `POST /v1/audio/transcriptions` endpoint
    // (Whisper / gpt-4o-transcribe family), documented at
    // developers.openai.com/api/docs/guides/speech-to-text — reachable with
    // the same API key configured here.
    transcriptionCapable: true,
  },
  {
    id: 'deepseek', label: 'DeepSeek', description: 'V4.1 Flash and V4 Pro', popular: true,
    authMethod: 'api_key', keyLabel: 'DeepSeek API key', keyPlaceholder: 'sk-…',
    defaultApiBase: 'https://api.deepseek.com',
    credentialManagementUrl: 'https://platform.deepseek.com/api_keys',
    defaultModel: 'deepseek/deepseek-flash', available: true,
    // DeepSeek's hosted API is text (and vision) chat completions only; it
    // has no audio-input or transcription endpoint. Every DeepSeek voice-app
    // guide pairs it with a third-party STT service for exactly this reason.
    transcriptionCapable: false,
  },
  {
    id: 'kimi', label: 'Kimi', description: 'Moonshot AI models', popular: true,
    authMethod: 'api_key', keyLabel: 'Kimi API key', keyPlaceholder: 'sk-…',
    defaultApiBase: 'https://api.moonshot.cn/v1',
    credentialManagementUrl: 'https://platform.kimi.com/console/api-keys',
    defaultModel: 'kimi/kimi-k3', available: true,
    // Moonshot's hosted chat-completions API does not accept audio input.
    // Moonshot has published an open-weight "Kimi-Audio" ASR-capable model,
    // but it ships only as downloadable weights (Hugging Face / GitHub) —
    // it is not exposed as a callable endpoint on the hosted platform this
    // credential authenticates against.
    transcriptionCapable: false,
  },
  {
    id: 'kimi-code', label: 'Kimi Code', description: 'Coding membership models', popular: true,
    authMethod: 'api_key', keyLabel: 'Kimi Code API key', keyPlaceholder: 'sk-…',
    defaultApiBase: 'https://api.kimi.com/coding/v1',
    credentialManagementUrl: 'https://www.kimi.com/code/console',
    defaultModel: 'kimi-code/k3', available: true,
    // Same hosted Moonshot platform as `kimi`, scoped further to a coding
    // membership; the same absence of a hosted transcription endpoint
    // applies, and the narrower scope only reduces access further.
    transcriptionCapable: false,
  },
  {
    id: 'gemini', label: 'Google Gemini', description: 'Gemini models', popular: true,
    authMethod: 'api_key', keyLabel: 'Gemini API key', keyPlaceholder: 'AIza…',
    defaultApiBase: 'https://generativelanguage.googleapis.com/v1beta',
    credentialManagementUrl: 'https://aistudio.google.com/app/apikey',
    defaultModel: 'gemini/gemini-3.7-flash', available: true,
    // Google documents first-party audio transcription in the Gemini API
    // (ai.google.dev/gemini-api/docs/transcribe and .../docs/audio):
    // `generateContent` accepts an audio input part and returns a
    // transcript, with automatic language identification — reachable with
    // the same API key configured here.
    transcriptionCapable: true,
  },
  {
    id: 'openrouter', label: 'OpenRouter', description: 'One key for many models', popular: false,
    authMethod: 'api_key', keyLabel: 'OpenRouter API key', keyPlaceholder: 'sk-or-…',
    defaultApiBase: 'https://openrouter.ai/api/v1',
    credentialManagementUrl: 'https://openrouter.ai/settings/keys',
    defaultModel: 'openrouter/openrouter/auto', available: true,
    // OpenRouter runs a dedicated `POST /api/v1/audio/transcriptions`
    // endpoint (openrouter.ai/docs/api/api-reference/transcriptions/create-audio-transcriptions,
    // announced in their "audio APIs" post), authenticated with the same
    // bearer key used for chat completions.
    transcriptionCapable: true,
  },
  {
    id: 'zai', label: 'Z.AI', description: 'GLM models', popular: false,
    authMethod: 'api_key', keyLabel: 'Z.AI API key', keyPlaceholder: '…',
    defaultApiBase: 'https://api.z.ai/api/paas/v4',
    credentialManagementUrl: 'https://z.ai/manage-apikey/apikey-list',
    defaultModel: 'zai/glm-5.3', available: true,
    // Z.AI's open platform documents a hosted ASR model, GLM-ASR-2512
    // (docs.z.ai/guides/audio/glm-asr-2512), callable via the same Z.AI API
    // key/base URL as the chat models this entry configures.
    transcriptionCapable: true,
  },
  {
    id: 'glm-coding', label: 'GLM Coding Plan', description: 'Zhipu coding-plan subscription', popular: false,
    authMethod: 'api_key', keyLabel: 'GLM Coding API key', keyPlaceholder: '…',
    defaultApiBase: 'https://open.bigmodel.cn/api/anthropic',
    credentialManagementUrl: 'https://bigmodel.cn/usercenter/proj-mgmt/apikeys',
    defaultModel: 'glm-coding/glm-5.3', available: true,
    // Unlike `zai`, this is a scoped coding-assistant subscription (bundles
    // GLM chat/coding models for tools like Claude Code/Cursor/Cline). Its
    // published scope is coding-tool integration; nothing documents this
    // subscription's credential also granting access to the general
    // platform's GLM-ASR endpoint, so it is not assumed capable.
    transcriptionCapable: false,
  },
  {
    id: 'github-copilot', label: 'GitHub Copilot', description: 'Copilot subscription', popular: true,
    authMethod: 'token', keyLabel: 'GitHub token', keyPlaceholder: 'gho_…',
    defaultApiBase: 'https://api.githubcopilot.com',
    credentialManagementUrl: 'https://github.com/settings/tokens',
    defaultModel: 'github-copilot/claude-opus-5', available: true,
    // GitHub ended the "Copilot Voice" technical preview in 2024 and
    // publishes no audio-transcription API endpoint. Copilot CLI's `/voice`
    // command transcribes locally on-device, not through a Copilot backend
    // endpoint, so a Copilot token grants no server-side transcription
    // capability.
    transcriptionCapable: false,
  },
  // ChatGPT OAuth is intentionally omitted until the Electron credential
  // bridge supports its sign-in flow; CLI/TUI and mobile keep the route.
] as const;

export const PROVIDER_IDS = PROVIDERS.map(({ id }) => id);

export function providerById(id: string): ProviderDefinition | undefined {
  return PROVIDERS.find((provider) => provider.id === id);
}
