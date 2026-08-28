# 桌面端 Settings 全面重设计 + 桌面音频能力

日期：2026-08-27
状态：设计定稿，待实施
范围：`clients/electron`、`lingxi-code/client-protocol`、`lingxi-code/engine`、`lingxi-code/apps/engine-desktop`、`lingxi-code/permission`、`lingxi-code/migrations`

本文档含两个可独立交付的部分：

- **Part A —— 设置重设计**：把桌面的设置从「一个真 modal + 一套死 mock」重做成分层感知的全页面设置，并打通引擎设置通道。
- **Part B —— 桌面音频能力**：桌面今天没有任何音频能力，本轮参照 iOS / Android 的既有实现补齐，语音设置页在 Part A 的框架里落一格。

A 不依赖 B。B 依赖 A 的设置页框架。

---

## 0. 现状（全部经过实测，不是推断）

### 0.1 桌面有两套 settings，一真一假

| | 位置 | 状态 |
|---|---|---|
| `BetaSettings` | `clients/electron/src/renderer/components/BetaDesktop.tsx:1843-2130` | **真的**，是 `App.tsx` 唯一挂载的设置界面。modal 形态，5 段：Appearance / Providers / Engine / Diagnostics / About |
| `components/settings/` 8 个文件 | `SettingsPage.tsx` 及 7 个子页 | **死代码**，全仓库无任何 import。全部本地 `useState`，`SettingsBillingPage` 里的三张发票是编的，`SettingsAccountPage` 里的邮箱与 "Max plan" 是硬编码的，Capabilities / Connectors / Cowork / Chrome / Extensions / Developer 六页是 `SettingsGenericPage` 占位符 |

死 mock 来自 `e1c10a2dc`（HTML 原型的直译移植）。

### 0.2 客户端能持久化的设置极少

`clients/electron/src/main/settings.ts` 的 `settings.v1.json` 仅含：`theme` / `model` / `apiBaseUrl` / `projects` / `activeSession` / `pinnedSessions` / workspace trust / `bypassPermissionsModeAccepted`。IPC 的 `updateSettings` patch 类型只有三个字段。

### 0.3 引擎侧已有完整的分层设置模型

- `lingxi-code/engine/src/settings/schema.rs`（1678 行，约 80 字段）：`permissions` / `hooks` / `sandbox` / `enabled_tools` / `output_style` / `status_line` / `enabled_plugins` / `plugin_configs` / `providers` / `routing` / `model_overrides` / `telemetry_enabled` 等。
- 合并优先级（高→低，`engine/src/settings/mod.rs:174`）：**env → managed → cli → local → project → user → defaults**。
- `engine/src/settings/tracer.rs` 已有 `ProvenanceTrace` / `FieldProvenance`，按层记录来源。**provenance 不需要新算，只需要暴露。**
- `apps/engine-desktop/src/settings_watch.rs` 已在 watch 这些文件并触发 `ConfigChange` hook；其模块注释明确写了它**不负责重新加载**设置。

### 0.4 缺的是桥

`client-protocol/src/commands.rs` 的 `ClientCommand` 有 `SetModel` / `SetPermissionMode` / provider 凭据三件套，**没有任何读写引擎设置的命令**。

### 0.5 协议变更代价低于预期

`client-protocol/src/version.rs` 记录了 F1-09 guard 的规则：**新增变体、新增可选字段是 additive，无需 bump**；仅删除 / 改名 / 改类型要 MAJOR。本设计的新增全部是 additive ⇒ `CLIENT_PROTOCOL_VERSION` 保持 `8.0.0`，`snapshots/blessed_major.txt` 无需 re-bless。

### 0.6 写入器已经存在，且不止一个

- `migrations::settings_update::update_settings(path, Vec<(String, Option<Value>)>)` —— `updateSettingsForSource` 的移植。浅层 key 补丁、`None` 即删除、未知键逐字保留、坏 JSON 拒绝覆写。今天的 `SettingsSource` 只有 `User` / `Local` 两个变体。
- `permission/src/persist.rs` —— 完整的权限写入器族：`persist_permission_rule_set` / `remove_permission_update` / `replace_permission_rules` / `persist_permission_mode` / `persist_workspace_directory(_ies)`。带目标层独占锁、原子替换、root-confined no-follow 写、别名规范化去重、未知键保留。
- `cli/src/commands/plugin_settings.rs` —— 插件设置写入器。

### 0.7 内部写标记是硬约束

`permission::{mark_internal_write, consume_internal_write}`（`permission/src/internal_writes.rs`）。`settings_watch.rs:343` 以 5 秒窗口消费该标记，并有测试 `handle_event_suppresses_one_recent_internal_write` 钉住。

**任何新写入器必须在写盘前调用 `mark_internal_write`**，否则桌面自己的每次保存都会被 watcher 当成外部编辑并触发一轮 ConfigChange hook。

### 0.8 权限的实时效果与写盘是两条路

`client-adapter/src/permission_gate.rs:291` —— 用户点「始终允许」时，先 `session_allow_rules.lock().push(rule)`（内存，决定本会话行为），再 best-effort 写盘到 **LocalSettings**（`:301`）。

两个推论：

1. 桌面往 `settings.json` 写一条规则，**运行中的会话不会捡起它**。
2. 普通用户积累的权限规则**全部住在 `settings.local.json`**。若 UI 不把 local 作为可编辑层，权限页会几乎是空的。

### 0.9 MCP 不在 settings.json

`mcp/src/json_config.rs` 的三个入口对应三个存储位置：

| 域 | 位置 |
|---|---|
| User | `~/.lingxi.json` 顶层 `mcpServers` |
| Local | `~/.lingxi.json` 的 `projects[<cwd>]` |
| Project | `<项目>/.mcp.json` |
| Dynamic | 插件注入（`plugin/src/discovery.rs`），只读 |
| Enterprise / Managed | 企业策略，只读 |

另：`migrations/src/migrate_mcp_servers.rs` 把三个 MCP **审批**字段（`enableAllProjectMcpServers` / `enabledMcpjsonServers` / `disabledMcpjsonServers`）从 `~/.lingxi.json` 迁到 `settings.local.json`，但其模块注释写明「No Rust reader consumes these settings keys yet」。**实施时必须先确认审批门当前实际读的是哪一处**，不能假定。

### 0.10 桌面没有任何音频能力

- `engine/src/settings/schema.rs` 中 tts / stt / speech / audio / voice **零命中**。
- `SpeechToText` / `TextToSpeech` / `VoiceRecorder` 的实现者仅：`platforms/ios`、`platforms/android`、`apps/ios-framework`、`apps/android-aar`（含测试假体）。
- `tools/mobile/src/speech.rs` 模块注释：「`None` on desktop; mobile composition roots wire a native Swift / Kotlin impl via UniFFI」。
- `traits/src/platform.rs:62-76` 的 `voice()` / `stt()` / `tts()` 默认返回 `None`；`impl Platform for` 只有 iOS / Android。
- `apps/engine-desktop/src/lib.rs:8827-8829`：`voice: None, stt: None, tts: None` —— **这就是注入点**。
- Electron 客户端里 voice 的全部命中都在死 mock 页内。

---

## 1. 决策记录

| # | 决策 | 取值 |
|---|---|---|
| D1 | 设置里装什么 | 真相优先 **+ 打通引擎设置桥**。删除全部 mock，不保留占位符 |
| D2 | 写入哪一层 | VS Code 式顶部层切换 |
| D3 | 呈现形态 | 全页面接管 + 内部路由 |
| D4 | 入选分区 | 权限、工具与 Agent 行为、Hooks 与插件、原始 JSON 逃生口（四项全选） |
| D5 | 生效语义 | 分类生效 + 显式重启，**不做热重载** |
| D6 | local 层 | 全局三个 tab：用户 / 项目 / 本地 |
| D7 | 音频 | 本轮一并实现，参照 iOS / Android |
| D8 | 视觉形态 | 参照 Codex desktop：搜索框、带图标的分组导航、行分组进卡片 |

---

# Part A —— 设置重设计

## A1. 信息架构

分组依据是**数据归属**，不是话题——这样「这一项现在能不能用」在结构上可推导，不靠文案解释。

```
个人          通用 · 外观 · 语音 · 项目与信任
模型与服务    Provider 凭据 · 自定义 Provider 与路由
编码          权限 · 工具与 Agent 行为 · Skills · MCP 服务器 · Hooks · 插件与市场
高级          原始 JSON · 诊断 · 关于
```

- **`编码` 组顶部有三层 tab（用户 / 项目 / 本地）**，其余组没有。
- `语音` / `项目与信任` / `诊断` / `关于` 住在客户端，不依赖引擎，任何时候可用。
- `MCP 服务器` 在 `编码` 组内，但**不复用层切换器**——它有自己的三域存储模型（见 A5.5）。

### A1.1 视觉形态（参照 Codex desktop）

- 导航顶部一个 **搜索框**。搜索是真功能：`nav.ts` 把 IA 声明成数据，每行的标题 / 描述 / 设置键建索引，命中后跳转到该页并高亮该行。
- 每个导航项带图标；分组标题为小号全大写弱化文本。
- 内容区：页标题 → 分节标题 → **圆角卡片**，行在卡内、以卡内分隔线相隔，卡与卡之间留白。取代现有的平铺行 + 底边框。
- 行 = 左侧标题 + 描述，右侧控件（Toggle / Segmented / Select / 值 + 按钮）。

### A1.2 删除

`clients/electron/src/renderer/components/settings/` 现有 8 个文件整体删除；`BetaDesktop.tsx` 中的 `BetaSettings`（1843-2130，约 290 行）删除。不迁移、不保留占位符。

随之消失的导航条目：Account / Billing / Usage / Capabilities / Connectors / Cowork / Lingxi in Chrome / Extensions / Developer。它们背后没有任何真实数据源。

### A1.3 明确不做（mock 里有但主进程无实现）

开机自启、菜单栏常驻、全局快捷键、防止休眠、聊天字体、Cowork、浏览器连接列表、账单与用量、界面语言。

这些不是「以后补」的占位符，是本次直接从 IA 移除。其中任何一项若要做，它是一个独立的功能需求，不是一个设置项。

---

## A2. 线上契约

全部 additive，`CLIENT_PROTOCOL_VERSION` 保持 `8.0.0`。

### A2.1 命令

```rust
// 通用
GetSettings
UpdateSettings             { destination, patch: Vec<(String, Option<Value>)> }

// 权限（映射到 permission/src/persist.rs 既有写入器）
UpdatePermissionRules      { destination, behavior, add: Vec<String>, remove: Vec<String> }
SetDefaultPermissionMode   { destination, mode }
UpdateWorkspaceDirectories { destination, add: Vec<String>, remove: Vec<String> }

// MCP（自己的存储模型）
ListMcpServers
UpsertMcpServer            { scope, name, config }
RemoveMcpServer            { scope, name }

// Skills（只读 + 动作）
ListSkills
ReloadSkills
```

### A2.2 事件与 DTO

```rust
ClientEvent::SettingsSnapshot(SettingsSnapshotDto)
ClientEvent::SettingsOperationFailed { request_id, message }
ClientEvent::McpServerListing(McpServerListingDto)
ClientEvent::SkillListing(SkillListingDto)

struct SettingsSnapshotDto {
    files:      Vec<SettingsFileDto>,                 // destination / path / exists / writable / parse_error
    effective:  Map<String, Value>,                   // 盘上四层合并的当前结果
    active:     Map<String, Value>,                   // 运行中的会话启动时实际加载的值
    provenance: Map<String, SettingsDestinationDto>,  // 每个键的最终来源（取自 tracer::ProvenanceTrace）
    locked:     Vec<String>,                          // 被 managed 层锁定、不可写的键
}

enum SettingsDestinationDto { Env, Managed, Cli, Local, Project, User, Defaults }
```

`SettingsDestinationDto` 用于**读**（provenance 可能指向任意一层）。全部**写**命令的 `destination` 只接受 `User` / `Project` / `Local` 三者；传入其余值是错误，错误消息须点名收到的值与可写集合。这与 `permission/src/persist.rs` 的 `supportsPersistence` 语义一致。

`effective` 与 `active` 的差集就是「待应用」，见 A4。

### A2.3 三条不变量

**I1 —— 有专用写入器的字段不走通用补丁。**
`UpdateSettings` 显式拒绝 `permissions` 顶层键，错误消息必须**点名** `permissions` 并**点名**替代命令。权限只有一条写入路径：`permission/src/persist.rs`。

唯一例外是高级页的原始 JSON 编辑器——它整层文件覆写，是刻意保留的逃生口，走同一个 `SettingsJson::validate` 与同一个 `mark_internal_write`。

**I2 —— 所有写入先 `permission::mark_internal_write(path)`。**
见 0.7。

**I3 —— UI 只渲染引擎回传的快照。**
写入成功后引擎重读并发一个新的 `SettingsSnapshot`；前端**不做乐观更新、不本地拼值**。界面上显示的永远是引擎重新读出来的那个值。

### A2.4 复用与最小改动

- 通用补丁复用 `migrations::settings_update::update_settings`，只给 `SettingsSource` 补一个 `Project` 变体与一条路径分支（`<project>/.lingxi/settings.json`）。
- provenance 直接来自 `settings/tracer.rs` 已有的 `ProvenanceTrace`，不新增计算。

### A2.5 顺手修掉的既有缺陷

`AllowedClientCommand` 允许清单有**两份拷贝**且已经漂了：`clients/electron/src/preload/index.ts` 那份缺 `get_conversation_controls` / `set_reasoning_selection` / `set_fast_mode`，`src/renderer/bridge/lingxi.d.ts` 那份有。

收敛为 `clients/electron/src/shared/` 下的单一来源，两侧引用。加新命令时只改一处。

---

## A3. 分层与 provenance 的交互语义

**user 是文件层里最低的一层**（0.3 的优先级顺序）。所以「我在用户层改了但生效值来自项目层」是常态而非边缘情况，必须是一等状态而不是 tooltip。

### A3.1 行状态机（六态，由引擎快照推导，前端不猜）

| 状态 | 呈现 |
|---|---|
| 未设置 | 显示默认值，弱化 |
| 在当前编辑层设置且生效 | 正常值 + 归属徽标 |
| **在当前编辑层设置但被更高层覆盖** | 值旁警示「已写入用户层；当前生效值来自项目层」+ 一键跳到该层 |
| 未在当前层设置，继承自其他层 | 显示生效值 + 来源徽标，控件可写（写入即产生覆盖） |
| 被 managed 锁定 | 控件禁用 + `策略` 徽标 |
| 该层文件解析失败 | 整层降级只读 + 指向高级页修复（写入器本来就拒绝覆写坏文件） |

### A3.2 归属徽标是全局概念

`设备` / `用户` / `项目` / `本地` / `策略`。

住在 Electron store 里的行（如 `bypassPermissionsModeAccepted`）标 `设备`，切换层不影响它。这让层切换器不必解释自己。

### A3.3 无项目时

`项目` 与 `本地` tab 禁用并说明原因（「打开一个项目后可编辑」），不是显示一个空表。

---

## A4. 生效与重启

引擎在会话启动时加载一次设置；`settings_watch.rs` 不负责重载（0.3）；权限的实时效果来自内存的 `session_allow_rules`（0.8）。所以「写完即生效」在引擎侧不成立。

**不猜，用差集。** `SettingsSnapshotDto` 同时给出 `effective`（盘上当前）与 `active`（运行中会话启动时加载的）：

- 「待应用」= 两者的实际差集。**不是**前端记账「用户刚点过保存」——那种记账在引擎重启后、或用户在终端改了文件后会撒谎。
- 差集为空 ⇒ 页面上不出现任何提示。
- 某行真的不同 ⇒ 该行标「已保存，当前会话仍在用旧值」，页顶出现 `重启引擎以应用（N 项）`。

分类：

| 类别 | 生效方式 |
|---|---|
| 外观、项目增删与切换、置顶会话、诊断动作、语音偏好 | 即时（住在客户端） |
| Provider 凭据 | 沿用既有语义：持久化是 IPC 的边界，重启由渲染进程拥有（`host.ts:283` 注释），因为要先清内存密钥再处理恢复。不动 |
| 全部引擎设置项 | 需重启。复用 `restartBridge(sessionId)`；`assertNoActiveTurn` 已存在 ⇒ turn 进行中按钮禁用并说明原因，不静默失败 |

---

## A5. 各页内容

本节只写需要说明的页。未列出的页内容已由现有实现决定：**诊断**沿用现有的日志流 / 复制报告 / 导出 JSON / 刷新 / 重启引擎；**关于**显示应用、Electron、引擎三个版本号；**语音**见 Part B。

### A5.1 通用

`通用` 与 `外观` 是两个独立页（对齐 Codex 的形态），不是一页的两节。

- `通用`：本页在本轮只承载跨页的杂项与入口，不新增未实现的开关（见 A1.3）。
- `外观`：深色 / 浅色 / **跟随系统**。`PersistedSettings.theme` 从 `'dark'|'light'` 扩到含 `'system'`，配 Electron `nativeTheme` 监听。**这是本设计中唯一一处客户端 store schema 变更。**

### A5.2 项目与信任

项目增删与切换、workspace trust 与指纹、置顶会话。全部已有 IPC。

### A5.3 Provider 凭据

沿用现有能力：内置 provider 的连接 / 替换 / 断开、Keychain 可用性状态、`runtimeOnly` 状态、`apiBaseUrl`。保住 Composer 的深链（`SettingsRoute { providerId, pendingModelReference, restoreFocus }`）。

### A5.4 自定义 Provider 与路由（新）

`settings.providers`：key 为 profile 名，值为
`{ type, baseUrl, apiKeyEnv, models: [{ id, aliases?, capabilities? }] }`。
支持的 `type`：`openai` / `openai-responses` / `anthropic` / `gemini` / `azure-openai` / `bedrock-claude` / `vertex-claude` / `vertex-gemini` / `foundry-claude`（`llm-client/src/provider_settings.rs`）。

`settings.routing`：`{ aliases, fallback, retry: { maxAttempts, backoffMs } }`，schema 注释标注全部 WIRED。

> **写入前必须做的校验**：schema 注释明确「`models` is REQUIRED per entry; an absent or empty list is an error at engine startup」。UI 必须在写盘**前**拒绝空 models 列表——否则一次保存会让引擎下次启动失败。这条校验要有专门的测试，且测试必须先证明自己能红。

### A5.5 MCP 服务器（新）

三个可写域（User / Local / Project）+ 两个只读域（插件注入的 Dynamic、企业策略的 Enterprise）。存储位置见 0.9。

页内自带域选择器，**不复用 `编码` 组的层切换器**——存储模型不同，共用会撒谎。

每个 server 显示：名称、域、传输方式、命令 / URL、`alwaysLoad`、超时。只读域的条目禁用编辑并标注来源。

实施前置：先确认 MCP 审批字段当前实际被谁读（0.9 末段），再决定审批开关放哪里。

### A5.6 Skills（只读 + 动作）

Skills 是目录发现制（`skill-api/src/registry.rs`），设置里只有 `sync_claude_ai_skills` 一个开关——**没有「每个 skill 的开关」可配**。

本页是查看页：列出已发现的 skills（名称、来源目录、来自哪个插件）、`sync_claude_ai_skills` 开关、`ReloadSkills` 动作、skill-doctor 诊断入口。

不要设计成配置页。

本页位于 `编码` 组，因此顶部有层切换器，但**它只作用于 `sync_claude_ai_skills` 这一行**；skills 列表本身是目录发现的结果，与层无关。列表区需明确标注这一点，避免让人以为切层会换一批 skills。

### A5.7 权限

- allow / ask / deny 三张规则表，增删改，带 pattern 语义提示。
- 默认权限模式。
- `additionalDirectories`。
- `bypassPermissionsModeAccepted`（`设备` 归属）。

全部走 `permission/src/persist.rs` 的既有写入器（I1）。

### A5.8 工具与 Agent 行为

`enabled_tools`、各 `disable_*`（`disable_all_hooks` / `skip_web_fetch_preflight` / `disable_agent_view` 等）、`output_style`、`model_overrides`、`always_thinking_enabled`、`show_thinking_summaries`、`auto_continue_at_usage_limit`、`vision_delegation_enabled`、workflow 相关开关。走通用 `UpdateSettings`。

### A5.9 Hooks

**只读展示 + 「在 JSON 中编辑」跳转**。不做结构化嵌套编辑器——`hooks` 是嵌套 JSON，结构化 UI 成本高且收益不明。

### A5.10 插件与市场

`enabled_plugins` / `plugin_configs` / `additional_marketplaces` / `allowed_marketplaces` / `blocked_marketplaces`。复用 `cli/src/commands/plugin_settings.rs` 的既有写入器。

### A5.11 高级 / 原始 JSON

当前层的原文编辑器，带校验与错误提示；四个层文件的路径、存在性、可写性、解析状态一览。是所有未入 UI 字段的兵库。

---

## A6. 文件结构

```
clients/electron/src/renderer/components/settings/
  SettingsScreen.tsx      全页壳 + 内部路由 + 搜索 + 层切换器
  nav.ts                  IA 声明：分组、页、图标、是否依赖引擎、是否分层、可搜索键
  rows.tsx                Card / Row / BlockRow / ProvenanceBadge / LockedBadge / OverriddenNotice
  useEngineSettings.ts    快照订阅、写入动作、六态推导
  pages/
    General.tsx  Appearance.tsx  Voice.tsx  Projects.tsx
    ProviderCredentials.tsx  CustomProviders.tsx
    Permissions.tsx  ToolsAgent.tsx  Skills.tsx  McpServers.tsx  Hooks.tsx  Plugins.tsx
    RawJson.tsx  Diagnostics.tsx  About.tsx
```

`nav.ts` 把「这页需不需要引擎」「分不分层」变成数据而非散落的条件判断——空状态、tab 启停、搜索索引都由它统一推导。

其他改动：

- `BetaDesktop.tsx` 从 2137 行降到约 1850 行。
- `App.tsx` 改挂 `SettingsScreen`，保持 `SettingsRoute`、`SettingsBackground` 的 inert 处理、Composer 深链回焦不变。
- `src/main/settings.ts`：`theme` 扩到含 `'system'`。
- `src/main/bridge.ts` / `host.ts`：转发新命令。
- `src/shared/`：`AllowedClientCommand` 单一来源（A2.5）。

---

# Part B —— 桌面音频能力

## B1. 分工：照搬移动端的边界，不照搬它的技术栈

移动端的模式是**客户端用原生 API 实现能力，通过 UniFFI 把 `Arc<dyn>` 注入引擎**。

桌面的忠实镜像是**Electron 实现能力，通过桥注入 engine-desktop**：engine-desktop 侧放三个代理实现 `BridgeVoiceRecorder` / `BridgeStt` / `BridgeTts`，把 trait 调用变成对客户端的请求。引擎侧不引入任何音频依赖，边界与移动端同构。

注入点：`apps/engine-desktop/src/lib.rs:8827-8829` 的三行 `None`。

trait 形状（`traits/src/{stt,tts,voice}.rs`）：

```rust
trait SpeechToText { async fn transcribe(&self, opts: SttOpts) -> Result<SttTranscript, SttError>; }
trait TextToSpeech { async fn synthesize(&self, opts: TtsOpts) -> Result<TtsAudio, TtsError>; }
trait VoiceRecorder {
    async fn start_recording(&self, opts: VoiceRecordingOpts) -> Result<(), VoiceError>;
    async fn stop_recording(&self) -> Result<VoiceRecording, VoiceError>;
    async fn is_recording(&self) -> bool;
}
```

## B2. 偏好模型：直接镜像，一个字段都不改

iOS 的 `VoicePreferencesSnapshot`（`clients/ios/Sources/Voice/VoiceRuntimeConfiguration.swift`）与 Android 的 `VoiceConfig` 已经是同一套：

| 字段 | 取值 |
|---|---|
| `schemaVersion` | 2 |
| `recognitionMode` | `automatic` / `onDevice` |
| `language` | `"auto"` 或 BCP-47 tag |
| `voiceSelection` | `system:<id>` / `sherpa:<modelId>:<voiceId>` / `system:default` |
| `rate` | 0.5 – 2.0 |
| `autoPlayReplies` | bool |

桌面沿用**同样的键与同样的选择串语法**，三端一致。存在 Electron 的 `settings.v1.json` 里（`设备` 归属，不分层）。

## B3. 能力快照：同样镜像

Android 的 `VoiceCapabilitySnapshot`（`clients/android/.../settings/VoiceSettingsCapabilities.kt`）已经把 requested 与 effective 分开，并带 `blockingIssues` 与 `fallbackReason`。这与 A4 的 effective/active 是同一个思路。

桌面复用该结构：`microphonePermission` / `platformRecognizerAvailable` / `requestedRecognitionBackend` / `effectiveRecognitionBackend` / `effectiveLanguage` / `voiceOptions` / `requestedVoice` / `effectiveVoice` / `blockingIssues` / `fallbackReason`。

设置页显示的是**探测出来的真实能力**，不是一个静态下拉。

## B4. 三个后端的桌面落点

| 能力 | 实现 | 状态 |
|---|---|---|
| 麦克风采集 | 渲染进程 `getUserMedia` + `MediaRecorder` | 可用 |
| TTS | 渲染进程 `speechSynthesis` | 可用；枚举出的就是 macOS 系统嗓音，天然匹配 `system:<id>` 语法 |
| STT | **走已配置 provider 的转写接口**（桌面本来就存着 provider 凭据） | Chromium 的 `SpeechRecognition` 在 Electron 中不可用（依赖 Google 服务与密钥） |

因此桌面 v1：

- `recognitionMode: automatic` = provider 转写。
- `recognitionMode: onDevice` **诚实地不可用**，落进移动端模型里已有的 `Unavailable` 槽位并给出 `fallbackReason`。
- **离线 Sherpa 不进 v1**：Rust 侧无绑定（全仓库唯一 sherpa 命中是一个无关的 parity 测试文件名），要做等于移植模型目录 + 下载器 + onnx 运行时，是独立一轮。

麦克风系统权限：macOS 的 TCC 提示由 Electron 触发；`Info.plist` 需 `NSMicrophoneUsageDescription`。

## B5. 工具注册

`speech` / `voice` 两个工具目前由 `tools/mobile` 的 `register_builtin` 注册（`speech.rs:205`、`voice.rs:175`）。桌面在能力存在时才注册它们。

crate 名 `tools/mobile` 从此不再准确，**但不改名**——最小改动优于顺手重构。在该 crate 的模块注释中记一句即可。

---

# 测试与验收

本次改动有两个高危特征：**判据容易恒真**，且**全绿不等于上过机**。测试策略据此加严。

## T1. 引擎侧

- `update_settings` 新增的 `Project` 分支：断言写入路径的**具体字符串**（`<project>/.lingxi/settings.json`），不是断言「写成功了」。
- `UpdateSettings` 拒绝 `permissions` 顶层键：断言错误消息**点名** `permissions` 且**点名**替代命令名。**先跑一个不含拒绝逻辑的反向用例，证明这条断言能红。**
- 内部写标记：每个写入器写完后 `consume_internal_write(path)` 必须为 true。**配反向用例**——不调 `mark_internal_write` 时该断言必须失败，否则这个门恒绿。
- provenance：造一个四层同时定义同一个键的 fixture，断言 `provenance[key]` 等于**合并结果实际来自的那层**（按 0.3 的优先级推出来对照），而不是等于一个硬编码字符串。
- 自定义 provider 的空 `models` 校验：断言写盘被拒绝且错误点名 `models`；先证明该断言能红。

## T2. `active` vs `effective`（最容易假绿，单列）

测试必须让两者**真的不同**——改盘上文件而不重启会话——断言推导出「待应用」。**并配一个 A/B**：改成相同则不得出现待应用。两个方向都测到，这个判据才有意义。

## T3. 前端

- `useEngineSettings` 的六态做穷举表测试（未设置 / 本层设置生效 / 本层设置被覆盖 / 继承 / 策略锁定 / 文件损坏）。
- 搜索索引：断言每个 nav 项与每个可搜索行都进了索引，且一次已知查询命中预期的页与行。

## T4. 死代码防漏门

断言全仓库 0 处 import `SettingsGenericPage`、0 处引用 `BetaSettings`。

**0-hit grep 本身不构成证据**——这个门必须先用一个已知样本证明它能命中，否则只是恒绿的装饰。

## T5. 音频

- `BridgeStt` / `BridgeTts` / `BridgeVoiceRecorder` 的往返测试用假客户端。
- 能力快照解析器的表驱动测试：无麦克风权限、无 provider 凭据、`onDevice` 模式 —— 各自产出预期的 `blockingIssues` 与 `fallbackReason`。
- `voiceSelection` 语法解析与 iOS / Android 的实现对齐（同样的输入产出同样的解析结果）。

## T6. 真机验收清单（不可省）

单测与评审抓不到「装起来根本没跑对」。最小验收动作：

1. 启动 Electron，打开设置。
2. 在**权限**页往**项目层**加一条规则；退出应用；`cat <project>/.lingxi/settings.json` 确认规则在里面，且其他键未被改动。
3. 重启应用，确认该规则真的生效，且页面上「待应用」提示已消失。
4. 在**用户层**设一个已被项目层定义的键，确认出现「已写入用户层；当前生效值来自项目层」而不是静默无变化。
5. 手动把某层文件改成坏 JSON，确认该层降级只读且写入被拒绝，原文件未被覆写。
6. 打开**语音**页，确认麦克风权限状态、可用嗓音列表来自系统而非硬编码；录一段并确认转写返回。
7. 断开全部 provider 凭据，确认语音页的 STT 落进 `blockingIssues` 而不是静默失败。

---

# 已知风险

| 风险 | 处置 |
|---|---|
| MCP 审批字段的读取方未确认（0.9） | 实施前先定位实际读者，再决定审批开关的归属；不得假定 |
| 自定义 provider 的空 `models` 会让引擎下次启动失败 | 写盘前校验 + 专门测试（T1） |
| 桌面 STT 依赖 provider 凭据，无凭据时不可用 | 落进 `blockingIssues`，不静默失败（T5 / T6.7） |
| 层覆盖导致「改了没反应」 | 六态机的第三态是一等状态（A3.1） |
| 桌面自身写盘触发 ConfigChange hook 风暴 | I2 + T1 的反向用例 |
