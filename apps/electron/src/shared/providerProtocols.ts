// Generated from the pinned llm-client provider catalog (87e4ae272a1c).
// Change provider facts in llm-client, then regenerate this catalog.
export const PROVIDER_PROTOCOLS = [
  // Host-owned ChatGPT account transport; separate from the OpenAI API-key account.
  { profile: "openai-chatgpt", provider: "openai-chatgpt", regions: ["international"], native: "open_ai_responses", formats: ["open_ai_responses"] },
  {
    "profile": "anthropic",
    "provider": "anthropic",
    "regions": [
      "international"
    ],
    "native": "anthropic_messages",
    "formats": [
      "anthropic_messages",
      "open_ai_chat"
    ]
  },
  {
    "profile": "deepseek-search",
    "provider": "deepseek",
    "regions": [
      "china_mainland",
      "international"
    ],
    "native": "anthropic_messages",
    "formats": [
      "anthropic_messages",
      "open_ai_chat",
      "open_ai_responses"
    ]
  },
  {
    "profile": "deepseek",
    "provider": "deepseek",
    "regions": [
      "china_mainland",
      "international"
    ],
    "native": "open_ai_chat",
    "formats": [
      "open_ai_chat",
      "anthropic_messages",
      "open_ai_responses"
    ]
  },
  {
    "profile": "gemini",
    "provider": "google",
    "regions": [
      "international"
    ],
    "native": "gemini_generate_content",
    "formats": [
      "gemini_generate_content",
      "open_ai_chat"
    ]
  },
  {
    "profile": "github-copilot",
    "provider": "github-copilot",
    "regions": [
      "international"
    ],
    "native": "open_ai_chat",
    "formats": [
      "open_ai_chat"
    ]
  },
  {
    "profile": "glm-coding",
    "provider": "zhipu",
    "regions": [
      "china_mainland"
    ],
    "native": "anthropic_messages",
    "formats": [
      "anthropic_messages"
    ]
  },
  {
    "profile": "glm",
    "provider": "zhipu",
    "regions": [
      "china_mainland"
    ],
    "native": "open_ai_chat",
    "formats": [
      "open_ai_chat"
    ]
  },
  {
    "profile": "grok-responses",
    "provider": "xai",
    "regions": [
      "international"
    ],
    "native": "open_ai_responses",
    "formats": [
      "open_ai_responses",
      "open_ai_chat",
      "anthropic_messages"
    ]
  },
  {
    "profile": "grok",
    "provider": "xai",
    "regions": [
      "international"
    ],
    "native": "open_ai_chat",
    "formats": [
      "open_ai_chat",
      "open_ai_responses",
      "anthropic_messages"
    ]
  },
  {
    "profile": "kimi-code",
    "provider": "kimi",
    "regions": [
      "china_mainland",
      "international"
    ],
    "native": "open_ai_chat",
    "formats": [
      "open_ai_chat",
      "open_ai_responses",
      "anthropic_messages"
    ]
  },
  {
    "profile": "kimi-intl",
    "provider": "kimi",
    "regions": [
      "international"
    ],
    "native": "open_ai_chat",
    "formats": [
      "open_ai_chat",
      "anthropic_messages",
      "open_ai_responses"
    ]
  },
  {
    "profile": "kimi-search-intl",
    "provider": "kimi",
    "regions": [
      "international"
    ],
    "native": "open_ai_responses",
    "formats": [
      "open_ai_responses",
      "anthropic_messages",
      "open_ai_chat"
    ]
  },
  {
    "profile": "kimi-search",
    "provider": "kimi",
    "regions": [
      "china_mainland"
    ],
    "native": "open_ai_responses",
    "formats": [
      "open_ai_responses",
      "open_ai_chat"
    ]
  },
  {
    "profile": "kimi",
    "provider": "kimi",
    "regions": [
      "china_mainland"
    ],
    "native": "open_ai_chat",
    "formats": [
      "open_ai_chat",
      "open_ai_responses"
    ]
  },
  {
    "profile": "minimax-intl",
    "provider": "minimax",
    "regions": [
      "international"
    ],
    "native": "anthropic_messages",
    "formats": [
      "anthropic_messages",
      "open_ai_responses",
      "open_ai_chat"
    ]
  },
  {
    "profile": "minimax",
    "provider": "minimax",
    "regions": [
      "china_mainland"
    ],
    "native": "anthropic_messages",
    "formats": [
      "anthropic_messages",
      "open_ai_responses",
      "open_ai_chat"
    ]
  },
  {
    "profile": "openai",
    "provider": "openai",
    "regions": [
      "international"
    ],
    "native": "open_ai_responses",
    "formats": [
      "open_ai_responses",
      "open_ai_chat"
    ]
  },
  {
    "profile": "openrouter",
    "provider": "openrouter",
    "regions": [
      "international"
    ],
    "native": "open_ai_chat",
    "formats": [
      "open_ai_chat",
      "open_ai_responses",
      "anthropic_messages"
    ]
  },
  {
    "profile": "qwen-hk",
    "provider": "qwen",
    "regions": [
      "international"
    ],
    "native": "open_ai_chat",
    "formats": [
      "open_ai_chat",
      "open_ai_responses"
    ]
  },
  {
    "profile": "qwen-intl",
    "provider": "qwen",
    "regions": [
      "international"
    ],
    "native": "open_ai_chat",
    "formats": [
      "open_ai_chat",
      "anthropic_messages",
      "open_ai_responses"
    ]
  },
  {
    "profile": "qwen-search-hk",
    "provider": "qwen",
    "regions": [
      "international"
    ],
    "native": "open_ai_responses",
    "formats": [
      "open_ai_responses",
      "open_ai_chat"
    ]
  },
  {
    "profile": "qwen-search-intl",
    "provider": "qwen",
    "regions": [
      "international"
    ],
    "native": "open_ai_responses",
    "formats": [
      "open_ai_responses",
      "anthropic_messages",
      "open_ai_chat"
    ]
  },
  {
    "profile": "qwen-search-us",
    "provider": "qwen",
    "regions": [
      "international"
    ],
    "native": "open_ai_responses",
    "formats": [
      "open_ai_responses",
      "open_ai_chat"
    ]
  },
  {
    "profile": "qwen-search",
    "provider": "qwen",
    "regions": [
      "china_mainland"
    ],
    "native": "open_ai_responses",
    "formats": [
      "open_ai_responses",
      "anthropic_messages",
      "open_ai_chat"
    ]
  },
  {
    "profile": "qwen-us",
    "provider": "qwen",
    "regions": [
      "international"
    ],
    "native": "open_ai_chat",
    "formats": [
      "open_ai_chat",
      "open_ai_responses"
    ]
  },
  {
    "profile": "qwen",
    "provider": "qwen",
    "regions": [
      "china_mainland"
    ],
    "native": "open_ai_chat",
    "formats": [
      "open_ai_chat",
      "anthropic_messages",
      "open_ai_responses"
    ]
  },
  {
    "profile": "zai-coding",
    "provider": "zhipu",
    "regions": [
      "international"
    ],
    "native": "anthropic_messages",
    "formats": [
      "anthropic_messages",
      "open_ai_responses",
      "open_ai_chat"
    ]
  },
  {
    "profile": "zai",
    "provider": "zhipu",
    "regions": [
      "international"
    ],
    "native": "open_ai_chat",
    "formats": [
      "open_ai_chat"
    ]
  }
];
