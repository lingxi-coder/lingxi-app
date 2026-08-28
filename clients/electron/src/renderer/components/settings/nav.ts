export interface NavPage {
  id: string;
  label: string;
  group: '个人' | '模型与服务' | '编码' | '高级';
  icon: string;
  /** 该页是否依赖运行中的引擎。false 的页在无引擎时仍完全可用。 */
  needsEngine: boolean;
  /** 该页是否受顶部的用户/项目/本地层切换器支配。 */
  layered: boolean;
  /** 该页拥有的设置键与关键词，供搜索建索引。 */
  searchKeys: string[];
  /** 该页的组件是否已经存在。声明了但未建的页渲染一个明确的占位，
   *  而不是空白屏幕。 */
  implemented: boolean;
}

export const SETTINGS_NAV: NavPage[] = [
  { id: 'general', label: '通用', group: '个人', icon: 'cog', needsEngine: false, layered: false, searchKeys: ['general', '通用'], implemented: true },
  { id: 'appearance', label: '外观', group: '个人', icon: 'sun', needsEngine: false, layered: false, searchKeys: ['theme', '主题', '外观', 'appearance', 'dark', 'light', 'system'], implemented: true },
  { id: 'voice', label: '语音', group: '个人', icon: 'mic', needsEngine: false, layered: false, searchKeys: ['voice', '语音', 'tts', 'stt', 'rate', 'autoPlayReplies'], implemented: false },
  { id: 'projects', label: '项目与信任', group: '个人', icon: 'folder', needsEngine: false, layered: false, searchKeys: ['project', '项目', 'trust', '信任', 'pinned'], implemented: true },

  { id: 'provider-credentials', label: 'Provider 凭据', group: '模型与服务', icon: 'key', needsEngine: true, layered: false, searchKeys: ['provider', 'credential', '凭据', 'apiBaseUrl', 'keychain'], implemented: true },
  { id: 'custom-providers', label: '自定义 Provider 与路由', group: '模型与服务', icon: 'plug', needsEngine: true, layered: false, searchKeys: ['providers', 'routing', 'baseUrl', 'apiKeyEnv', 'models', 'aliases', 'fallback', 'retry'], implemented: true },

  { id: 'permissions', label: '权限', group: '编码', icon: 'shield', needsEngine: true, layered: true, searchKeys: ['permissions', '权限', 'allow', 'deny', 'ask', 'additionalDirectories', 'bypassPermissions'], implemented: true },
  { id: 'tools-agent', label: '工具与 Agent 行为', group: '编码', icon: 'sliders', needsEngine: true, layered: true, searchKeys: ['enabledTools', 'outputStyle', 'modelOverrides', 'alwaysThinkingEnabled', 'showThinkingSummaries', 'visionDelegationEnabled', 'disableAllHooks', 'skipWebFetchPreflight'], implemented: true },
  { id: 'skills', label: 'Skills', group: '编码', icon: 'sparkle', needsEngine: true, layered: true, searchKeys: ['skills', 'syncClaudeAiSkills', 'reload'], implemented: true },
  { id: 'mcp', label: 'MCP 服务器', group: '编码', icon: 'server', needsEngine: true, layered: false, searchKeys: ['mcp', 'mcpServers', 'server', '服务器'], implemented: true },
  { id: 'hooks', label: 'Hooks', group: '编码', icon: 'anchor', needsEngine: true, layered: true, searchKeys: ['hooks', 'ConfigChange', 'PreToolUse', 'PostToolUse'], implemented: true },
  { id: 'plugins', label: '插件与市场', group: '编码', icon: 'puzzle', needsEngine: true, layered: true, searchKeys: ['enabledPlugins', 'pluginConfigs', 'marketplace', '插件', '市场'], implemented: true },

  { id: 'raw-json', label: '原始 JSON', group: '高级', icon: 'braces', needsEngine: true, layered: false, searchKeys: ['json', 'raw', '原始', 'settings.json'], implemented: true },
  { id: 'diagnostics', label: '诊断', group: '高级', icon: 'activity', needsEngine: false, layered: false, searchKeys: ['diagnostics', '诊断', 'log', '日志', 'export'], implemented: true },
  { id: 'about', label: '关于', group: '高级', icon: 'info', needsEngine: false, layered: false, searchKeys: ['about', '关于', 'version', '版本'], implemented: true },
];

/** 按标签与设置键做大小写不敏感的子串匹配。 */
export function searchNav(query: string): NavPage[] {
  const needle = query.trim().toLowerCase();
  if (!needle) return [];
  return SETTINGS_NAV.filter((page) =>
    page.label.toLowerCase().includes(needle) ||
    page.id.includes(needle) ||
    page.searchKeys.some((key) => key.toLowerCase().includes(needle)));
}
