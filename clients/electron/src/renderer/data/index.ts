// ─── MOCK DATA ──────────────────────────────────────────────
// Ported verbatim from the design prototype. The desktop shell renders this
// static mock data; there is no backend wiring (a later milestone).

export interface Session {
  id: string;
  title: string;
  activity?: string;
  kbd?: string;
  active?: boolean;
  hidden?: boolean;
}

export interface Project {
  id: string;
  name: string;
  scope: string;
  branch: string;
  dirty?: boolean;
  diff?: { add: number; del: number };
  sessions: Session[];
}

// Projects = repos, each with a list of recent coding sessions
export const PROJECTS: Project[] = [
  {
    id: 'mlplatform', name: 'MLPlatform', scope: 'axielix', branch: 'main',
    sessions: [
      { id: 'mlp-1', title: 'Mediapipe', activity: '', kbd: '⌘1' },
    ],
  },
  {
    id: 'agai', name: 'agai_inspection', scope: 'axielix', branch: 'main',
    sessions: [
      { id: 'ag-1', title: '调整未打卡首页布局', kbd: '⌘2' },
      { id: 'ag-2', title: '分析宇宙起源答答正确性', kbd: '⌘3' },
      { id: 'ag-3', title: 'Rust 代码以 iOS/Android 调用', kbd: '⌘4' },
      { id: 'ag-4', title: '更新打卡与 feature 配置逻辑', kbd: '⌘5' },
      { id: 'ag-5', title: '修复 TCP H264/H265 延迟', kbd: '⌘6' },
      { id: 'ag-6', title: '环境变量读取 fallback', hidden: true },
      { id: 'ag-7', title: '进程权限重试', hidden: true },
    ],
  },
  {
    id: 'visionx', name: 'visionx-android', scope: 'axielix', branch: 'main',
    sessions: [],
  },
  {
    id: 'lingxi-next', name: 'LingXi-Next', scope: 'axielix', branch: 'main', dirty: true, diff: { add: 287, del: 0 },
    sessions: [
      { id: 'lxn-1', title: 'Review lingxi core rust engine', activity: '2h', active: true },
    ],
  },
  {
    id: 'lingxi', name: 'lingxi', scope: 'axielix', branch: 'main',
    sessions: [
      { id: 'lx-1', title: '添加 Android 特有技能', activity: '23h' },
      { id: 'lx-2', title: '调研 harness agent 记忆压缩', activity: '3d' },
      { id: 'lx-3', title: '对齐 iOS 和 Android 项目', activity: '1w' },
    ],
  },
  {
    id: 'telegram', name: 'Telegram', scope: 'tg-mirror', branch: 'master',
    sessions: [
      { id: 'tg-1', title: '币安 c2c 购买 usdt 后 提币金额…', activity: '1w' },
      { id: 'tg-2', title: 'iOS', activity: '2w' },
      { id: 'tg-3', title: 'Android', activity: '3w' },
    ],
  },
];

// agent run = the "Ran an agent" receipt cards in the screenshot
// types: dispatch (heading), narration (plain text), agent (card), thinking
export type RunItem =
  | { type: 'narration'; text: string; tone?: 'muted'; strong?: boolean }
  | { type: 'agent'; state: 'done' | 'running'; title: string; sub?: string; expandable?: boolean; link?: boolean }
  | { type: 'meta'; dur: string; tokens: string }
  | { type: 'audio'; bars: number[]; duration: number }
  // The assistant's streamed reasoning (`thinking_delta`), rendered as a dim,
  // italic, collapsible block above the answer. `done` flips true once the
  // reasoning stream closes (message_complete / turn_ended).
  | { type: 'thinking'; text: string; done?: boolean };

export const RUN: RunItem[] = [
  { type: 'narration', text: 'Dispatching Task 1: Bootstrap workspace', tone: 'muted' },
  { type: 'agent', state: 'done', title: 'Ran an agent', expandable: true },
  { type: 'narration', text: 'Task 1 implementer DONE. Dispatching spec compliance reviewer.' },
  { type: 'agent', state: 'done', title: 'Ran agent', sub: 'Spec review Task 1' },
  { type: 'narration', text: 'Spec compliance ✓. Dispatching code quality reviewer.' },
  { type: 'agent', state: 'done', title: 'Ran agent', sub: 'Code quality review Task 1' },
  { type: 'narration', text: 'Task 1 complete: ✓ Spec compliant + ✓ Code quality approved.', strong: true },

  { type: 'narration', text: 'Dispatching Task 2: Create lingxi-protocol crate skeleton', tone: 'muted' },
  { type: 'agent', state: 'done', title: 'Ran an agent', expandable: true },
  { type: 'narration', text: "Task 2 implementer DONE_WITH_CONCERNS — concern is about plan-sequencing (the validation command can't run until all 5 members have Cargo.tomls due to Cargo's workspace resolution). The files themselves match spec; concern is an observation about the plan, not a correctness issue. The implementer verified files are syntactically valid by isolating them in /tmp. Proceeding to spec review." },
  { type: 'agent', state: 'done', title: 'Ran agent', sub: 'Spec review Task 2' },
  { type: 'narration', text: 'Spec compliance ✓. Dispatching code quality reviewer for Task 2.' },
  { type: 'agent', state: 'done', title: 'Ran an agent', expandable: true },
  { type: 'narration', text: 'Task 2 ✓ complete. Moving to Task 3.', strong: true },

  { type: 'narration', text: 'Dispatching Task 3: IDs (newtype wrappers)', tone: 'muted' },
  { type: 'agent', state: 'running', title: 'Running agent', sub: 'Implement Task 3: ID newtypes', link: true },

  { type: 'meta', dur: '13m 54s', tokens: '281.8k tokens' },
];

export interface FileChange {
  path: string;
  add: number;
  del: number;
  status: 'A' | 'M' | 'D';
}

export const FILES_CHANGED: FileChange[] = [
  { path: 'Cargo.toml', add: 18, del: 0, status: 'M' },
  { path: 'crates/lingxi-core/Cargo.toml', add: 24, del: 0, status: 'A' },
  { path: 'crates/lingxi-core/src/lib.rs', add: 42, del: 0, status: 'A' },
  { path: 'crates/lingxi-protocol/Cargo.toml', add: 19, del: 0, status: 'A' },
  { path: 'crates/lingxi-protocol/src/lib.rs', add: 8, del: 0, status: 'A' },
  { path: 'crates/lingxi-protocol/src/agent.rs', add: 64, del: 0, status: 'A' },
  { path: 'crates/lingxi-protocol/src/codec.rs', add: 38, del: 0, status: 'A' },
  { path: 'crates/lingxi-store/Cargo.toml', add: 21, del: 0, status: 'A' },
  { path: 'crates/lingxi-runtime/Cargo.toml', add: 22, del: 0, status: 'A' },
  { path: 'crates/lingxi-cli/Cargo.toml', add: 17, del: 0, status: 'A' },
  { path: 'README.md', add: 14, del: 0, status: 'M' },
];

export interface SlashCommand {
  cmd: string;
  desc: string;
  sub?: string;
  hint?: string;
}

export const SLASH_COMMANDS: SlashCommand[] = [
  { cmd: '/compact', desc: '压缩上下文 · 把过往消息折叠为摘要', sub: 'Used' },
  { cmd: '/plan', desc: '规划：把任务拆解成可调度子任务', hint: 'Plan' },
  { cmd: '/review', desc: '让 reviewer agent 审一遍 diff', hint: 'QA' },
  { cmd: '/run', desc: '在沙箱里跑一条 shell / cargo / npm 命令', hint: 'Exec' },
  { cmd: '/branch', desc: '切换或新建分支', hint: 'Git' },
  { cmd: '/explain', desc: '解释某个文件或函数', hint: 'Read' },
];

export interface Model {
  id: string;
  name: string;
  variant?: string;
  tag: string;
  desc: string;
  color: string;
}

export const MODELS: Model[] = [
  { id: 'lx-47', name: 'Lingxi 4.7', tag: '', desc: '默认 · 200K 上下文', color: 'oklch(72% 0.18 268)' },
  { id: 'lx-47-1m', name: 'Lingxi 4.7', variant: '1M', tag: 'Max', desc: '深度推理 · 100万 token 上下文', color: 'oklch(72% 0.18 268)' },
  { id: 'lx-46s', name: 'Sonata 4.6', tag: '', desc: '快速响应 · 日常交互', color: 'oklch(74% 0.16 195)' },
  { id: 'lx-45h', name: 'Mini 4.5', tag: '', desc: '极速 · 轻量调用', color: 'oklch(74% 0.16 155)' },
  { id: 'lx-46l', name: 'Lingxi 4.6', variant: 'Legacy', tag: '', desc: '上一代 · 兼容性保留', color: 'oklch(60% 0.04 270)' },
];

export interface Effort {
  id: string;
  label: string;
  desc: string;
}

export const EFFORTS: Effort[] = [
  { id: 'low', label: 'Low', desc: '一次调用·不多推理' },
  { id: 'medium', label: 'Medium', desc: '默认 · 平衡费用与准确性' },
  { id: 'high', label: 'High', desc: '反复检查 · 适合理解复杂代码' },
  { id: 'xhigh', label: 'Extra high', desc: '多路探索 · 取最佳' },
  { id: 'max', label: 'Max', desc: '拼一次 · 限时任务使用' },
];

export interface PlanUsage {
  label: string;
  used: number;
  reset: string;
  color: 'accent' | 'accent2' | 'accent3';
}

export const PLAN_USAGE: PlanUsage[] = [
  { label: '5-hour limit', used: 22, reset: 'resets 2h', color: 'accent' },
  { label: 'Weekly · all models', used: 31, reset: 'resets 5d', color: 'accent' },
  { label: 'Weekly · Lingxi 4.7', used: 4, reset: 'resets 5d', color: 'accent2' },
  { label: 'Sonata only', used: 6, reset: 'resets 5d', color: 'accent3' },
];

// ─── BACKGROUND TASKS ──────────────────────────────────────
export interface BgTask {
  id: string;
  title: string;
  kind?: string;
  dur: string | null;
  tokens: string | null;
  tools: number | null;
  tool?: string;
}

export const BG_TASKS_RUNNING: BgTask[] = [
  { id: 'r1', title: 'Write Plan M2-02 (jsonrpc + MCP + bridge)', dur: '11m 45s', tokens: '137.0k', tools: 34, tool: 'Bash' },
  { id: 'r2', title: 'Write Plan M2-06 (SecureStorage + SSE + Process)', dur: '8m 26s', tokens: '96.8k', tools: 38, tool: 'Bash' },
  { id: 'r3', title: 'Write Plan M2-07 (tests + docs + release)', dur: '7m 31s', tokens: '91.8k', tools: 20, tool: 'Bash' },
];

export const BG_TASKS_FINISHED: BgTask[] = [
  { id: 'f1', title: 'Write Plan M2-04 (Sandbox runtime)', kind: 'Agent', dur: '10m 12s', tokens: '160.7k', tools: 31 },
  { id: 'f2', title: 'Write Plan M2-05 (FS watch + Swarm)', kind: 'Agent', dur: '9m 29s', tokens: '135.5k', tools: 35 },
  { id: 'f3', title: 'Write Plan M2-03 (LSP)', kind: 'Agent', dur: '8m 49s', tokens: '160.8k', tools: 33 },
  { id: 'f4', title: 'Write Plan M2-01 (corrections)', kind: 'Agent', dur: '8m 8s', tokens: '147.1k', tools: 40 },
  { id: 'f5', title: 'Find writing-plans skill location', kind: 'Bash', dur: null, tokens: null, tools: null },
  { id: 'f6', title: "Verify Codex's 6 spec corrections against claude-code source", kind: 'Agent', dur: '2m 23s', tokens: '62.1k', tools: 32 },
  { id: 'f7', title: 'T1 steps 1–3: scaffold dir, install graphify', kind: 'Bash', dur: null, tokens: null, tools: null },
];

// Mock transcript steps for the detail view
export type TranscriptStepData =
  | { kind: 'tool'; label: string; target?: string; expandable?: boolean; expanded?: boolean }
  | { kind: 'bash'; cmd: string; out: string[] };

export const TRANSCRIPT_DEMO: TranscriptStepData[] = [
  { kind: 'tool', label: 'Ran Check McpTransportSpec variants exa…', expandable: true },
  { kind: 'tool', label: 'Read', target: 'mcp.rs', expandable: true },
  { kind: 'tool', label: 'Read', target: 'Cargo.toml', expandable: true },
  { kind: 'tool', label: 'Read', target: 'Cargo.toml', expandable: true },
  { kind: 'tool', label: 'Ran List existing plans', expanded: true },
  {
    kind: 'bash',
    cmd: 'ls /Users/luolingfeng/Projects/LingXi-Next/docs/superpowers/plans/ 2>/dev/null | head -20',
    out: [
      '2026-05-21-claw-code-staged-graphify.md',
      '2026-05-22-lingxi-core-m1-01-foundation.md',
      '2026-05-22-lingxi-core-m1-02-security-cost.md',
      '2026-05-22-lingxi-core-m1-03-tools-hooks.md',
      '2026-05-22-lingxi-core-m1-04-sessions.md',
      '2026-05-22-lingxi-core-m1-05-cli-shell.md',
      '2026-05-22-lingxi-core-m2-01-jsonrpc.md',
    ],
  },
  { kind: 'tool', label: 'Read', target: 'transport.rs', expandable: true },
  { kind: 'tool', label: 'Edit', target: 'transport.rs', expandable: true },
];

// ─── PERMISSION MODES ──────────────────────────────────────
export interface PermMode {
  id: string;
  label: string;
}

export const PERM_MODES: PermMode[] = [
  { id: 'ask', label: 'Ask permissions' },
  { id: 'accept', label: 'Accept edits' },
  { id: 'plan', label: 'Plan mode' },
  { id: 'auto', label: 'Auto mode' },
  { id: 'bypass', label: 'Bypass permissions' },
];

export const PERM_DEFAULT = 'bypass';
