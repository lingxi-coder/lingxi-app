import type { DocPage, DocText } from './types';

const t = (en: string, zh: string): DocText => ({ en, zh });
const sources = (...paths: string[]) => paths.map((path) => ({ repo: 'harness-runtime' as const, path }));
const sourceUrl = 'https://github.com/lingxi-coder/harness-runtime';

export const runtimeSystemPages: DocPage[] = [
  {
    id: 'agent-execution', group: 'system', title: t('Agent execution and recovery', 'Agent 执行与恢复'),
    description: t('Follow one admitted turn from host input to model requests, streamed tool execution and a durable terminal state.', '追踪已准入轮次从宿主输入，到模型请求、流式工具执行及持久化终态的全过程。'),
    sourceUrl,
    sources: sources('crates/runtime/src/api.rs', 'crates/runtime/src/desktop/sdk.rs', 'crates/orchestrator/src/conversation/drivers/prepare.rs', 'crates/orchestrator/src/conversation/drivers/streaming.rs', 'crates/orchestrator/src/conversation/prompt.rs', 'crates/orchestrator/src/streaming_loop.rs', 'crates/orchestrator/src/streaming_executor.rs', 'crates/orchestrator/src/turn_loop/tool_dispatch.rs', 'crates/orchestrator/src/turn_loop/api_recovery.rs', 'crates/orchestrator/src/provider_adapter.rs', 'crates/llm-runtime/src/model/retry.rs', 'crates/llm-runtime/src/model/retry_scope.rs'),
    sections: [
      {
        id: 'owners', title: t('Execution owners', '执行负责人'),
        paragraphs: [
          t('The host admits work and owns the connection, UI and lifetime. Harness/SessionHandle delegates to the mounted SessionService. DesktopRuntime forwards to ConversationOrchestrator; that orchestrator owns conversation mutation, context preparation, permission-aware tool dispatch, compaction and outcome settlement. The subagent runner uses its own context and output ownership while sharing the same tool contracts.', '宿主负责工作准入、连接、界面与生命周期。Harness／SessionHandle 委托给已挂载 SessionService。DesktopRuntime 转交 ConversationOrchestrator；编排器负责对话修改、上下文准备、带权限的工具调度、压缩和结果结算。子 Agent runner 使用自己的上下文与输出归属，同时共享工具契约。'),
          t('llm-runtime is the Harness-facing model execution adapter. Its ProviderApiAdapter translates host conversation data through the independent llm-client SDK. Provider protocols, network transport and stream decoding remain in llm-client; local tools and host authorization remain in Harness.', 'llm-runtime 是面向 Harness 的模型执行适配层。ProviderApiAdapter 经独立 llm-client SDK 转换宿主对话数据。供应商协议、网络传输与流解码属于 llm-client；本地工具与宿主授权属于 Harness。'),
        ],
        diagram: { src: '/docs-assets/runtime-agent-loop.svg', alt: t('Host admission, context, model stream, gated tools and durable settlement', '宿主准入、上下文、模型流、带权限工具和持久化结算'), caption: t('A tool result returns to the model loop; cancellation and failures converge on settlement and cleanup.', '工具结果返回模型循环；取消与失败汇合到结算及清理。') },
      },
      {
        id: 'prepare', title: t('Prepare the request context', '准备请求上下文'),
        steps: [
          t('Capture the current session, cancellation token, working directory and resolved model/profile when the host accepts the turn. A later UI selection cannot rewrite the identity of an already-running request.', '宿主接受轮次时捕获当前会话、取消令牌、工作目录和解析后的模型／profile。之后的 UI 选择不能改写已执行请求的身份。'),
          t('Load applicable instruction memory, command/skill context, session history and host capability reminders. Advertise tools after enablement, session allowlists and provider capability projection.', '加载适用的指令记忆、命令／skill 上下文、会话历史和宿主能力提示。工具声明经过启用、会话允许列表及供应商能力投影。'),
          t('Apply context modifiers and compaction where required, then submit the model request through the SDK boundary. Keep provider credentials and host policy out of model-supplied tool arguments.', '按需应用上下文修改器与压缩，再通过 SDK 边界提交模型请求。供应商凭据与宿主策略不能来自模型提供的工具参数。'),
        ],
        note: t('Context preparation is not the displayed transcript: hidden instructions, tool references, reminders and compacted summaries can alter the request while persisted visible message identity remains stable.', '上下文准备并不等于显示的对话记录：隐藏指令、工具引用、提示和压缩摘要可能改变请求，同时持久化可见消息标识保持稳定。'),
      },
      {
        id: 'stream-tools', title: t('Stream output and execute tools', '流式输出与工具执行'),
        paragraphs: [
          t('The streaming pump accumulates typed HistoryEvent values and checks content-block and terminal boundaries. It emits text and completed assistant rows incrementally. Each tool call retains tool_use_id, canonical name, reassembled JSON input and the original provider call ID. Tool dispatch may begin at a completed tool block while later response blocks continue arriving.', '流式 pump 累积类型化 HistoryEvent，并检查内容 block 与终态边界。文本和完成的助手行逐步输出。每个工具调用保留 tool_use_id、规范名称、重组的 JSON 输入和原始供应商调用 ID。工具 block 完成时即可开始调度，同时后续响应 block 继续到达。'),
          t('Tracked dispatch validates input, applies hooks and permission policy, schedules concurrency according to the tool contract, and publishes a matching result. The result enters history for the next model request. Message grouping IDs, actual persisted row UUIDs and provider IDs have different roles; preserve each instead of treating them as interchangeable.', '带追踪的调度校验输入、应用 hooks 与权限策略、按工具契约安排并发，并发布匹配结果。结果进入历史供下一次模型请求使用。消息分组 ID、实际持久化行 UUID 和供应商 ID 职责不同，应各自保留，不能互换。'),
        ],
        links: [{ label: t('Tool implementation contract', '工具实现契约'), href: '/docs/harness-tools' }, { label: t('Session storage and identity', '会话存储与标识'), href: '/docs/session-storage' }],
      },
      {
        id: 'stop-recovery', title: t('Stop, cancel and recover', '停止、取消与恢复'),
        paragraphs: [
          t('EndTurn, MaxTurns and Cancelled describe successful control outcomes. A provider, stream, tool or persistence error can instead fail the turn. Stream EOF without a valid completion is not automatically success. Partial output and paired error results are retained where the implemented recovery path permits them.', 'EndTurn、MaxTurns 和 Cancelled 描述成功的控制结果。供应商、流、工具或持久化错误也可能导致轮次失败。没有有效完成信号的流 EOF 不能自动视为成功。实现的恢复路径允许时，保留部分输出及配对的错误结果。'),
          t('Prompt-too-long recovery belongs to the orchestration/context layer. The Harness model-service layer drives transient retry over SDK-prepared calls; llm-client owns request encoding and transport, while its prepared dispatch does not replay a turn automatically. Neither recovery path can blindly re-execute a tool that already started. Cancellation unwinds owned work and preserves outcome evidence; it is not proof that external writes were rolled back.', 'prompt-too-long 恢复属于编排／上下文层。Harness 模型服务层基于 SDK 准备后的调用驱动瞬态重试；llm-client 负责请求编码与传输，其 prepared 调度不会自动重放轮次。两种恢复路径都不能盲目重执行已启动工具。取消退出所属工作并保留结果证据，并不证明外部写入已回滚。'),
        ],
      },
      {
        id: 'example-checks', title: t('Replayable input and acceptance', '可重放输入与验收'),
        paragraphs: [t('This is a complete ClientCommand payload for a mounted session, not a Bridge transport envelope. Route events and permission replies through the same connection owner. To exercise the Rust facade without a provider, use the complete lifecycle test in Host lifecycle & persistence.', '这是已挂载会话的完整 ClientCommand 载荷，不是 Bridge 传输封装。通过同一连接负责人路由事件与权限响应。无需供应商验证 Rust 门面时，使用“宿主生命周期与持久化”中的完整生命周期测试。')],
        code: [{ label: t('ClientCommand · admitted turn', 'ClientCommand · 准入轮次'), language: 'json', verification: 'excerpt', code: '{\n  "type": "send_prompt",\n  "text": "Read the project instructions and explain the architecture",\n  "images": [],\n  "turn_id": 42\n}' }],
        bullets: [
          t('Existing tests cover prompt preparation, cancellation at admission, accepted streaming rows, truncated streams, tool pairing and prompt-too-long recovery. Run the scoped orchestrator suite against the baseline under review.', '现有测试覆盖提示准备、准入时取消、已接受流行、截断流、工具配对及 prompt-too-long 恢复。应对审查基线运行对应 orchestrator 测试。'),
          t('Live provider streaming, UI ordering during disconnect and irreversible external tool effects require separate host/service acceptance. Source presence and mock success do not establish those results.', '真实供应商流、断线时 UI 顺序及不可逆外部工具效果需要独立宿主／服务验收。源码存在与模拟成功不能证明这些结果。'),
        ],
      },
    ],
  },
  {
    id: 'configuration-security', group: 'system', title: t('Configuration, credentials and isolation', '配置、凭据与隔离'),
    description: t('Trace settings provenance and keep model routing, authorization, credential storage and OS isolation as explicit host responsibilities.', '追踪设置来源，明确模型路由、授权、凭据存储与操作系统隔离的宿主职责。'),
    sourceUrl,
    sources: sources('crates/core/src/settings/mod.rs', 'crates/core/src/settings/loader.rs', 'crates/core/src/settings/merger.rs', 'crates/core/src/settings/schema.rs', 'crates/runtime/src/desktop/settings_watch.rs', 'crates/runtime/src/desktop/credentials.rs', 'crates/platforms/posix/src/secure_storage/factory.rs', 'crates/platforms/posix/src/secure_storage/plaintext.rs', 'crates/permission/src/mode.rs', 'crates/permission/src/policy_gate.rs', 'crates/sandbox/src/decision.rs'),
    sections: [
      {
        id: 'settings-layers', title: t('Settings sources and merge rules', '设置来源与合并规则'),
        paragraphs: [
          t('The effective value stack merges defaults → user → project → local → CLI/flag settings → managed tiers → environment. Managed tiers are supplied in ascending priority. The user file is ~/.lingxi/settings.json; project and local files are .lingxi/settings.json and .lingxi/settings.local.json under the project. Explicit hosts may select a different user path and file-layer scope.', '生效值按默认 → 用户 → 项目 → 本地 → CLI／flag 设置 → managed 层 → 环境进行合并。managed 层按优先级升序提供。用户文件为 ~/.lingxi/settings.json；项目和本地文件为项目下的 .lingxi/settings.json 与 .lingxi/settings.local.json。显式宿主可选择不同用户路径和文件层范围。'),
          t('Merge semantics are field-specific: supplied scalars override, list fields concatenate/deduplicate, and hooks/sandbox objects deep-merge. Permission arrays accumulate. A malformed, oversized or unreadable file layer is skipped as a whole; later valid layers still apply. The provenance trace explains the selected source; never infer it from just the last file opened.', '合并语义按字段区分：已提供标量覆盖，列表拼接／去重，hooks／sandbox 对象深度合并；权限数组累积。格式错误、过大或不可读文件作为整层跳过，后续有效层继续生效。来源 trace 解释最终选定来源，不能仅由最后打开的文件推断。'),
        ],
      },
      {
        id: 'credential-lifecycle', title: t('Credentials and route identity', '凭据与路由标识'),
        paragraphs: [
          t('The host resolves provider profile, account and credential source before handing request services to llm-client. Secret values belong to secure storage and request headers, not JSONL, tool arguments or public configuration exports. An empty startup API key may allow command initialization, while a model request later fails authentication; construction success is not sign-in success.', '宿主先解析供应商 profile、账户与凭据来源，再将请求服务交给 llm-client。秘密值属于安全存储与请求 header，不属于 JSONL、工具参数或公开配置导出。空启动 API key 可能允许命令初始化，但模型请求之后认证失败；构建成功并非登录成功。'),
          t('On macOS, NativePreferred requires the bundled credential broker and fails closed for a mispackaged client. NativeOrMemory is an explicitly isolated process-local mode; PlainTextFixture is an explicit test selection. Linux secure-storage availability and fallback follow its factory path. A plaintext backend uses restrictive file permissions, but those permissions are not encryption.', 'macOS 的 NativePreferred 要求随包凭据 broker；打包身份错误时拒绝继续。NativeOrMemory 是显式隔离的进程内模式；PlainTextFixture 是显式测试选项。Linux 安全存储可用性与回退遵循其 factory 路径。明文后端使用严格文件权限，但文件权限不等于加密。'),
        ],
        note: t('Do not log whole configuration or credential structs. Field-specific Debug implementations and historical comments are not an unconditional redaction guarantee; log profile IDs, source names and failure categories instead of secret contents.', '不要记录整个配置或凭据 struct。字段专用 Debug 实现与历史注释不能作为无条件脱敏保证；记录 profile ID、来源名称及失败分类，避免秘密内容。'),
      },
      {
        id: 'authorization', title: t('Tool authorization', '工具授权'),
        paragraphs: [
          t('PolicyGate is the dispatch enforcement seam. Allow proceeds, Deny returns a denied tool outcome, and Ask uses the injected prompting transport when required. Session visibility filters and per-invocation authorization are separate controls. The same authorization seam applies to subagents and teammates.', 'PolicyGate 是调度执行约束边界。Allow 继续执行，Deny 返回拒绝的工具结果，Ask 在需要时使用注入的提示传输。会话可见性过滤与逐调用授权是独立控制。同一授权边界适用于子 Agent 与 teammate。'),
          t('Wire mode names are default, plan, acceptEdits, bypassPermissions, dontAsk and auto; Bubble is internal. Mode availability and persistent-settings acceptance are not identical. Auto additionally depends on model/provider and host policy. Use mode/rule implementations for exact decisions; a mode label alone does not summarize every rule, hook or managed ceiling.', 'wire 模式名称为 default、plan、acceptEdits、bypassPermissions、dontAsk 和 auto；Bubble 仅内部使用。模式可用性与持久设置可接受性不相同。Auto 还受模型／供应商和宿主策略限制。具体决策以 mode／rule 实现为准；单个模式名称不能概括所有规则、hook 或 managed 上限。'),
        ],
      },
      {
        id: 'sandbox-and-watchers', title: t('OS sandbox and live settings', '系统沙箱与实时设置'),
        paragraphs: [
          t('Sandbox wrapping is chosen separately from permission mode and project trust. The decision considers actual runtime availability, an explicit override plus permission for unsandboxed execution, empty commands and excluded commands. Platform backends then implement the selected filesystem/network/process restrictions. A user approval does not create a missing native sandbox.', '沙箱封装独立于权限模式与项目信任选择。决策考虑实际 runtime 可用性、显式覆盖及非沙箱执行权限、空命令和排除命令，再由平台后端落实文件系统／网络／进程限制。用户授权不会创建缺失的原生沙箱。'),
          t('Desktop settings watchers monitor user/project/local/managed directories and fire ConfigChange. The watcher starts even without configured hooks because managed safety updates still need observation. Managed drop-ins are read in sorted order. Retain the watcher handle for the lifetime of the composition; dropping it stops its tasks.', '桌面设置 watcher 监控用户／项目／本地／managed 目录并触发 ConfigChange。即使未配置 hooks，也启动 watcher，因为 managed 安全更新仍需观测。managed drop-in 按排序读取。在组合生命周期内保留 watcher handle；丢弃它会停止任务。'),
        ],
      },
      {
        id: 'safe-example', title: t('Project-local example and verification', '项目本地示例与验证'),
        paragraphs: [t('This complete .lingxi/settings.local.json example grants only a read-oriented tool. It contains no credentials. It is one input layer: managed rules and live authorization still participate, and tool enablement depends on the host. Keep personal choices in the local file rather than exporting secrets with project configuration.', '以下完整 .lingxi/settings.local.json 示例只声明读操作工具授权，不含凭据。它只是输入层之一：managed 规则与实时授权仍然参与，工具启用取决于宿主。个人选择放在本地文件中，避免随项目配置导出秘密。')],
        code: [{ label: '.lingxi/settings.local.json', language: 'json', verification: 'excerpt', code: '{\n  "permissions": {\n    "allow": ["Read"]\n  }\n}' }, { label: t('Focused settings and sandbox checks', '定向设置与沙箱检查'), language: 'bash', verification: 'manual', code: 'cargo test --locked -p core --test settings_4layer_test\ncargo test --locked -p harness-runtime --features desktop --test settings_watch_spawn\ncargo test --locked -p harness-runtime --features desktop --test sandbox_startup_test' }],
        note: t('These tests validate merge, watcher and startup contracts. Packaged macOS broker identity, native sandbox enforcement, Linux secret service and mobile OS permission prompts must be checked on their actual targets.', '这些测试验证合并、watcher 与启动契约。打包后的 macOS broker 身份、原生沙箱约束、Linux secret service 及移动系统权限提示需在实际目标验证。'),
      },
    ],
  },
  {
    id: 'session-storage', group: 'system', title: t('Session identity and durable storage', '会话标识与持久化存储'),
    description: t('Understand JSONL graphs, stable UUIDs, cold restoration, forks and the difference between context compaction and transcript reclamation.', '理解 JSONL 图、稳定 UUID、冷恢复、分支及上下文压缩与对话存储回收的区别。'),
    sourceUrl,
    sources: sources('crates/session/src/jsonl/path.rs', 'crates/session/src/jsonl/schema.rs', 'crates/session/src/jsonl/reader.rs', 'crates/session/src/jsonl/recovery.rs', 'crates/session/src/jsonl/durable_writer.rs', 'crates/session/src/jsonl/message_identity.rs', 'crates/session/src/jsonl/transcript_compact.rs', 'crates/session/src/resumer.rs', 'crates/session/src/branch.rs', 'crates/orchestrator/src/conversation/compaction.rs', 'crates/client/src/adapter/lowering/messages.rs', 'crates/runtime/src/desktop/session_state/resume.rs'),
    sections: [
      {
        id: 'layout-and-ids', title: t('Storage layout and identities', '存储布局与标识'),
        paragraphs: [
          t('Production transcripts live at <config-home>/projects/<sanitized-cwd>/<session-uuid>.jsonl. Long sanitized cwd names receive a deterministic DJB2 suffix. Large tool outputs live under the sibling <session-uuid>/tool-results directory, not beneath the project workspace. Rollout recording has a separate lower-level schema and policy; do not interchange the formats.', '生产对话保存在 <config-home>/projects/<sanitized-cwd>/<session-uuid>.jsonl。较长的清洗 cwd 名称带确定性 DJB2 后缀。大型工具输出位于相邻 <session-uuid>/tool-results 目录，而不是项目工作空间内。Rollout 记录有独立底层 schema 和策略，两种格式不能互换。'),
          t('Session IDs identify mounted conversations. Outer JSONL UUIDs identify persisted rows; parentUuid links the graph. Provider response IDs identify remote responses. Host stream indexes preserve presentation ordering and use a monotonic watermark even after rows disappear. Text and visual position are unsuitable keys for restoring timestamps or resolving duplicates.', 'Session ID 标识挂载对话。外层 JSONL UUID 标识持久化行，parentUuid 连接图。供应商响应 ID 标识远端响应。宿主 stream index 保持展示顺序，即使行消失也保留单调水位。文本与视觉位置不能作为恢复时间或解决重复的键。'),
        ],
        diagram: { src: '/docs-assets/runtime-session-lifecycle.svg', alt: t('Durable append and UUID graph restoration, fork and context compaction', '持久追加、UUID 图恢复、分支与上下文压缩'), caption: t('A resume reuses the source session; a fork creates a new session identity. Context compaction does not erase the historical transcript by itself.', '恢复复用来源会话；分支创建新会话标识。上下文压缩自身不会抹去历史对话。') },
      },
      {
        id: 'durability', title: t('Append and publication barriers', '追加与发布屏障'),
        paragraphs: [
          t('The durable writer pins filesystem roots and handles, holds session-state authority, checks delivery identity, appends and fsyncs before acknowledging success. Retrying identical content with the same delivery identity returns AlreadyPresent; conflicting content for that identity fails. A write that reached the file but failed durability remains a failed publication until its retry crosses the barrier.', 'durable writer 固定文件系统根与 handle，持有会话状态权限，检查交付标识，追加并 fsync 后才确认成功。相同交付标识及内容的重试返回 AlreadyPresent；同标识但内容冲突则失败。已写入文件但持久化失败仍属于发布失败，直到重试跨过屏障。'),
          t('UI delivery, transcript snapshots and durable append are separate boundaries. Surface persistence warnings and preserve retry information. Shutdown drains the owning lifecycle coordinator; do not infer durability from MessageComplete or TurnEnded alone.', 'UI 交付、对话快照与持久追加是独立边界。展示持久化警告并保留重试信息。关闭由所属生命周期协调器排空；不能仅由 MessageComplete 或 TurnEnded 推断持久化。'),
        ],
      },
      {
        id: 'resume-and-fork', title: t('Cold resume and fork', '冷恢复与分支'),
        paragraphs: [
          t('The reader separates graph rows from metadata, indexes rows by UUID and uses the final occurrence for duplicate UUIDs. Recovery reconstructs the selected parent chain and anchored tool results. Reading file lines directly is useful for diagnostics, but it is not the resume algorithm.', 'reader 区分图行与 metadata，按 UUID 建索引，同 UUID 采用最后一次记录。恢复重建选定父链与锚定工具结果。直接读取文件行可用于诊断，但不是恢复算法。'),
          t('SessionResumer restores messages and metadata with an empty FileStateCache so historical Read state cannot authorize stale file assumptions. Runtime composition reattaches permissions, plugin/MCP connections, output style and accounting. Forking creates a new session JSONL and rewrites session identity/parent chain/sidechain facts; it is not simply renaming the original file.', 'SessionResumer 恢复消息与 metadata，并创建空 FileStateCache，避免历史 Read 状态支持过期文件假设。权限、插件／MCP 连接、输出样式和计费由 runtime 组合重新挂载。分支创建新 session JSONL，并重写会话标识／父链／sidechain 事实，不是简单改名原文件。'),
          t('In the CLI, /branch creates this sibling conversation and mounts it for the user; /fork names a different operation that spawns a detached background agent. Select the host command by its actual behavior rather than assuming every use of the word fork creates the same session lifecycle.', '在 CLI 中，/branch 创建这种相邻对话并为用户挂载；/fork 是另一个操作，启动脱离前台的后台 Agent。选择宿主命令时应依据实际行为，不能假设所有名为 fork 的操作都创建相同会话生命周期。'),
        ],
      },
      {
        id: 'compaction', title: t('Two different compactions', '两种不同的压缩'),
        tables: [{ headers: [t('Operation', '操作'), t('Purpose and owner', '目的与负责人')], rows: [
          [t('Model context compaction', '模型上下文压缩'), t('Orchestrator/compaction reduce the next request context using summaries, boundaries and retained state. It preserves the persisted history needed for replay and host projection.', 'Orchestrator／compaction 通过摘要、边界和保留状态减少后续请求上下文，保留重放与宿主投影所需的持久化历史。')],
          [t('Local transcript reclamation', '本地对话存储回收'), t('session/jsonl/transcript_compact reclaims superseded metadata. Local GC is off by default and controlled by LINGXI_TRANSCRIPT_LOCAL_GC. It is not the model context window policy.', 'session／jsonl／transcript_compact 回收被替代的 metadata。本地 GC 默认关闭，受 LINGXI_TRANSCRIPT_LOCAL_GC 控制，与模型上下文窗口策略不同。')],
        ] }],
        note: t('Do not hand-edit a live JSONL to reduce token use. Use the orchestrator compaction interface, and preserve the writer authority and session epoch while switching or recovering a session.', '不要通过手工编辑活动 JSONL 减少 token。使用 orchestrator 压缩接口，切换或恢复会话时保留 writer 权限与 session epoch。'),
      },
      {
        id: 'inspection', title: t('Read-only diagnostic example', '只读诊断示例'),
        paragraphs: [t('The script counts record types and duplicate UUIDs without printing prompt text, credentials or tool payloads. Supply a copied transcript path. It intentionally reports malformed lines instead of silently presenting them as a valid restored session.', '此脚本统计记录类型与重复 UUID，不打印提示文本、凭据或工具载荷。传入复制后的对话文件路径。它明确报告格式错误行，不会静默将其展示为有效恢复会话。')],
        code: [{ label: t('Python · inspect a copied JSONL', 'Python · 检查已复制的 JSONL'), language: 'python', verification: 'manual', code: 'import collections\nimport json\nimport pathlib\nimport sys\n\ncounts = collections.Counter()\nuuids = collections.Counter()\nfor line_no, line in enumerate(pathlib.Path(sys.argv[1]).read_text().splitlines(), 1):\n    try:\n        row = json.loads(line)\n    except json.JSONDecodeError:\n        print(f"malformed line: {line_no}")\n        continue\n    if not isinstance(row, dict):\n        print(f"non-object line: {line_no}")\n        continue\n    counts[str(row.get("type", "unknown"))] += 1\n    if isinstance(row.get("uuid"), str):\n        uuids[row["uuid"]] += 1\nprint("record types:", dict(sorted(counts.items())))\nprint("duplicate UUID count:", sum(count > 1 for count in uuids.values()))' }],
        bullets: [t('Existing session tests cover golden JSONL, duplicate UUIDs, parent-chain recovery, fork names and transcript compaction wiring. Orchestrator tests separately cover durable append warnings and timestamp projection.', '现有 session 测试覆盖 golden JSONL、重复 UUID、父链恢复、分支名称及对话压缩接通。Orchestrator 测试另行覆盖持久追加警告与时间投影。'), t('Crash recovery on a target filesystem, interrupted writes and cold resume with actual attachments need end-to-end host acceptance. A parsed copy is diagnostic evidence only.', '目标文件系统上的崩溃恢复、中断写入与带真实附件的冷恢复需要端到端宿主验收。可解析的副本仅提供诊断证据。')],
      },
    ],
  },
  {
    id: 'extensions-lifecycle', group: 'system', title: t('Tools, skills, hooks, MCP and LSP', '工具、Skills、Hooks、MCP 与 LSP'),
    description: t('Discover extensions through trusted registries and retain authority across activation, refresh, invocation and teardown.', '经可信注册表发现扩展，并在激活、更新、调用和清理过程中保持权限归属。'),
    sourceUrl,
    sources: sources('crates/tool-api/src/registry.rs', 'crates/skill-api/src/lib.rs', 'crates/runtime/src/desktop/skill_loader.rs', 'crates/hooks/src/events.rs', 'crates/hooks/src/executor.rs', 'crates/hooks/src/ssrf_guard.rs', 'crates/plugin/src/manager.rs', 'crates/mcp/src/registry.rs', 'crates/mcp/src/registry/catalog_refresh.rs', 'crates/lsp/src/registry.rs', 'crates/runtime/src/mobile/mobile_lsp.rs'),
    sections: [
      {
        id: 'extension-kinds', title: t('Extension responsibilities', '扩展职责'),
        tables: [{ headers: [t('Surface', '接口'), t('Runtime responsibility', '运行时职责')], rows: [
          [t('Tool', '工具'), t('Canonical operation, schema, enablement, invocation permission, scheduling hints, progress and result.', '规范操作、schema、启用、调用权限、调度提示、进度与结果。')],
          [t('Skill/command', 'Skill／命令'), t('Discovered instruction content and invocation semantics. A skill is not an independent provider network transport.', '发现的指令内容与调用语义。Skill 不是独立供应商网络传输。')],
          [t('Hook', 'Hook'), t('Configured event behavior that can inject context, enforce decisions or run host-owned actions.', '配置的事件行为，可注入上下文、约束决策或执行宿主动作。')],
          [t('Plugin', '插件'), t('Validated package contributions across tool/command/agent/skill/hook/MCP/LSP/workflow registries.', '经过校验的包贡献，涵盖工具／命令／Agent／skill／hook／MCP／LSP／workflow 注册表。')],
        ] }],
      },
      {
        id: 'discovery-activation', title: t('Discover and activate once', '发现并单次激活'),
        paragraphs: [
          t('Desktop composition loads applicable commands/skills and constructs the production PluginManager with shared registries. ToolRegistry advertises deterministic partitions for builtins and dynamic sources. Session allowlists, tool-pool selection, plugin trust and managed restrictions participate before capabilities reach the model.', '桌面组合加载适用命令／skills，使用共享注册表构建生产 PluginManager。ToolRegistry 以确定性分区声明内置与动态来源。能力到达模型前，会话允许列表、工具池选择、插件信任与 managed 限制均参与控制。'),
          t('A package manifest can describe components without proving successful activation. Inspect loaded state and diagnostics. Reload updates the owning partition rather than duplicating capabilities; disable tears down contributed components and retains explicit failure state when teardown cannot complete.', '包 manifest 可以声明组件，但不能证明激活成功。应检查加载状态与诊断。Reload 更新所属分区而不是复制能力；Disable 清理贡献组件，清理未完成时保留明确失败状态。'),
        ],
      },
      {
        id: 'hook-flow', title: t('Hooks and permission-aware invocation', 'Hooks 与带权限的调用'),
        paragraphs: [
          t('Events include PreToolUse, PostToolUse, PostToolUseFailure, SessionStart/End, PermissionRequest, TaskCompleted, ConfigChange and PostToolBatch. Command, HTTP, prompt and agent executors have different dependencies and error semantics. Use the executor registry rather than interpreting hook JSON inside a UI client.', '事件包括 PreToolUse、PostToolUse、PostToolUseFailure、SessionStart／End、PermissionRequest、TaskCompleted、ConfigChange 和 PostToolBatch。Command、HTTP、prompt 与 agent executor 的依赖和错误语义不同。使用 executor 注册表，不在 UI 客户端自行解释 hook JSON。'),
          t('Hook output is parsed through the shared response contract and may affect dispatch/context. HTTP hooks apply the explicit SSRF guard. Hook-initiated MCP calls use the already-live MCP registry; the desktop late-binding bridge avoids a construction cycle and does not grant hooks independent server discovery authority.', 'Hook 输出经过共享 response 契约解析，可能影响调度／上下文。HTTP hook 应用显式 SSRF guard。Hook 发起的 MCP 调用使用已活动 MCP 注册表；桌面延迟绑定 bridge 避免构建循环，并不赋予 hook 独立发现服务器的权限。'),
        ],
      },
      {
        id: 'mcp-and-lsp', title: t('MCP and LSP lifecycle', 'MCP 与 LSP 生命周期'),
        paragraphs: [
          t('McpRegistry owns connections, authentication state, startup loading, catalog refresh and reconnect/backoff. The relevant host transport supplies stdio, HTTP, SSE or WebSocket support; mobile has a separate injected transport. A disconnected server is unavailable with diagnostic state, not a successful empty catalog. Elicitation and input-required states return through host request routing.', 'McpRegistry 负责连接、认证状态、启动加载、目录更新和重连／退避。对应宿主传输提供 stdio、HTTP、SSE 或 WebSocket；移动端使用独立注入传输。断开的服务器是带诊断状态的不可用服务，不是成功的空目录。Elicitation 与 input-required 状态经宿主请求路由返回。'),
          t('LSP registration accepts verified plugin servers; user/project settings cannot inject arbitrary LSP servers. The registry tracks activation, connections and restart policy (default maximum three restarts). Mobile Auto/Off/On activation uses trusted workspace classification and Mobile Linux transport, so native process availability remains a prerequisite.', 'LSP 注册只接收已验证插件服务器；用户／项目设置不能注入任意 LSP 服务器。注册表追踪激活、连接与重启策略（默认最多三次重启）。移动端 Auto／Off／On 激活使用可信工作空间分类与 Mobile Linux 传输，原生进程可用性仍是前提。'),
        ],
      },
      {
        id: 'extension-checks', title: t('Reproducible lifecycle checks', '可重复生命周期检查'),
        code: [{ label: t('MCP and LSP registry tests', 'MCP 与 LSP 注册表测试'), language: 'bash', verification: 'manual', code: 'cargo test --locked -p mcp --test compile_smoke\ncargo test --locked -p lsp --test plugin_only_registration_test\ncargo test --locked -p lsp --test spawn_handshake_test\ncargo test --locked -p test-harness --test mcp_lifecycle' }],
        steps: [t('Verify discovery with an explicit fixture, activate it, check advertised canonical names and exercise one permission-gated invocation.', '使用显式 fixture 验证发现，激活后检查声明的规范名称，并执行一次受权限控制调用。'), t('Simulate transport disconnect and catalog refresh; ensure old tools disappear and pending requests resolve with the appropriate failure.', '模拟传输断开与目录更新，确认旧工具消失，等待请求以对应失败结束。'), t('Disable the plugin, inspect owned process cleanup and retry a retained teardown failure. Test genuine third-party processes and OAuth separately.', '禁用插件，检查所属进程清理并重试保留的清理失败。真实第三方进程与 OAuth 单独验收。')],
        note: t('These commands identify existing contracts; they do not certify remote server behavior or third-party plugins. Keep external service versions and target-host evidence with the acceptance record.', '这些命令定位现有契约，不认证远端服务器行为或第三方插件。验收记录应包含外部服务版本与目标宿主证据。'),
      },
    ],
  },
  {
    id: 'agents-background', group: 'system', title: t('Subagents and background tasks', '子 Agent 与后台任务'),
    description: t('Separate agent-pool admission, task ownership, mailbox delivery and terminal publication across foreground and detached work.', '区分前台与脱离前台工作中的 Agent 池准入、任务归属、邮箱交付与终态发布。'),
    sourceUrl,
    sources: sources('crates/agent/src/context.rs', 'crates/agent/src/handle.rs', 'crates/agent/src/runner.rs', 'crates/agent/src/pool.rs', 'crates/agent/src/pool/capacity.rs', 'crates/runtime/src/desktop/background_agent.rs', 'crates/runtime/src/desktop/session_agents.rs', 'crates/tasks/src/lib.rs', 'crates/tasks/src/handlers/local_agent.rs', 'crates/coordinator/src/lib.rs', 'crates/runtime/src/parked_agent_restore.rs'),
    sections: [
      {
        id: 'agent-boundary', title: t('Agent context and admission', 'Agent 上下文与准入'),
        paragraphs: [
          t('An agent carries its definition, resolved model/profile, allowed tools, working context, cancellation and parent ownership. Pool admission enforces capacity rather than assuming unlimited parallelism. Child permission mode is derived from trusted parent/definition facts and policy; a model argument cannot manufacture broader authority.', 'Agent 携带定义、解析模型／profile、允许工具、工作上下文、取消和父级归属。池准入执行容量限制，而不是假设无限并发。子权限模式由可信父级／定义事实与策略派生，模型参数不能制造更大权限。'),
          t('The main orchestrator and subagent runner share provider and tool contracts but have distinct histories and output projection. Keep agent_id, task_id, session_id and provider request IDs separate. Subagent transcript rows retain message identity, including partial stream and terminal error evidence.', '主编排器与子 Agent runner 共享供应商及工具契约，但使用独立历史和输出投影。agent_id、task_id、session_id 与供应商请求 ID 应分开。子 Agent 对话行保留消息标识，包括部分流和终态错误证据。'),
        ],
      },
      {
        id: 'background-composition', title: t('Background execution needs a host decorator', '后台执行需要宿主装饰器'),
        paragraphs: [
          t('Desktop background_agent is the actual async-agent decorator. It converts Agent run_in_background into a persistent LocalAgent task, installs the mailbox and pumps SendMessage toward task resume. The bare pool spawner reports an unwired-path error for unsupported detached admission; merely constructing a pool does not provide durable background execution.', 'Desktop 的 background_agent 是实际 async-agent 装饰器。它将 Agent run_in_background 转为持久 LocalAgent task，安装邮箱并把 SendMessage 推向任务恢复。裸池 spawner 对未接通的脱离前台准入报告错误；仅构建池并不会提供持久后台执行。'),
          t('TaskRegistry owns status, output and cancellation for long-lived work. The host routes lifecycle/progress notifications and publishes a terminal result once. A detached caller or closed foreground panel must not silently lose the task owner or its durable receipt.', 'TaskRegistry 负责长期工作的状态、输出和取消。宿主路由生命周期／进度通知并单次发布终态结果。调用方脱离或前台面板关闭时，不能静默丢失任务负责人或持久回执。'),
        ],
      },
      {
        id: 'mailbox-and-observer', title: t('Messages, observers and parked agents', '消息、观察者与停驻 Agent'),
        paragraphs: [
          t('Mailbox delivery and model execution are different phases. A message accepted by the host is not proof that the recipient model has consumed it. Runtime agent listings and transcript loading project shared state; observer guards prevent output from a stale owner or replaced session from appearing in the current conversation.', '邮箱交付与模型执行是不同阶段。宿主接受消息并不证明接收方模型已消费它。运行时 Agent 列表与对话加载投影共享状态；observer guard 防止过期负责人或替换会话的输出进入当前对话。'),
          t('Parked agents preserve resumable state rather than being treated as running forever. Restoration must recover their session ownership, tool grants and mailbox linkage before admission. An agent record without a live or restored owner is not an executable background task.', '停驻 Agent 保留可恢复状态，而不是被视为永久运行。恢复必须在准入前重建会话归属、工具授权和邮箱关联。没有活动或恢复负责人的 Agent 记录并非可执行后台任务。'),
        ],
      },
      {
        id: 'failure-and-cancel', title: t('Failure, capacity and cancellation', '失败、容量与取消'),
        paragraphs: [
          t('Report admission starvation separately from provider failure and tool failure. Cancellation propagates through the task/agent owner; collect final transcript and output cleanup before releasing leases. A partially emitted response can be retained on provider failure, while a terminal task status still records the failure.', '准入饥饿、供应商失败和工具失败分别报告。取消经 task／agent 负责人传播；释放 lease 前收集最终对话并清理输出。供应商失败时可保留部分已输出响应，同时终态 task 状态仍记录失败。'),
          t('Coordinator/teammate behavior is enabled by the selected composition, shared registry and transport. Desktop in-process and pane teammates have host-specific lifecycle. Do not infer cross-device collaboration or mobile parity from the presence of the collaboration Cargo feature alone.', 'Coordinator／teammate 行为由选定组合、共享注册表和传输启用。桌面进程内及 pane teammate 有各自宿主生命周期。不能仅由 collaboration Cargo feature 推断跨设备协作或移动端等价能力。'),
        ],
      },
      {
        id: 'agent-checks', title: t('Exercise admission and owner cleanup', '验证准入与负责人清理'),
        code: [{ label: t('Agent and desktop composition checks', 'Agent 与桌面组合检查'), language: 'bash', verification: 'manual', code: 'cargo test --locked -p agent --lib\ncargo test --locked -p harness-runtime --features desktop --test coordinator_activation\ncargo test --locked -p harness-runtime --features desktop --test pool_starvation\ncargo test --locked -p harness-runtime --features desktop --test background_output_writer' }],
        bullets: [t('Test pool capacity, a queued cancellation, foreground detachment, mailbox resume, retained partial output and a terminal publication retry. Existing unit/fixture checks do not establish multi-provider live behavior.', '测试池容量、排队取消、前台脱离、邮箱恢复、部分输出保留及终态发布重试。现有单元／fixture 检查不代表多供应商真实行为已验收。'), t('Validate pane processes, workspace cleanup and external task side effects on the actual desktop OS. Cold restart acceptance must show restored ownership, not just a persisted task list.', '在实际桌面系统验证 pane 进程、工作空间清理与外部任务副作用。冷重启验收必须证明负责人已恢复，不能只展示持久 task 列表。')],
      },
    ],
  },
  {
    id: 'automation-workflows-fusion', group: 'system', title: t('Cron, Workflow and Fusion', 'Cron、Workflow 与 Fusion'),
    description: t('Understand the live execution paths for schedules, scripted agent orchestration and evidence-based multi-model work.', '理解计划任务、脚本 Agent 编排及多模型证据工作的实际执行路径。'),
    sourceUrl,
    sources: sources('crates/runtime/src/desktop/assembly.rs', 'crates/runtime/src/desktop/cron_management.rs', 'crates/runtime/src/mobile/host/automation.rs', 'crates/runtime/src/mobile/host/assembly.rs', 'crates/cron/src/run_due.rs', 'crates/tools/workflow/src/lib.rs', 'crates/tools/workflow/src/workflow_input_schema.json', 'crates/workflow/src/lib.rs', 'crates/workflow/src/description.rs', 'crates/tasks/src/handlers/local_workflow.rs', 'crates/runtime/src/desktop/fusion_services.rs', 'crates/runtime/src/desktop/fusion_attempts.rs', 'crates/runtime/src/desktop/fusion_recorder.rs', 'crates/runtime/src/desktop/fusion_implement.rs', 'crates/fusion/src/orchestrator.rs'),
    sections: [
      {
        id: 'execution-matrix', title: t('Composition support', '组合支持'),
        tables: [{ headers: [t('Capability', '能力'), t('Desktop', '桌面'), t('Mobile', '移动端')], rows: [
          [t('Cron', 'Cron'), t('CronScheduler is built, loaded and started when host scheduler/trust policy and flags permit; child runtimes do not own another scheduler.', '宿主 scheduler／trust 策略与 flag 允许时，构建、加载并启动 CronScheduler；子 runtime 不再拥有重复 scheduler。'), t('Persistent schedules and foreign-host-triggered firing. The Rust mobile composition does not run a continuous CronScheduler.', '持久计划与外部宿主触发执行。Rust 移动组合不运行持续 CronScheduler。')],
          [t('Workflow', 'Workflow'), t('TaskRegistryWorkflowLauncher and QuickJS-backed local_workflow tasks are wired.', '已接通 TaskRegistryWorkflowLauncher 与 QuickJS 支持的 local_workflow task。'), t('MobileWorkflowLauncher is wired into mobile assembly; OS lifecycle still governs execution opportunities.', 'MobileWorkflowLauncher 接入移动组装；执行机会仍受系统生命周期管理。')],
          [t('Fusion', 'Fusion'), t('Production services connect attempt admission, pricing, evidence, implementation and durable receipts.', '生产服务连接尝试准入、计费、证据、实现与持久回执。'), t('Do not infer a full mobile executor from shared DTO/history projection. Verify the native host entrypoint before offering execution.', '不能由共享 DTO／历史投影推断完整移动执行器。提供执行前应验证原生宿主入口。')],
        ] }],
      },
      {
        id: 'cron-lifecycle', title: t('Scheduled execution and OS constraints', '计划执行与系统约束'),
        paragraphs: [
          t('Schedule persistence, due-occurrence selection and turn execution are separate phases. Cron run_due serializes storage read-modify-write with a process-wide lock and releases it before running the turn. Host scheduling/trust policy controls when autonomous turns may start, and persisted results retain the originating task identity.', '计划持久化、到期 occurrence 选择与轮次执行是独立阶段。Cron run_due 使用进程级锁串行化存储读改写，并在运行轮次前释放。宿主计划／信任策略控制自治轮次准入，持久结果保留来源 task 标识。'),
          t('Mobile firing returns session_id, result_text, terminal status and retryable. Its turn budget is 180 seconds beneath the OS worker lifecycle. Mobile recurring intervals must be at least 15 minutes; one-shot schedules are exempt. A schedule accepted by Rust does not guarantee an exact OS wake time or that iOS/Android grant background execution.', '移动触发返回 session_id、result_text、终态 status 和 retryable。轮次预算为 180 秒，并受更外层系统 worker 生命周期约束。移动重复间隔至少 15 分钟，单次计划不受该限制。Rust 接受计划不保证精确系统唤醒时间，也不保证 iOS／Android 允许后台执行。'),
        ],
      },
      {
        id: 'workflow-runtime', title: t('Workflow scripts and resume', 'Workflow 脚本与恢复'),
        paragraphs: [
          t('Workflow accepts inline script, named workflow or scriptPath. scriptPath takes precedence and is reopened through its approved rooted identity rather than by an untrusted pathname. Scripts are limited to 512 KiB and run through the QuickJS workflow runtime. A missing launcher reports a clear unwired failure.', 'Workflow 接收内联 script、命名 workflow 或 scriptPath。scriptPath 优先，并以批准的 rooted 身份重新打开，而不是使用不可信路径名。脚本上限 512 KiB，经 QuickJS workflow runtime 执行。缺失 launcher 时报告明确的未接通失败。'),
          t('A successful launch normally returns async_launched and task/run IDs, then completion arrives as a task notification. Parse/compile diagnostics can be returned without a registered task; check the returned error and task identity. ResumeFromRunId is same-session and reuses unchanged agent-call results; stop the old task before resuming an edited workflow.', '成功启动通常返回 async_launched 与 task／run ID，随后完成以 task 通知到达。解析／编译诊断可能在未注册 task 时返回，需检查返回 error 与 task 标识。ResumeFromRunId 限同会话，并复用未变化的 Agent 调用结果；恢复编辑后的 workflow 前应停止旧 task。'),
        ],
        code: [
          { label: t('review-workflow.js · complete Workflow script', 'review-workflow.js · 完整 Workflow 脚本'), language: 'javascript', verification: 'manual', code: `export const meta = {
  name: 'boundary-review',
  description: 'Inspect provider and host ownership, then summarize evidence.',
  phases: [{ title: 'Inspect' }, { title: 'Summarize' }],
};

const question = typeof args === 'object' && args !== null
  ? String(args.question ?? 'Explain the provider and host boundaries')
  : String(args ?? 'Explain the provider and host boundaries');

phase('Inspect');
const evidence = await agent(
  'Read the project instructions and source. Answer with concrete file evidence: ' + question,
  { label: 'inspect', phase: 'Inspect' },
);
if (evidence === null) throw new Error('Inspection agent did not complete');

phase('Summarize');
return await agent(
  'Summarize this evidence and identify any unverified assumptions:\\n' + evidence,
  { label: 'summarize', phase: 'Summarize' },
);` },
          { label: t('Workflow tool input · arguments retain their JSON type', 'Workflow 工具输入 · 参数保留 JSON 类型'), language: 'json', verification: 'excerpt', code: '{\n  "scriptPath": "./review-workflow.js",\n  "args": {\n    "question": "Which modules own the provider and host boundaries?"\n  }\n}' },
        ],
        note: t('This is a Workflow script, whose body runs in the host async wrapper; its top-level return is intentional and is not a standalone browser/Node module. Agent calls use the actual host tools and model services, so execution still requires credentials, grants and a running launcher. The JSON payload does not bypass authorization.', '这是 Workflow 脚本，其主体在宿主 async wrapper 内运行；顶层 return 是有意设计，不是独立浏览器／Node 模块。Agent 调用使用真实宿主工具与模型服务，执行仍需凭据、授权和活动 launcher。JSON 载荷不能绕过授权。'),
      },
      {
        id: 'fusion-lifecycle', title: t('Fusion evidence and durable results', 'Fusion 证据与持久结果'),
        paragraphs: [
          t('Desktop Fusion composes model resolution, panel/attempt admission, pricing and budget ownership, evidence collection and optional implementation. Evidence from multiple models still needs synthesis and a recorded result; concurrent completion is not agreement. Attempts retain model/profile/account attribution for cost and recovery.', '桌面 Fusion 组合模型解析、panel／attempt 准入、计费与预算归属、证据收集及可选实现。多模型证据仍需综合及记录结果，并发完成并不等于一致。Attempt 保留模型／profile／账户归属用于计费与恢复。'),
          t('FusionRecorder and desktop lifecycle retain durable publication receipts. ShutdownReport.publications exposes receipts observed while draining. Handle incomplete publication through the owning coordinator and retry barrier; do not report success solely because an analyst or implementer model finished streaming.', 'FusionRecorder 与桌面生命周期保留持久发布回执。ShutdownReport.publications 返回排空期间观察到的回执。未完成发布通过所属协调器及重试屏障处理；不能仅因 analyst 或 implementer 模型流结束就报告成功。'),
        ],
      },
      {
        id: 'automation-checks', title: t('Acceptance scenarios', '验收场景'),
        code: [{ label: t('Workflow and Fusion checks', 'Workflow 与 Fusion 检查'), language: 'bash', verification: 'manual', code: 'cargo test --locked -p tool-workflow --lib\ncargo test --locked -p tasks --lib\ncargo test --locked -p fusion --lib\ncargo test --locked -p harness-runtime --features desktop --lib fusion_attempt' }],
        bullets: [t('Exercise schedule storage, a duplicate due occurrence, denied autonomous admission, a timed-out mobile turn and retryable versus terminal failure. Test workflow malformed input, missing launcher, compile error, cancellation and same-session resume.', '验证计划存储、重复到期 occurrence、自治准入拒绝、移动轮次超时及可重试／终态失败。Workflow 验证错误输入、缺失 launcher、编译错误、取消与同会话恢复。'), t('Fusion checks cover pool admission, pricing and durable handoff. Real multi-model runs, native background delivery and post-crash publication require separate target acceptance records.', 'Fusion 检查覆盖池准入、计费和持久交接。真实多模型运行、原生后台交付及崩溃后发布需独立目标验收记录。')],
      },
    ],
  },
  {
    id: 'multimodal-host', group: 'system', title: t('Attachments, audio and realtime hosts', '附件、音频与实时宿主'),
    description: t('Route multimodal input and audio through explicit provider capability and native resource owners.', '经显式供应商能力与原生资源负责人路由多模态输入和音频。'),
    sourceUrl,
    sources: sources('crates/runtime/src/api.rs', 'crates/client/src/protocol/commands.rs', 'crates/orchestrator/src/image_input.rs', 'crates/audio-runtime/src/lib.rs', 'crates/audio-runtime/src/cloud.rs', 'crates/runtime/src/desktop/configuration.rs', 'crates/runtime/src/mobile/audio_service.rs', 'crates/runtime/src/mobile/host/audio_context.rs', 'crates/runtime/src/mobile/host/audio_realtime_owner.rs', 'crates/runtime/tests/mobile/realtime_owner.rs', 'crates/runtime/tests/desktop/audio_injection.rs'),
    sections: [
      {
        id: 'attachment-ingress', title: t('Attachment ingress and model capabilities', '附件输入与模型能力'),
        paragraphs: [
          t('RunInput.images is a vector of host paths. ClientCommand SendPrompt uses inline {media_type, base64} image records; its host adapter writes those bytes to temporary host files before invoking the path-oriented engine seam. Resumed image DTOs carry stable sources rather than matching prompt text.', 'RunInput.images 是宿主路径数组。ClientCommand SendPrompt 使用内联 {media_type, base64} 图像记录；宿主适配器先将字节写入临时宿主文件，再调用面向路径的引擎接口。恢复后的 image DTO 携带稳定来源，不按提示文本匹配。'),
          t('Model/provider capability determines whether input is native vision or can use the configured vision delegation path. File readability, size/type handling and provider encoding are separate checks. Video, PDF and arbitrary attachments are not implied by RunInput.images: use the host/tool surface that explicitly supports their processing.', '模型／供应商能力决定原生视觉输入或已配置的视觉委托路径。文件可读性、大小／类型处理与供应商编码是独立检查。RunInput.images 并不隐含视频、PDF 或任意附件支持，应使用明确支持处理的宿主／工具接口。'),
        ],
      },
      {
        id: 'audio-routing', title: t('Device, offline and cloud audio', '设备、离线与云端音频'),
        paragraphs: [
          t('harness_runtime::audio re-exports the independent audio-runtime composition and shared AudioService contracts. UnifiedAudioService owns per-operation routes, available device/offline services and an optional cloud host. Automatic chooses a supported system backend, then an installed compatible offline backend; it never selects cloud implicitly. Capture, recording and playback remain native resources.', 'harness_runtime::audio 重新导出独立 audio-runtime 组合与共享 AudioService 契约。UnifiedAudioService 管理逐操作路由、可用设备／离线服务和可选 cloud host。Automatic 先选择支持的系统后端，再选已安装兼容离线后端，不隐式选云端。采集、录音与播放仍属于原生资源。'),
          t('Explicit Cloud routes bind a trusted exact session profile/account or an explicit profile, with an audio-specific model. A model-supplied tool argument does not select credential identity. update_settings replaces routes atomically; in-flight calls retain their selected snapshot. Advertised tool actions follow current owner-specific capabilities and readiness.', '显式 Cloud 路由绑定可信的精确会话 profile／账户或显式 profile，并选择音频专用模型。模型提供的工具参数不能选择凭据身份。update_settings 原子替换路由，已执行调用保留选定快照。声明的工具动作遵循当前 owner 专属能力与就绪状态。'),
        ],
        code: [{ label: t('Rust · compose local automatic audio', 'Rust · 组合本地自动音频'), language: 'rust', verification: 'compile', exampleId: 'harness-audio-composition', code: 'use std::sync::Arc;\nuse harness_runtime::audio::{AudioRoutingSettings, AudioService, UnifiedAudioService};\n\nfn local_audio(device: Arc<dyn AudioService>) -> UnifiedAudioService {\n    UnifiedAudioService::new(\n        device,\n        Vec::new(),\n        None,\n        AudioRoutingSettings::default(),\n    )\n}' }],
      },
      {
        id: 'host-wiring', title: t('Audio availability is host supplied', '音频可用性由宿主提供'),
        paragraphs: [
          t('DesktopConfig.audio is injected by the Bridge composition over its connection. CLI, TUI and offline desktop factories leave it absent, so audio tools are absent there. Android/iOS hosts implement native callbacks and capability snapshots; native microphone permission, installed models and resource readiness determine which operations the session may advertise.', 'DesktopConfig.audio 由 Bridge 组合通过连接注入。CLI、TUI 与离线桌面 factory 不提供，因此没有音频工具。Android／iOS 宿主实现原生回调和能力快照；原生麦克风权限、已安装模型及资源就绪状态决定会话可声明哪些操作。'),
          t('AudioOwner and operation IDs correlate resources to the active session. Session switch and engine disposal end owned audio and recording handles with bounded cleanup. Provider profile/account selection is rechecked at the boundary; stale owners cannot reuse a new session’s audio credentials.', 'AudioOwner 与 operation ID 将资源关联到活动会话。会话切换和引擎销毁以有界清理结束所属音频及 recording handle。供应商 profile／账户选择在边界重新检查，过期 owner 不能复用新会话音频凭据。'),
        ],
      },
      {
        id: 'realtime-owner', title: t('Realtime session ownership', '实时会话归属'),
        paragraphs: [
          t('Realtime integrates an SDK session/control/event driver with a host-owned audio agent run. Native capture and playback belong to the host; provider event decoding and realtime transport belong to llm-client. Runtime commands expose context and start/input/stop operations through the shared client contract.', '实时能力将 SDK session／control／event driver 与宿主音频 Agent run 集成。原生采集与播放属于宿主；供应商事件解码和实时传输属于 llm-client。Runtime 命令经共享客户端契约提供上下文及 start／input／stop 操作。'),
          t('Mobile realtime owner guards admission and cleanup: a conflicting NewSession is rejected while the owned run is active; permission responses use the same bound gate. Stop/cancel closes streams and releases native resources before session replacement. Transport closure, audio failure and cancellation need explicit outcomes rather than silently ending capture.', '移动 realtime owner 约束准入与清理：所属 run 活动时拒绝冲突的 NewSession；权限响应使用同一绑定 gate。Stop／cancel 关闭流并释放原生资源后才能替换会话。传输关闭、音频失败与取消需明确结果，不能静默结束采集。'),
        ],
      },
      {
        id: 'audio-acceptance', title: t('Verification boundaries', '验证边界'),
        code: [{ label: t('Audio routing and injected-host checks', '音频路由与注入宿主检查'), language: 'bash', verification: 'manual', code: 'cargo test --locked -p audio-runtime --lib\ncargo test --locked -p harness-runtime --features desktop --test audio_injection\ncargo test --locked -p harness-runtime --features mobile --test realtime_owner' }],
        bullets: [t('Existing tests use routing fixtures and mock realtime drivers to validate owner admission, capability selection, permission routing and cleanup. Compile-only examples establish API availability, not sound output.', '现有测试用路由 fixture 与模拟实时 driver 验证 owner 准入、能力选择、权限路由和清理。仅编译示例证明接口可用，不证明声音输出。'), t('Capture/playback latency, interruption by phone calls, Bluetooth routes, permissions, offline model quality and actual cloud audio/realtime requests require device/service acceptance. Preserve model/profile, OS build and resource cleanup evidence.', '采集／播放延迟、电话中断、蓝牙路由、权限、离线模型质量及真实云端音频／实时请求需设备／服务验收。保留模型／profile、系统版本和资源清理证据。')],
      },
    ],
  },
  {
    id: 'runtime-visualization', group: 'system', title: t('Inline visualization integration', '内联可视化集成'),
    description: t('Publish immutable visualization revisions and mount them through one shared authorized service on Electron, Android and iOS.', '发布不可变可视化 revision，并在 Electron、Android 与 iOS 中经同一授权服务挂载。'),
    sourceUrl,
    sources: sources('crates/runtime/src/inline_visualization.rs', 'crates/runtime/src/desktop_visualization.rs', 'crates/runtime/src/mobile/visualization_host.rs', 'crates/tools/ui/src/inline_visualization.rs', 'crates/visualization/src/store.rs', 'crates/visualization/src/service.rs', 'crates/visualization/src/document.rs', 'crates/visualization/src/context.rs', 'crates/visualization/src/checks.rs', 'crates/runtime/tests/inline_visualization.rs', 'crates/runtime/tests/mobile/visualization_host.rs'),
    sections: [
      {
        id: 'host-opt-in', title: t('Capability and host opt-in', '能力与宿主显式启用'),
        paragraphs: [
          t('Desktop/mobile compositions opt in with inline_visualization only when the host WebView can render the widgets. The flag registers both Visualization and its visualize skill. CLI and headless leave it off, so neither capability is advertised. A browser-like rendering surface is an explicit prerequisite.', '桌面／移动组合仅在宿主 WebView 能呈现 widget 时通过 inline_visualization 显式启用。该 flag 同时注册 Visualization 与 visualize skill。CLI 和 headless 关闭它，两种能力均不声明。浏览器式呈现界面是显式前提。'),
          t('The runtime shares one store per config-home in the process so the publishing tool and host service serialize writers consistently. Hosts forward scheme requests and shell messages; the shared service owns routing, authorization, document assembly, CSP and state comparison-and-swap.', 'runtime 在进程中为每个 config-home 共享一个 store，让发布工具与宿主服务一致地串行化写入。宿主转发 scheme 请求与 shell 消息；共享服务负责路由、授权、文档组装、CSP 与状态比较并交换。'),
        ],
      },
      {
        id: 'revision-storage', title: t('Immutable revisions and confirmed state', '不可变 revision 与确认状态'),
        paragraphs: [
          t('Storage is rooted at <config-home>/visualizations/<root-session-uuid>, with index.json, immutable revisions/<id>/<rev>.html and versioned state/<id>/<rev>.json. Writes use rooted atomic write/sync/rename and are acknowledged only after success. Advisory locking serializes writers across processes.', '存储根为 <config-home>/visualizations/<root-session-uuid>，包含 index.json、不可变 revisions/<id>/<rev>.html 和版本化 state/<id>/<rev>.json。写入使用 rooted 原子 write／sync／rename，仅成功后确认。Advisory lock 在跨进程间串行化写入。'),
          t('Confirmed widget state keeps modelContent and privateContent separate. The combined state limit is 16 KiB; session fragment storage is limited to 64 MiB and each visualization ID to 50 revisions. Validation/quota/storage failures return explicit errors before the UI may claim a saved state.', '确认的 widget 状态区分 modelContent 与 privateContent。合并状态上限 16 KiB；会话 fragment 存储上限 64 MiB，每个可视化 ID 最多 50 revision。校验／配额／存储失败返回明确错误，之后 UI 不能宣称已保存。'),
        ],
      },
      {
        id: 'mount-lifecycle', title: t('Authorized mount lifecycle', '授权挂载生命周期'),
        steps: [
          t('Ask VisualizationService to mount a root-session revision with theme, locale and expansion state. It returns a capability token, generation and document URL.', '请求 VisualizationService 按 root-session revision、主题、语言与展开状态挂载。它返回 capability token、generation 与文档 URL。'),
          t('Forward the WebView scheme request to the shared service and preserve its status/CSP headers. Do not bypass the service by reading arbitrary fragment files from the UI.', '将 WebView scheme 请求转发共享服务并保留其 status／CSP header。UI 不能通过读取任意 fragment 文件绕过服务。'),
          t('Relay state writes with the live mount identity and expected version. A stale mount or conflicting version is rejected; use returned winning state on conflict.', '使用活动挂载标识与预期版本转发状态写入。过期挂载或版本冲突被拒绝；冲突时采用返回的胜出状态。'),
          t('Unmount on replacement or session transition. Session deletion and orphan sweep clean storage through the owning runtime rather than arbitrary UI filesystem deletion.', '替换或会话切换时卸载。会话删除及孤儿 sweep 经所属 runtime 清理存储，不由 UI 任意删除文件系统。'),
        ],
      },
      {
        id: 'followup-context', title: t('Follow-up context is snapshotted at send', '后续上下文在发送时取快照'),
        paragraphs: [
          t('A SendPrompt may carry visualization_context {id, revision}. The runtime reads that revision’s confirmed modelContent when the prompt is sent/enqueued and fences it ahead of the typed text. Unknown session/id/revision leaves the original text unchanged. Widget privateContent is not promoted into model context.', 'SendPrompt 可携带 visualization_context {id, revision}。runtime 在提示发送／排队时读取该 revision 已确认 modelContent，并在输入文本前加围栏。未知 session／id／revision 保持原文本不变。Widget privateContent 不提升到模型上下文。'),
        ],
        code: [{ label: t('ClientCommand · follow up on revision 2', 'ClientCommand · 继续 revision 2'), language: 'json', verification: 'excerpt', code: '{\n  "type": "send_prompt",\n  "text": "Explain the selected option",\n  "images": [],\n  "visualization_context": {\n    "id": "viz-example",\n    "revision": 2\n  }\n}' }],
        note: t('Replace the example ID with an ID actually returned by publication. The field does not authorize reading a different conversation’s visualization, and an invented ID simply contributes no follow-up context.', '将示例 ID 替换为实际发布返回的 ID。该字段不授权读取其他对话可视化，虚构 ID 不会贡献后续上下文。'),
      },
      {
        id: 'visualization-acceptance', title: t('Cross-platform acceptance', '跨平台验收'),
        code: [{ label: t('Shared store and host contracts', '共享存储与宿主契约'), language: 'bash', verification: 'manual', code: 'cargo test --locked -p visualization --lib\ncargo test --locked -p harness-runtime --features desktop --test inline_visualization\ncargo test --locked -p harness-runtime --features desktop,mobile --test visualization_host' }],
        bullets: [t('Tests exercise revision storage, token/generation authorization, stale state rejection, quotas and shared host behavior. Verify theme/locale, collapsed and expanded mounts and follow-up context against copied prompts.', '测试覆盖 revision 存储、token／generation 授权、过期状态拒绝、配额及共享宿主行为。按实际复制提示验证主题／语言、折叠与展开挂载及后续上下文。'), t('Electron custom scheme and iOS/Android WebView message routing, touch behavior, accessibility and CSP enforcement still need actual host UI acceptance. A shared Rust test does not prove all three native viewers.', 'Electron 自定义 scheme 和 iOS／Android WebView 消息路由、触控、无障碍及 CSP 约束仍需实际宿主 UI 验收。共享 Rust 测试不能证明三端原生 viewer 均通过。')],
      },
    ],
  },
];
