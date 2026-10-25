import type { DocPage, DocText } from './types';

const t = (en: string, zh: string): DocText => ({ en, zh });
const sources = (...paths: string[]) => paths.map((path) => ({ repo: 'harness-runtime' as const, path }));
const sourceUrl = 'https://github.com/lingxi-coder/harness-runtime';

// Original task-oriented guidance, grounded in the linked LingXi source.
export const agentWorkflowPages: DocPage[] = [
  {
    id: 'agent-sdk-guide', group: 'harness', title: t('Choose an Agent integration', '选择 Agent 集成方式'),
    description: t('Map familiar Agent SDK concepts to LingXi’s actual Rust runtime, model client and product protocol, then choose the smallest useful integration.', '将熟悉的 Agent SDK 概念对应到灵犀实际的 Rust 运行时、模型客户端与产品协议，选择适合任务的集成层。'),
    sourceUrl, sources: [...sources('crates/runtime/src/api.rs', 'crates/runtime/src/lib.rs', 'crates/runtime/src/desktop/sdk.rs', 'crates/runtime/src/mobile/host.rs', 'crates/client/src/lib.rs', 'crates/client/src/protocol/commands.rs'), { repo: 'llm-client', path: 'src/lib.rs' }, { repo: 'lingxi-app', path: 'packages/bridge-client/src/client.ts' }, { repo: 'lingxi-app', path: 'apps/bridge-server/src/server.rs' }, { repo: 'lingxi-app', path: 'apps/bridge-server/src/router.rs' }],
    sections: [
      { id: 'pick-the-layer', title: t('Start with the operation you need', '从你要完成的操作出发'), tables: [{ headers: [t('Your application needs', '应用需要'), t('Use', '选用'), t('What you provide', '你需要提供')], rows: [
        [t('Call a model or consume model deltas', '调用模型或消费模型增量'), t('LLM Client', 'LLM Client'), t('Provider configuration, request, credential and response handling.', '供应商配置、请求、凭据和响应处理。')],
        [t('Run a tool-using agent in a Rust host', '在 Rust 宿主中运行会调用工具的 Agent'), t('Harness Runtime', 'Harness Runtime'), t('An assembled runtime, output destination, permission gate and lifecycle owner.', '已组装运行时、输出目标、权限 gate 和生命周期负责人。')],
        [t('Build a UI for an existing LingXi host', '为既有灵犀宿主构建界面'), t('ClientCommand / ClientEvent and product Bridge', 'ClientCommand／ClientEvent 与产品 Bridge'), t('Authenticated transport, command dispatch, event rendering and interactive replies.', '可信传输、命令分发、事件呈现和交互回复。')],
        [t('Run commands or a PTY on Android/iOS', '在 Android／iOS 运行命令或 PTY'), t('Mobile Linux Runtime', 'Mobile Linux Runtime'), t('Verified resources, platform paths and process ownership.', '经过验证的资源、平台路径与进程归属。')],
      ] }], paragraphs: [t('A direct model request does not execute tool calls on your filesystem. Harness adds the agent loop through its host composition. Its public facade gives access to an existing session; it is not a process launcher or a hosted service.', '直接模型请求不会替你在文件系统执行工具调用。Harness 通过宿主组合提供 Agent 循环；公共门面访问已存在会话，并不充当进程启动器或托管服务。')] },
      { id: 'concept-map', title: t('Translate concepts before translating code', '先对应概念，再迁移代码'), tables: [{ headers: [t('Familiar concept', '熟悉的概念'), t('LingXi integration point', '灵犀集成点'), t('Migration decision', '迁移时的决定')], rows: [
        [t('Run an agent query', '运行 Agent query'), t('SessionHandle::run(RunInput, CancellationToken)', 'SessionHandle::run(RunInput, CancellationToken)'), t('Supply an assembled session; keep the terminal Result separate from streamed output.', '先提供已组装会话；区分终态 Result 和流式输出。')],
        [t('Iterate assistant/tool messages', '迭代助手／工具消息'), t('OutputStream or AdapterOutputStream → ClientEventSink', 'OutputStream 或 AdapterOutputStream → ClientEventSink'), t('Install output during composition, before the first run.', '首次 run 前在组合阶段安装输出目标。')],
        [t('Resume/fork a conversation', '恢复／分支对话'), t('ResumeSession; ForkSession on MobileHost', 'ResumeSession；MobileHost 上的 ForkSession'), t('Resume confirms a mounted session. MobileHost fork confirms branch creation; mount the returned branch before sending work to it. Desktop Bridge does not currently route ForkSession.', 'Resume 确认会话已挂载；MobileHost 的 Fork 确认分支已创建，向该分支提交任务前还需挂载返回的新标识。当前 Desktop Bridge 未接通 ForkSession。')],
        [t('Approve a tool call', '批准工具调用'), t('PermissionGate; AdapterPermissionGate for protocol hosts', 'PermissionGate；协议宿主可用 AdapterPermissionGate'), t('Route the real user decision to its request owner using request_id.', '使用 request_id 将真实用户决定交给请求所属执行。')],
        [t('Tools, skills, hooks and MCP', '工具、Skills、Hooks 与 MCP'), t('Host-owned registries and their lifecycle', '宿主拥有的注册表及生命周期'), t('Verify activation and dependencies in the composition you ship.', '在实际发行组合中核对激活与依赖。')],
      ] }], note: t('This is a workflow map, not an API compatibility claim. Claude SDK option objects, TypeScript imports, Python clients and authentication are not interchangeable with LingXi’s Rust API.', '本表对应使用流程，不代表 API 兼容。Claude SDK 选项对象、TypeScript 导入、Python 客户端和认证方式不能直接替代灵犀 Rust API。') },
      { id: 'host-contract', title: t('Write down the host contract first', '先写清宿主契约'), steps: [
        t('Choose desktop, mobile or injected SessionService/LifecycleService implementations; confirm required features and native resources.', '选择 desktop、mobile 或注入的 SessionService／LifecycleService 实现，并确认所需 feature 和原生资源。'),
        t('Define where text, tools, questions, permissions and terminal outcomes go. Related adapters should share the same connection/session owner.', '确定文本、工具、问题、权限与终态的交付位置；相关适配器应使用一致的连接／会话归属。'),
        t('Set working directory, tool scope, provider/model and credential ownership before admitting work.', '准入工作前设置工作目录、工具范围、供应商／模型及凭据归属。'),
        t('Cancel a turn and await cleanup separately from draining the host when the application exits.', '轮次取消并等待清理，与应用退出时排空宿主，应分别处理。'),
      ], links: [{ label: t('Harness consumer manifest', 'Harness 消费者清单'), href: '/docs/harness#installation' }, { label: t('First turn recipe', '第一个轮次配方'), href: '/docs/agent-first-turn' }, { label: t('Model-only quickstart', '仅调用模型的快速开始'), href: '/docs/llm-client#first-request' }] },
      { id: 'migrate-one-slice', title: t('Migrate one complete interaction', '迁移一条完整交互'), bullets: [
        t('Start with a text-only turn including normal completion and failure. Then add streaming presentation and one approval-gated tool.', '先实现纯文本轮次，覆盖正常结束与失败；再接流式呈现和一个需要审批的工具。'),
        t('Port option intent, not names. Model choice, tool allowlists and session continuation must reach their owning host services rather than invented RunInput fields.', '迁移选项的意图而非名称。模型选择、工具允许列表与会话继续，应进入对应宿主服务，而非凭空增加 RunInput 字段。'),
        t('Verify images, tool results, cancelled approvals, follow-up context and shutdown on the target host before expanding the migration.', '扩大迁移范围前，在目标宿主核对图像、工具结果、审批取消、追问上下文与关闭。'),
      ] },
      { id: 'use-the-reference', title: t('Read recipes and signatures together', '配合阅读配方与签名'), paragraphs: [t('Recipes describe a complete interaction; Harness chapters describe API signatures; the declaration index locates implementation. Compiler-generated Rustdoc resolves methods, feature gates and trait bounds for the built profile. A raw declaration search does not resolve those conditions.', '配方介绍完整交互，Harness 章节提供 API 签名，声明索引用于定位实现。编译器生成的 Rustdoc 解析构建配置的方法、feature 条件与 trait 约束；原始声明检索不能替代这些解析。')], links: [{ label: t('Harness API chapters', 'Harness API 章节'), href: '/docs/harness' }, { label: t('API declarations and Rustdoc', 'API 声明与 Rustdoc'), href: '/docs/api-reference' }, { label: t('Claude Agent SDK topic reference', 'Claude Agent SDK 主题参考'), href: 'https://code.claude.com/docs/en/agent-sdk/overview' }] },
    ],
  },
  {
    id: 'agent-first-turn', group: 'harness', title: t('Run your first Agent turn', '运行第一个 Agent 轮次'),
    description: t('Go from an assembled Harness to a prompt, a terminal result, follow-up context and a clean shutdown.', '从已组装 Harness 出发，完成提示词、终态结果、后续上下文与有序关闭。'),
    packageName: 'harness-runtime', sourceUrl, sources: sources('crates/runtime/src/api.rs', 'crates/runtime/src/lib.rs', 'crates/runtime/src/desktop/sdk.rs', 'crates/core/src/host/orchestrator.rs'),
    sections: [
      { id: 'prerequisites', title: t('Prepare a real runtime', '准备真实运行时'), paragraphs: [t('This recipe starts with &Harness from your host. HarnessBuilder wraps injected services; it does not choose a model, discover tools or create a workspace. Desktop build_harness(config, output, permissions).await assembles the existing desktop runtime and returns the facade.', '本配方从宿主提供的 &Harness 开始。HarnessBuilder 包装注入服务，不会自行选择模型、发现工具或创建工作空间。桌面的 build_harness(config, output, permissions).await 组装既有桌面运行时并返回门面。')], steps: [t('Use the complete consumer manifest and the same SDK source identities as the host.', '使用完整消费者清单，并保持与宿主一致的 SDK 来源。'), t('Connect output and permission services before building the composition.', '构建组合前接通输出与权限服务。'), t('Configure the workspace and an available provider/model. Begin with text and no image paths.', '配置工作空间及可用供应商／模型。首次验证使用纯文本和空图像路径。')], links: [{ label: t('Complete consumer manifest', '完整消费者清单'), href: '/docs/harness#installation' }, { label: t('Desktop composition example', '桌面组合示例'), href: '/docs/harness-lifecycle#platform-assembly' }] },
      { id: 'submit-and-inspect', title: t('Submit a prompt and inspect state', '提交提示词并读取状态'), code: [{ label: t('Rust · function for an assembled host', 'Rust · 已组装宿主中的函数'), language: 'rust', verification: 'manual', exampleId: 'harness-guide-first-turn', code: `use harness_runtime::{CancellationToken, HandleError, Harness, RunInput, TurnOutcome};

async fn run_and_inspect(
    harness: &Harness,
    prompt: String,
) -> Result<(TurnOutcome, usize), HandleError> {
    let session = harness.session();
    let outcome = session.run(
        RunInput { prompt, images: Vec::new() },
        CancellationToken::new(),
    ).await?;
    let message_count = session.transcript().await.len();
    Ok((outcome, message_count))
}` }], tables: [{ headers: [t('Value', '值'), t('Meaning', '含义')], rows: [
        [t('RunInput.prompt', 'RunInput.prompt'), t('User text passed to the agent execution path.', '交给 Agent 执行路径的用户文本。')],
        [t('RunInput.images', 'RunInput.images'), t('Host-side paths resolved by the composition, not URLs or inline base64.', '由组合解析的宿主路径，不是 URL 或内联 base64。')],
        [t('Result<TurnOutcome, HandleError>', 'Result<TurnOutcome, HandleError>'), t('Terminal execution status; streaming text and tools use the output destination.', '执行终态；流式文本与工具信息使用输出目标。')],
        [t('session.transcript()', 'session.transcript()'), t('Ordered snapshot of the mounted conversation, useful for inspection or rebuilding a view.', '已挂载对话的有序快照，可用于检查或重建视图。')],
      ] }] },
      { id: 'classify-completion', title: t('Handle every terminal case', '处理每种终态'), tables: [{ headers: [t('Result', '结果'), t('Application response', '应用处理')], rows: [
        [t('Ok(EndTurn)', 'Ok(EndTurn)'), t('Finish this interaction and retain the session for a follow-up.', '结束本次交互，保留会话以接收追问。')],
        [t('Ok(MaxTurns)', 'Ok(MaxTurns)'), t('Explain that the configured turn budget was reached; preserve partial work.', '说明已达到配置的轮次预算，保留部分工作。')],
        [t('Ok(Cancelled)', 'Ok(Cancelled)'), t('Clear the busy state after run unwinds; retain already produced output.', 'run 完成退出后清除忙碌状态，保留已经产生的输出。')],
        [t('Err(ActionFailed(reason))', 'Err(ActionFailed(reason))'), t('Show the failure reason rather than an empty successful answer.', '显示失败原因，而不是空的成功答案。')],
        [t('Err(Unimplemented(reason))', 'Err(Unimplemented(reason))'), t('Check selected host capabilities and wiring before retrying.', '重试前检查选定宿主能力与接线。')],
      ] }] },
      { id: 'continue-context', title: t('Ask a follow-up sequentially', '顺序继续追问'), paragraphs: [t('Call run again on the same mounted session only after the prior run completes. The composition retains conversation context. Cloning SessionHandle shares access to the same session; it neither creates another conversation nor establishes safe overlapping turns.', '上一轮 run 完成后，才在同一已挂载会话上再次调用 run。组合保留对话上下文。克隆 SessionHandle 只是共享同一会话访问权，不创建其他对话，也不保证重叠轮次安全。')], code: [{ label: t('Rust · sequential follow-up excerpt', 'Rust · 顺序追问片段'), language: 'rust', verification: 'excerpt', code: `let session = harness.session();
session.run(RunInput {
    prompt: "Explain the repository layout".into(),
    images: Vec::new(),
}, CancellationToken::new()).await?;

session.run(RunInput {
    prompt: "Which component owns persistence?".into(),
    images: Vec::new(),
}, CancellationToken::new()).await?;` }] },
      { id: 'close-the-host', title: t('Finish the host lifecycle', '完成宿主生命周期'), paragraphs: [t('Do not shut down after each prompt when the application needs follow-ups. At actual exit, stop admission, finish or cancel active work and await it, then call harness.shutdown(). An incomplete ShutdownReport retains errors and publication receipts for a sequential retry.', '应用需要继续对话时，不应在每个提示词后关闭运行时。真正退出时，先停止准入，完成或取消当前工作并等待结束，再调用 harness.shutdown()。不完整的 ShutdownReport 保留错误和发布回执，供顺序重试。')], code: [{ label: t('Rust · shutdown report excerpt', 'Rust · 关闭报告片段'), language: 'rust', verification: 'excerpt', code: `let report = harness.shutdown().await;
if !report.complete {
    // Retain report.errors and report.publications.
    // Resolve the outstanding barrier before a sequential shutdown retry.
}` }], links: [{ label: t('Cancellation and recovery', '取消与恢复'), href: '/docs/agent-session-workflows' }, { label: t('Shutdown contract', '关闭契约'), href: '/docs/harness-lifecycle#shutdown' }] },
    ],
  },
  {
    id: 'agent-ui-streaming', group: 'harness', title: t('Connect streamed output to a UI', '把流式输出接到界面'),
    description: t('Wire one event destination and render text, tool activity and interactive requests without confusing message completion with turn completion.', '接通一个事件目标，呈现文本、工具活动和交互请求，并区分消息完成与轮次完成。'),
    sourceUrl, sources: sources('crates/runtime/src/api.rs', 'crates/client/src/adapter/sink.rs', 'crates/client/src/adapter/output_stream.rs', 'crates/client/src/adapter/permission_gate.rs', 'crates/client/src/adapter/turn.rs', 'crates/client/src/protocol/events.rs', 'crates/client/src/protocol/permission.rs'),
    sections: [
      { id: 'wire-before-run', title: t('Install output before the first run', '首次运行前安装输出目标'), paragraphs: [t('Harness run does not return a stream iterator. The host supplies OutputStream during composition. For a DTO-based UI, implement ClientEventSink and wrap it with AdapterOutputStream::new. Connect the turn emitter and terminal permission notifications to the same event destination.', 'Harness 的 run 不返回流迭代器。宿主在组合阶段提供 OutputStream。采用 DTO 的界面可实现 ClientEventSink，并以 AdapterOutputStream::new 包装；轮次 emitter 与权限终态通知也应接到同一事件目标。')], apis: [{ name: 'ClientEventSink::emit', signature: 'async fn emit(&self, event: client::protocol::events::ClientEvent)', description: t('Receives one owned, lowered event. Enqueue transport work; avoid rendering or blocking network I/O on the agent task.', '接收一个已转换且拥有自身数据的事件。将传输工作入队，避免在 Agent 任务内直接渲染或执行阻塞网络 I/O。') }], links: [{ label: t('Adapter setup', '适配器接线'), href: '/docs/harness-client#output-adapter' }] },
      { id: 'project-events', title: t('Project events into stable rows', '把事件投影为稳定条目'), tables: [{ headers: [t('Event', '事件'), t('UI update', '界面更新'), t('Important distinction', '关键区别')], rows: [
        [t('TurnStarted', 'TurnStarted'), t('Mark the admitted action active; retain its optional turn_id.', '标记已准入动作为活动状态，保留可选 turn_id。'), t('Correlate it with the submitted user action.', '与提交的用户动作关联。')],
        [t('TextDelta', 'TextDelta'), t('Append to the current assistant message and batch view updates.', '追加到当前助手消息，并批量更新视图。'), t('A delta is not a complete message.', '增量不是完整消息。')],
        [t('ToolUseStarted / ToolHeartbeat', 'ToolUseStarted／ToolHeartbeat'), t('Create/update activity using the tool-use id.', '按工具调用 id 创建／更新活动。'), t('input_json is a JSON string, not an already parsed object.', 'input_json 是 JSON 字符串，不是已解析对象。')],
        [t('ToolUseResult', 'ToolUseResult'), t('Attach result_json, is_error and display to the matching id.', '按对应 id 关联 result_json、is_error 与 display。'), t('A failed tool need not terminate the entire turn.', '单个工具失败不一定结束整个轮次。')],
        [t('MessageComplete', 'MessageComplete'), t('Finish one message; apply the optional full message snapshot.', '结束一条消息，并应用可选完整消息快照。'), t('The agent can continue another model/tool iteration.', 'Agent 可以继续下一次模型／工具迭代。')],
        [t('TurnEnded', 'TurnEnded'), t('Apply terminal outcome, stop reason and cost.', '应用轮次终态、停止原因与成本。'), t('Preserve partial output on cancellation or budget exhaustion.', '取消或预算耗尽时保留部分输出。')],
        [t('Error', 'Error'), t('Show its typed kind and message and reconcile the failed interaction.', '呈现类型化 kind 与 message，并收敛失败交互。'), t('The turn wrapper emits Error on failure; do not wait forever for a success-only marker.', '轮次包装器在失败时发送 Error，不能永久等待只在成功路径出现的标记。')],
      ] }], paragraphs: [t('AdapterOutputStream can emit message boundaries during execution; TurnEventEmitter can also synthesize a complete message snapshot after the pumped turn succeeds. Do not assume every MessageComplete means the run is finished, or that the final snapshot is a new independent assistant answer. Reconcile message identity/snapshots using the host contract.', 'AdapterOutputStream 可在执行中发送消息边界；流式轮次成功后，TurnEventEmitter 还可合成完整消息快照。不能假定每个 MessageComplete 都代表 run 已结束，也不能将最终快照视为新的独立助手答案。应按宿主契约协调消息标识与快照。')] },
      { id: 'wire-shape', title: t('Read the wire shape correctly', '正确读取线协议形状'), code: [{ label: t('JSON · independent event objects', 'JSON · 独立事件对象'), language: 'json', verification: 'excerpt', code: `{"type":"turn_started","turn_id":12}
{"type":"text_delta","text":"I will inspect the project."}
{"type":"message_complete","stop_reason":"tool_use"}` }], paragraphs: [t('These are event objects, not a request batch, transport frame or complete turn. Client enums are non-exhaustive and use snake_case type tags. Keep a fallback path for additive events; native bindings must match the host’s protocol layout.', '这里是事件对象，不是请求批次、传输帧或完整轮次。Client enum 使用 snake_case type 标记且为非穷尽类型。应为新增事件保留回退路径；原生绑定必须匹配宿主协议布局。')] },
      { id: 'interactive-output', title: t('Keep approvals and questions interactive', '让审批和问题保持可交互'), paragraphs: [t('A tool permission request is a dedicated PermissionRequest DTO delivered through PermissionRequestSink, not a PermissionRequest variant of ClientEvent. When configured, the gate emits PermissionRequestResolved via the event sink. AskUserQuestion has its own event and answer command. Render each interaction by request_id and owner rather than appending ordinary assistant text.', '工具权限请求是经 PermissionRequestSink 交付的独立 PermissionRequest DTO，不是 ClientEvent 的 PermissionRequest variant。配置后，gate 经事件 sink 发送 PermissionRequestResolved。AskUserQuestion 则有自己的事件和回答命令。每种交互都应按 request_id 与归属呈现，而非追加为普通助手文本。')], steps: [t('Display the request without completing the turn.', '显示请求，不将轮次标记为完成。'), t('Send the matching decision or answer over the active host connection.', '经活动宿主连接发送对应决定或回答。'), t('Remove the request on authoritative resolution, including expiry or cancellation before the click.', '收到权威终态后移除请求，包括用户点击前发生的过期或取消。')], links: [{ label: t('Approval round-trip', '审批往返'), href: '/docs/agent-tool-workflows#approval-round-trip' }] },
      { id: 'reconnect-and-rebuild', title: t('Restore a snapshot before live projection', '实时投影前先恢复快照'), paragraphs: [t('A new output adapter does not replay earlier deltas. Rebuild conversation state through the host’s resume/snapshot path, then apply events for the mounted session. Keep session ids, turn ids and tool-use ids separate. If the transport has epoch/sequence metadata, follow its replay rules rather than deduplicating equal text.', '新的输出适配器不会重放此前增量。先经宿主恢复／快照路径重建对话，再应用已挂载会话的实时事件。会话 id、turn id 与工具调用 id 应分开。传输提供 epoch／sequence 时，遵循它的重放规则，不要任意合并相同文本。')], bullets: [t('Do not replace the transcript with only the latest delta after reconnect.', '重连后不要用最后一段增量替换完整对话。'), t('If the UI disappears, notify its lifecycle owner; do not let an unconsumed queue grow indefinitely.', '界面消失时通知生命周期负责人，不让无人消费的队列无限增长。'), t('Exercise tools, pending approvals, transport failure and normal turn completion; text alone misses these transitions.', '覆盖工具、等待审批、传输失败与正常轮次终态；只展示文本会漏掉这些转换。')], links: [{ label: t('Session recovery', '会话恢复'), href: '/docs/agent-session-workflows' }] },
    ],
  },
  {
    id: 'agent-session-workflows', group: 'harness', title: t('Continue, resume, fork and cancel', '继续、恢复、分支与取消'),
    description: t('Choose the right session operation and distinguish cancellation, saved history and active execution ownership.', '选择正确的会话操作，并区分取消、已保存历史与活动执行归属。'),
    sourceUrl, sources: [...sources('crates/runtime/src/api.rs', 'crates/client/src/protocol/commands.rs', 'crates/client/src/protocol/events.rs', 'crates/client/src/protocol/listings.rs', 'crates/runtime/src/desktop/sdk.rs', 'crates/runtime/src/mobile/host.rs', 'crates/session/src/jsonl/loader.rs'), { repo: 'lingxi-app', path: 'apps/bridge-server/src/server.rs' }, { repo: 'lingxi-app', path: 'apps/bridge-server/src/router.rs' }],
    sections: [
      { id: 'choose-operation', title: t('Match the operation to user intent', '按用户意图选择操作'), tables: [{ headers: [t('Intent', '意图'), t('Operation', '操作'), t('Identity behavior', '标识行为')], rows: [
        [t('Ask another question now', '现在继续提问'), t('Run again on the mounted SessionHandle', '在已挂载 SessionHandle 上再次 run'), t('Keep the conversation; wait for the preceding turn.', '保留对话，先等待上一轮结束。')],
        [t('Start an independent conversation', '开始独立对话'), t('NewSession', 'NewSession'), t('The host creates/mounts a session and emits SessionStarted.', '宿主创建并挂载会话，发送 SessionStarted。')],
        [t('Open saved history', '打开保存历史'), t('ResumeSession', 'ResumeSession'), t('Name a target; SessionResumed confirms the mounted id and messages.', '指定目标；SessionResumed 确认挂载 id 和消息。')],
        [t('Explore another branch', '探索另一分支'), t('ForkSession on MobileHost', 'MobileHost 上的 ForkSession'), t('MobileHost creates a new branch; creation alone does not mount it. Desktop Bridge does not currently handle this command.', 'MobileHost 创建新分支；创建成功本身不代表已挂载该分支。当前 Desktop Bridge 未处理此命令。')],
        [t('Stop the current work only', '只停止当前工作'), t('CancellationToken / Cancel', 'CancellationToken／Cancel'), t('End a turn without deleting history or shutting down the host.', '结束轮次，不删除历史，也不关闭宿主。')],
      ] }] },
      { id: 'resume-over-the-protocol', title: t('Resume through the host protocol', '经宿主协议恢复'), code: [{ label: t('JSON · send commands in this order', 'JSON · 按顺序分别发送命令'), language: 'json', verification: 'excerpt', code: `{"type":"list_sessions","limit":20}
{"type":"resume_session","session_id":"<saved-session-id>"}
{"type":"send_prompt","text":"Continue the investigation","images":[],"turn_id":21}` }], steps: [t('List sessions and let the user choose a returned session id.', '列出会话，让用户选择返回的会话 id。'), t('Send ResumeSession and wait for SessionResumed before submitting follow-up work.', '发送 ResumeSession，收到 SessionResumed 后再提交追问。'), t('Rebuild messages from the resumed snapshot and retain its authoritative identity.', '用恢复快照重建消息，并保留权威标识。'), t('SendPrompt targets the mounted connection. Do not add session_id to ordinary live prompt/cancel commands.', 'SendPrompt 作用于已挂载连接，不要给普通实时提示词／取消命令添加 session_id。')], note: t('The objects illustrate ordering, not batching. The public Harness facade has no resume(session_id) method; the host owns mounting and persistence.', '这些对象说明顺序，不表示批处理。公共 Harness 门面没有 resume(session_id) 方法；挂载与持久化由宿主负责。') },
      { id: 'fork-a-conversation', title: t('Fork without overwriting the source', '分支而不覆盖原对话'), code: [{ label: t('JSON · MobileHost fork into code mode', 'JSON · MobileHost 分支到 code 模式'), language: 'json', verification: 'excerpt', code: `{
  "type": "fork_session",
  "session_id": "<source-session-id>",
  "target_mode": "code"
}` }], paragraphs: [t('This recipe follows the current MobileHost implementation. Desktop Bridge does not route ForkSession, so a desktop client must not offer this command or wait for SessionForked until its host implements it. The shared protocol type alone does not establish support.', '本配方对应当前 MobileHost 实现。Desktop Bridge 未接通 ForkSession，因此桌面客户端在宿主实现前不应提供该命令，也不应等待 SessionForked。仅存在共享协议类型不能证明已支持。'), t('ForkSession accepts SessionModeDto chat or code and keeps the current session’s workspace according to its command contract. Treat its result as a new conversation identity. A source id in a lifecycle command does not change the rule that normal live commands target the mounted connection.', 'ForkSession 的 SessionModeDto 值为 chat 或 code；按命令契约保留当前会话工作空间。应将结果作为新对话标识。生命周期命令携带来源 id，并不改变普通实时命令作用于已挂载连接的规则。')], steps: [t('On MobileHost, send ForkSession and wait for SessionForked; retain the returned new branch id.', '在 MobileHost 上发送 ForkSession，等待 SessionForked，并保存返回的新分支 id。'), t('Mount that id with ResumeSession, or open the host connection for that branch. Wait for SessionResumed or the host’s authoritative mounted-session confirmation.', '通过 ResumeSession 挂载该 id，或打开宿主中属于该分支的连接；等待 SessionResumed 或宿主权威的挂载确认。'), t('Only then send a follow-up prompt. Sending a prompt immediately after creation can still target the original mounted session.', '确认后再发送追问；仅创建分支后立即发提示词，仍可能作用于原来挂载的会话。')], bullets: [t('Retain the source entry until the host confirms the new branch.', '宿主确认新分支前，保留原会话入口。'), t('Do not fork by editing saved JSONL in place or reusing the source id.', '不要通过原地编辑已保存 JSONL 或复用来源 id 来实现分支。')] },
      { id: 'cooperative-cancellation', title: t('Cancel cooperatively and await the run', '协作取消，并等待 run 结束'), code: [{ label: t('Rust · connect a stop signal to one run', 'Rust · 将停止信号接到一个 run'), language: 'rust', verification: 'manual', exampleId: 'harness-guide-cooperative-cancellation', code: `use harness_runtime::{CancellationToken, HandleError, Harness, RunInput, TurnOutcome};

async fn run_until_stopped(
    harness: &Harness,
    prompt: String,
    stop: CancellationToken,
) -> Result<TurnOutcome, HandleError> {
    let turn_cancel = CancellationToken::new();
    let session = harness.session();
    let run = session.run(
        RunInput { prompt, images: Vec::new() },
        turn_cancel.clone(),
    );
    tokio::pin!(run);
    tokio::select! {
        result = &mut run => result,
        () = stop.cancelled() => {
            turn_cancel.cancel();
            run.await
        }
    }
}` }], paragraphs: [t('Create a fresh turn token for the next run: a cancelled token remains cancelled. Over the protocol, include turn_id when cancellation must target the same user action rather than a later active turn. A terminal cancellation outcome does not roll back previously executed tools.', '下一次运行使用新的轮次令牌，因为已取消令牌会保持取消状态。协议上需要停止同一次用户操作时，应携带 turn_id，避免误取消稍后活动轮次。取消终态不回滚此前已经执行的工具。')] },
      { id: 'saved-is-not-running', title: t('Saved history is not an execution owner', '已保存历史不等于执行负责人'), tables: [{ headers: [t('Observation', '观察结果'), t('What it proves', '能证明什么'), t('Next check', '下一步检查')], rows: [
        [t('Session appears in a listing', '会话出现在列表'), t('A resumable record exists.', '存在可恢复记录。'), t('Mount it and inspect SessionResumed.', '挂载并检查 SessionResumed。')],
        [t('Partial text was saved', '部分文本已保存'), t('Some output was retained.', '部分输出已保留。'), t('Inspect terminal state and active/recovered owner.', '检查终态及活动／恢复负责人。')],
        [t('Cancellation was requested', '已请求取消'), t('A stop signal reached the host.', '宿主收到了停止信号。'), t('Await terminal completion and tool/permission cleanup.', '等待终态与工具／权限清理。')],
        [t('ShutdownReport.complete is false', 'ShutdownReport.complete 为 false'), t('A required barrier is still incomplete.', '必要屏障尚未完成。'), t('Retain errors/publications; retry sequentially.', '保留 errors／publications，顺序重试。')],
      ] }], links: [{ label: t('Storage and durable publication', '存储与持久发布'), href: '/docs/session-storage' }, { label: t('Host shutdown', '宿主关闭'), href: '/docs/harness-lifecycle#shutdown' }] },
    ],
  },
  {
    id: 'agent-tool-workflows', group: 'harness', title: t('Add a tool and handle approval', '添加工具并处理审批'),
    description: t('Take a tool from schema and host registration through a real permission round-trip, result presentation and teardown.', '让工具从 schema 与宿主注册进入真实权限往返、结果呈现和清理流程。'),
    sourceUrl, sources: sources('crates/tool-api/src/tool_trait.rs', 'crates/tool-api/src/registry.rs', 'crates/tool-api/src/context.rs', 'crates/client/src/adapter/permission_gate.rs', 'crates/client/src/protocol/permission.rs', 'crates/client/src/protocol/commands.rs', 'crates/client/src/protocol/events.rs'),
    sections: [
      { id: 'design-a-small-tool', title: t('Define one operation and its contract', '定义一个操作及其契约'), paragraphs: [t('A custom tool is a Rust Tool implementation registered with the host. Start with a narrow operation, canonical name, source-owned input schema and explicit result. Keep validation and authorization separate from the side-effecting call.', '自定义工具是注册到宿主的 Rust Tool 实现。先定义范围清晰的操作、规范名称、源码拥有的输入 schema 和明确结果。输入校验、授权与产生副作用的 call 应分开。')], tables: [{ headers: [t('Tool surface', 'Tool 接口'), t('Responsibility', '职责')], rows: [
        [t('name / input_schema / input_repair_metadata', 'name／input_schema／input_repair_metadata'), t('Declare canonical identity, accepted shape and required streamed-input repair facts.', '声明规范标识、输入形状和流式输入修复所需事实。')],
        [t('is_enabled / is_concurrency_safe', 'is_enabled／is_concurrency_safe'), t('Declare capability availability and whether this input may overlap other tool work.', '声明能力是否可用，以及输入能否与其他工具工作重叠。')],
        [t('validate_input', 'validate_input'), t('Check operation-specific requirements before side effects.', '副作用前检查操作特定条件。')],
        [t('check_permissions', 'check_permissions'), t('Compute an input/context-specific result. Being listed is not permission.', '根据输入／上下文计算权限结果；列入目录不代表已授权。')],
        [t('call / ToolProgressSender', 'call／ToolProgressSender'), t('Execute with ToolUseContext, cancellation, progress and a structured result.', '结合 ToolUseContext、取消、进度与结构化结果完成执行。')],
      ] }], links: [{ label: t('Tool contract and signatures', 'Tool 契约与签名'), href: '/docs/harness-tools#tool-contract' }] },
      { id: 'register-with-owner', title: t('Register with the lifecycle owner', '向生命周期负责人注册'), paragraphs: [t('Register builtins while the host still owns a mutable ToolRegistry. MCP tools belong to McpConnectionId partitions; plugin tools belong to PluginId partitions. Use owner-scoped removal/replacement so disconnected services or disabled plugins cannot leave stale capabilities advertised.', '宿主仍独占可变 ToolRegistry 时注册内置工具。MCP 工具属于 McpConnectionId 分区，插件工具属于 PluginId 分区。使用按归属的移除／替换路径，使断开服务或禁用插件后不会继续声明过期能力。')], code: [{ label: t('Rust · register an implemented builtin', 'Rust · 注册已实现的内置工具'), language: 'rust', verification: 'excerpt', code: `use std::sync::Arc;
use tool_api::{Tool, ToolRegistry};

fn register_host_tool(registry: &mut ToolRegistry, tool: Arc<dyn Tool>) {
    registry.register_builtin(tool);
}` }], note: t('Registration does not assemble the tool’s host dependencies or bypass session capability filters.', '注册不会组装工具所需宿主依赖，也不会绕过会话能力过滤。') },
      { id: 'approval-round-trip', title: t('Complete an approval round-trip', '完成审批往返'), code: [
        { label: t('JSON · outbound permission DTO', 'JSON · 输出的权限 DTO'), language: 'json', verification: 'excerpt', code: JSON.stringify({ request_id: 17, kind: { type: 'tool_use_confirm', tool_name: 'Bash', tool_input_json: JSON.stringify({ command: 'git status --short' }), default_allow: true } }, null, 2) },
        { label: t('JSON · approve once', 'JSON · 单次批准'), language: 'json', verification: 'excerpt', code: '{"type":"approve_permission","request_id":17,"response":{"type":"allow_once"}}' },
        { label: t('JSON · deny', 'JSON · 拒绝'), language: 'json', verification: 'excerpt', code: '{"type":"deny_permission","request_id":17}' },
      ], steps: [t('Forward PermissionRequest through PermissionRequestSink and show the real tool/input.', '经 PermissionRequestSink 转发 PermissionRequest，显示真实工具／输入。'), t('Echo request_id on the reply. The transport receive task must remain responsive while the turn waits.', '回复中回传 request_id。轮次等待时，传输接收任务必须仍可响应。'), t('Let AdapterPermissionGate::resolve find the parked request; false means unknown or already resolved.', '由 AdapterPermissionGate::resolve 查找等待请求；false 表示未知或已经完成。'), t('Remove the prompt on PermissionRequestResolved, including expired or cancelled outcomes.', '收到 PermissionRequestResolved 后移除提示，包括过期或取消终态。')] },
      { id: 'allowed-choices', title: t('Offer only host-approved choices', '只提供宿主允许的选择'), tables: [{ headers: [t('Choice/state', '选择／状态'), t('Required behavior', '应有行为')], rows: [
        [t('AllowOnce', 'AllowOnce'), t('Approve one parked invocation without creating a durable rule.', '批准一次等待调用，不创建持久规则。')],
        [t('AllowAlways', 'AllowAlways'), t('Let the gate derive a rule from the captured call. Durable persistence also needs configured paths and enabled persistence.', '由 gate 从已捕获调用派生规则。持久化还要求配置路径并启用持久化。')],
        [t('suppress_always_allow_rule', 'suppress_always_allow_rule'), t('Hide persistent allow; the gate also downgrades unsuitable responses.', '隐藏持久允许操作；gate 也会降级不适用响应。')],
        [t('auto_mode_prompt', 'auto_mode_prompt'), t('Render the engine-provided optional action; do not infer eligibility from the tool name.', '呈现引擎提供的可选动作，不按工具名自行推断资格。')],
        [t('Timeout / teardown', '超时／连接清理'), t('Resolve pending work through the gate; never leave it parked indefinitely.', '通过 gate 结束等待工作，不能永久挂起。')],
      ] }], paragraphs: [t('The adapter defaults to a 300-second request timeout and supports with_timeout. The host can cancel captured owners or drain all pending requests on teardown. A UI default selection is not proof of a user decision.', '适配器默认请求超时为 300 秒，支持 with_timeout。宿主可取消已捕获归属的请求，也可在连接清理时排空全部请求。界面默认选择不代表用户已经决定。')] },
      { id: 'test-the-call', title: t('Test dispatch, approval and cleanup', '验证调度、审批与清理'), bullets: [t('Advertise and invoke through the real dispatcher; approve once and verify one result with the same tool-use id.', '经真实调度器声明和调用；批准一次，核对同一工具调用 id 对应一条结果。'), t('Reject malformed input and deny a valid call; confirm neither performs side effects.', '拒绝无效输入并拒绝一次有效调用，确认两条路径均未执行副作用。'), t('Cancel during approval, then answer the old request; it must not revive execution.', '审批等待时取消，再回答旧请求；不能使执行复活。'), t('Disconnect MCP or disable the owner plugin and verify removal from the capability view.', '断开 MCP 或禁用所属插件，检查能力视图中工具已移除。')], links: [{ label: t('Tool API and capability filtering', '工具 API 与能力过滤'), href: '/docs/harness-tools' }, { label: t('Streamed UI events', '界面流事件'), href: '/docs/agent-ui-streaming' }] },
    ],
  },
  {
    id: 'agent-extension-recipes', group: 'harness', title: t('Compose skills, hooks and MCP', '组合 Skills、Hooks 与 MCP'),
    description: t('Choose the right extension, configure a scoped lifecycle check and verify an external capability is connected and callable.', '选择合适扩展，配置范围清晰的生命周期检查，并核对外部能力已连接且可调用。'),
    sourceUrl, sources: sources('crates/skill/src/frontmatter.rs', 'crates/skill/src/model.rs', 'crates/skill/src/registry.rs', 'crates/hooks/src/loader.rs', 'crates/hooks/src/settings_layer.rs', 'crates/hooks/src/definition.rs', 'crates/hooks/src/response.rs', 'crates/mcp/src/registry.rs', 'crates/plugin/src/manager.rs', 'crates/runtime/src/desktop/skill_loader.rs'),
    sections: [
      { id: 'choose-extension', title: t('Choose by responsibility', '按职责选择扩展'), tables: [{ headers: [t('Need', '需求'), t('Mechanism', '机制'), t('Example', '例子')], rows: [
        [t('Teach a repeatable procedure', '教授可重复流程'), t('Skill / command', 'Skill／命令'), t('Repository review steps and a required report format.', '仓库审查步骤与明确报告格式。')],
        [t('Run code or decide at a lifecycle event', '在生命周期事件运行代码或决定'), t('Hook', 'Hook'), t('Validate a pending write or record completion metadata.', '校验待写操作或记录完成元数据。')],
        [t('Expose external tools/data', '暴露外部工具／数据'), t('MCP server', 'MCP 服务器'), t('A project service with its own connection and authentication.', '拥有自身连接与认证的项目服务。')],
        [t('Distribute several extensions together', '共同分发多种扩展'), t('Plugin', '插件'), t('Versioned skills, hooks and servers under one lifecycle owner.', '共同生命周期负责人管理的版本化 Skills、Hooks 和服务器。')],
      ] }], paragraphs: [t('A skill adds instructions; it does not bypass tool permissions or install process transport. A hook handles specific events; it should not replace a normal tool. MCP needs a live, authenticated connection rather than a settings entry alone.', 'Skill 增加指令，不绕过工具权限，也不安装进程传输。Hook 处理特定事件，不应替代普通工具。MCP 需要真实认证连接，不能只看配置条目。')] },
      { id: 'write-a-skill', title: t('Write a small, testable skill', '编写小而可验证的 Skill'), code: [{ label: t('Markdown · skill source example', 'Markdown · Skill 源文件示例'), language: 'markdown', verification: 'manual', code: `---
name: inspect-change
description: Review a scoped change and report reproducible findings.
---

1. Read the requested diff and its owning source.
2. Check one normal path and one failure path.
3. Report actionable findings with file locations.
4. State which checks were actually run.
` }], paragraphs: [t('parse_skill_markdown parses frontmatter/body into Skill; SkillRegistry handles registration/discovery. The host loader selects trusted roots and source tiers. Verify the skill appears in the host listing, then invoke it on a small fixture with an observable expected result.', 'parse_skill_markdown 将 frontmatter／正文解析为 Skill；SkillRegistry 负责注册／发现。宿主 loader 选择可信根目录与来源层级。先确认 Skill 出现在宿主列表，再用结果可观察的小 fixture 调用。')], note: t('This is file content; placement follows the host’s configured discovery roots. Another product’s configuration directory is not automatically active in every embedded host.', '示例是文件内容；位置由宿主配置的发现根目录决定。另一产品的配置目录不会在所有嵌入宿主中自动生效。') },
      { id: 'add-a-hook', title: t('Add a scoped pre-execution check', '添加有范围的执行前检查'), code: [{ label: t('JSON · settings hook example', 'JSON · settings Hook 示例'), language: 'json', verification: 'manual', code: JSON.stringify({ hooks: { PreToolUse: [{ matcher: 'Write|Edit', hooks: [{ type: 'command', command: 'python3 scripts/check-write.py', timeout: 5, onFailure: 'block' }] }] } }, null, 2) }], tables: [{ headers: [t('Field', '字段'), t('Purpose', '用途')], rows: [
        [t('PreToolUse / matcher', 'PreToolUse／matcher'), t('Select the pre-tool event and canonical tool names to check.', '选择工具执行前事件与待检查规范工具名。')],
        [t('type / command', 'type／command'), t('Use the command executor and a host-resolved program. Supply the script yourself.', '使用 command executor 和宿主解析的程序；脚本需自行提供。')],
        [t('timeout', 'timeout'), t('Bound foreground execution in seconds.', '以秒为单位限制前台执行时长。')],
        [t('onFailure: block', 'onFailure: block'), t('Supported pre-action consumers block the action when this hook fails.', '支持该策略的执行前消费者在 Hook 失败时阻止动作。')],
      ] }], paragraphs: [t('Use parse_hook_layer_strict for strict editor/service validation of the event-map layer. parse_hooks_from_settings_json reads the enclosing settings object. Parsing establishes a definition, not a ready executor: command, HTTP, prompt, agent and MCP handlers need their host dependencies. Test an allow path and a blocked/error path.', '编辑器／服务严格校验事件映射层时使用 parse_hook_layer_strict；parse_hooks_from_settings_json 读取外层 settings 对象。解析只建立定义，不代表 executor 就绪：command、HTTP、prompt、agent 与 MCP handler 需要宿主依赖。应同时测试允许路径及阻止／错误路径。')] },
      { id: 'connect-mcp', title: t('Check MCP readiness explicitly', '显式检查 MCP 就绪状态'), apis: [
        { name: 'McpRegistry::connect', signature: 'pub async fn connect(&self, config: McpServerConfig) -> Result<McpConnectionId, McpError>', description: t('Connect through the host-supplied transport, returning the connection id or concrete failure.', '经宿主提供的传输建立连接，返回连接 id 或明确失败。') },
        { name: 'McpRegistry::has_callable_server', signature: 'pub async fn has_callable_server(&self, name: &str) -> bool', description: t('Check current callability rather than the mere existence of saved configuration.', '检查当前可调用性，而不只检查保存配置是否存在。') },
        { name: 'McpRegistry::subscribe_catalog_changes', signature: 'pub fn subscribe_catalog_changes(&self) -> broadcast::Receiver<McpCatalogChanged>', description: t('Observe catalog changes and replace the owning capability partition.', '观察目录变更，替换所属能力分区。') },
      ], steps: [t('Resolve the intended server configuration and authentication in the host.', '在宿主解析预期服务器配置与认证。'), t('Connect using the platform’s available transport; inspect connection/auth state.', '使用平台可用传输连接，检查连接／认证状态。'), t('Publish accepted tools into the server’s owning partition.', '将通过接纳的工具发布到服务器所属分区。'), t('Refresh/remove the partition on catalog changes or disconnect.', '目录变更或断开时刷新／移除该分区。')] },
      { id: 'package-and-diagnose', title: t('Package the workflow and diagnose activation', '打包流程并诊断激活'), paragraphs: [t('For a workflow spanning skills, hooks and MCP tools, use plugin lifecycle ownership instead of unrelated UI-side installation steps. Inspect loaded plugin state and component diagnostics. A manifest entry or parsed hook is not proof its runner, process or credential is ready.', '流程涉及 Skills、Hooks 与 MCP 工具时，应使用插件生命周期归属，不由界面执行互不关联的安装步骤。检查插件加载状态与组件诊断。Manifest 条目或已解析 Hook 不能证明其 runner、进程或凭据就绪。')], tables: [{ headers: [t('Symptom', '现象'), t('First check', '优先检查')], rows: [
        [t('Skill is absent', 'Skill 未出现'), t('Discovery roots, source tier, frontmatter and active-session loader.', '发现根目录、来源层级、frontmatter 与活动会话 loader。')],
        [t('Listed hook never runs', '已列出的 Hook 不执行'), t('Event, matcher, managed policy and injected executor/runner.', '事件、matcher、managed 策略与注入 executor／runner。')],
        [t('Configured MCP has no callable tools', '已配置 MCP 没有可调用工具'), t('Connection/auth state and catalog refresh diagnostics.', '连接／认证状态与目录刷新诊断。')],
        [t('Removed plugin tools remain visible', '已移除插件工具仍可见'), t('Owner-scoped teardown and next capability snapshot.', '按归属的清理与下一次能力快照。')],
      ] }], links: [{ label: t('Plugin activation', '插件激活'), href: '/docs/harness-plugins' }, { label: t('Extension architecture', '扩展架构'), href: '/docs/extensions-lifecycle' }, { label: t('Claude hooks topic reference', 'Claude Hooks 主题参考'), href: 'https://code.claude.com/docs/en/hooks' }] },
    ],
  },
  {
    id: 'agent-parallel-tasks', group: 'harness', title: t('Observe parallel agents and tasks', '观察并行 Agent 与任务'),
    description: t('Give independent work clear ownership and build progress/cancellation controls around authoritative agent and task state.', '为独立工作定义清晰归属，并根据权威 Agent／任务状态构建进度与取消操作。'),
    sourceUrl, sources: [...sources('crates/agent/src/pool.rs', 'crates/agent/src/pool/capacity.rs', 'crates/agent/src/handle.rs', 'crates/runtime/src/desktop/background_agent.rs', 'crates/runtime/src/mobile/host.rs', 'crates/tasks/src/registry.rs', 'crates/tasks/src/state.rs', 'crates/client/src/protocol/commands.rs', 'crates/client/src/protocol/events.rs', 'crates/client/src/protocol/listings.rs'), { repo: 'lingxi-app', path: 'apps/bridge-server/src/router.rs' }, { repo: 'lingxi-app', path: 'apps/bridge-server/src/router/tasks.rs' }],
    sections: [
      { id: 'split-by-owner', title: t('Split independent work with clear owners', '按清晰归属拆分独立工作'), paragraphs: [t('Parallel work is useful when subtasks can progress independently, such as an API review and a UI review. Give each worker an input, owned output and completion condition. Shared files, registries and external side effects still need coordination.', '子任务能独立推进时，并行工作才有价值，例如 API 审查和界面审查。为每个 worker 指定输入、负责输出与完成条件。共享文件、注册表和外部副作用仍需协调。')], tables: [{ headers: [t('Identifier', '标识'), t('Meaning and use', '含义与用途')], rows: [
        [t('session_id', 'session_id'), t('Conversation/workspace owner; scopes history and client views.', '对话／工作空间归属，用于限定历史与客户端视图。')],
        [t('agent_id', 'agent_id'), t('Agent instance and its own transcript; use host-returned ids for inspection.', 'Agent 实例及其独立对话，检查时使用宿主返回的 id。')],
        [t('task_id', 'task_id'), t('Long-lived task state/output owner; use it for output reads and stop requests.', '长期任务状态／输出归属，用于读取输出和请求停止。')],
        [t('turn_id', 'turn_id'), t('Optional client correlator for one admitted foreground action and targeted cancellation.', '单次已准入前台动作的可选客户端关联标识，用于定向取消。')],
      ] }] },
      { id: 'admission-before-execution', title: t('Treat capacity as admission', '将容量视为准入问题'), paragraphs: [t('StateMachinePool has an explicit max_concurrent capacity. Ordinary slot admission fails fast when physical capacity is full; group admission reserves all requested slots or none. Distinguish capacity refusal from provider timeout and tool failure before choosing a retry policy.', 'StateMachinePool 有显式 max_concurrent 容量。普通槽位在物理容量满时快速拒绝；组准入一次取得全部所需槽位，或完全不取得。选择重试策略前，应区分容量拒绝、供应商超时与工具失败。')], bullets: [t('Keep cancellation responsive during admission and execution.', '准入与执行期间均保持取消可响应。'), t('Do not hold a partial group allocation while waiting for the remaining slots.', '不要占住部分组槽位，再等待剩余槽位。'), t('A cloned session handle is not a new worker; use the host’s actual subagent/task composition.', '克隆会话句柄不是新 worker，应使用宿主实际的子 Agent／任务组合。')], links: [{ label: t('Agent pool and ownership', 'Agent 池与归属'), href: '/docs/agents-background' }] },
      { id: 'background-owner', title: t('Use a real background-task owner', '使用真正的后台任务负责人'), paragraphs: [t('Desktop’s background-agent decorator connects detached Agent work to LocalAgent task state, output and mailbox handling. A bare pool spawner is not a replacement for that durable composition. Closing a view, spawning a Tokio future or retaining an in-memory agent record does not establish restart-safe background execution.', '桌面的 background-agent 装饰器将脱离前台的 Agent 工作接入 LocalAgent 任务状态、输出与邮箱处理。裸 pool spawner 不能替代持久组合。关闭视图、创建 Tokio future 或保留内存 Agent 记录，都不能单独建立可跨重启的后台执行。')], steps: [t('Choose a host with task handlers and background admission wired.', '选择已接通任务 handler 与后台准入的宿主。'), t('Retain the admitted task id and owner; read status and output through that owner.', '保留准入后的 task id 与归属，并经该负责人读取状态和输出。'), t('Distinguish mailbox delivery from the recipient actually consuming a message.', '区分邮箱交付与接收方实际消费消息。'), t('After restart, verify restored ownership and execution state before offering Resume.', '重启后先核对恢复归属与执行状态，再提供恢复操作。')] },
      { id: 'inspect-progress', title: t('Build a monitor using the protocol', '使用协议构建监视器'), code: [{ label: t('JSON · independent monitoring commands', 'JSON · 独立监视命令'), language: 'json', verification: 'excerpt', code: `{"type":"task_list","request_id":"tasks-panel-1"}
{"type":"task_output","task_id":"<task-id>","offset":0}
{"type":"list_session_agents"}
{"type":"load_session_agent_transcript","agent_id":"<agent-id>"}` }], paragraphs: [t('Desktop Bridge returns TaskRow events and, when request_id is supplied, an authoritative TaskListComplete receipt carrying that id. Current MobileHost ignores request_id and emits only TaskRow events; it has no task-list completion receipt. Keep receipt-based snapshot reconciliation specific to a host that implements it, and do not wait for TaskListComplete on MobileHost.', 'Desktop Bridge 返回 TaskRow 事件；提供 request_id 时，还返回携带该 id 的权威 TaskListComplete 回执。当前 MobileHost 忽略 request_id，只发送 TaskRow，没有任务列表完成回执。基于回执的快照协调仅适用于已实现该能力的宿主，不应在 MobileHost 上等待 TaskListComplete。'), t('TaskOutput returns TaskOutputChunk with content, total_lines and truncated. Follow the host’s offset contract for later reads; do not derive an offset from JavaScript string length or assume a truncated result delivered every line. Agent listings/transcripts differ from task output and are scoped to the mounted session.', 'TaskOutput 返回包含 content、total_lines 与 truncated 的 TaskOutputChunk。后续读取应遵循宿主 offset 契约，不按 JavaScript 字符串长度推导偏移，也不能假定截断结果已交付全部行。Agent 列表／对话不同于任务输出，并限定在已挂载会话范围。')], note: t('Use ids returned by the host. Placeholder ids are not executable identities. An empty output chunk or partial transcript is not a completion signal; inspect task/agent state.', '应使用宿主返回的 id，占位 id 不是可执行标识。空输出块或部分对话不是完成信号，应检查任务／Agent 状态。') },
      { id: 'stop-and-collect', title: t('Stop, then collect terminal state', '停止后收集终态'), code: [{ label: t('JSON · stop the selected task', 'JSON · 停止选定任务'), language: 'json', verification: 'excerpt', code: '{"type":"task_stop","task_id":"<task-id>"}' }], steps: [t('Send TaskStop for the selected host-returned id, not an unrelated foreground turn.', '为选定的宿主返回 id 发送 TaskStop，不要误取消无关前台轮次。'), t('Keep the row visible as stopping/pending until the host reports a terminal state.', '宿主报告终态前，保留条目并显示停止中／等待状态。'), t('Retain partial output, failure reason and durable completion/publication receipts.', '保留部分输出、失败原因与持久完成／发布回执。'), t('Release observers and task/workspace resources through the lifecycle owner.', '通过生命周期负责人释放 observer 和任务／工作空间资源。')], note: t('The existence of collaboration/task types does not prove desktop, mobile or cross-device execution parity. Check actual host wiring for the platform you ship.', '存在 collaboration／task 类型，不代表桌面、移动端或跨设备执行完全等价。应检查实际发行平台的宿主接线。'), links: [{ label: t('Background execution and recovery', '后台执行与恢复'), href: '/docs/agents-background' }, { label: t('Validation boundaries', '验证边界'), href: '/docs/validation-evidence' }] },
    ],
  },
];
