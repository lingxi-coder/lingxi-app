# LingXi 技术手册

当前工作区的双语系统与 SDK 文档。正文由官网文档数据生成，请在 apps/web/src/data/docs 修改。

[英文目录](../en/README.md)

## 开始

- [灵犀](overview.md) — 灵犀是运行在你正在使用的电脑和手机上的个人 Agent。这些页面说明如何安装、开始会话、选择模型，以及它能做什么。
- [安装灵犀](install.md) — 查看今天可以安装的灵犀应用，以及命令行工具的名称。
- [第一次会话](first-session.md) — 在项目里打开灵犀，确认文件夹，并查看说明与设置的保存位置。
- [模型与 API key](accounts.md) — 接入你要使用的模型服务。密钥保存在这台电脑的凭据存储里。

## 使用

- [命令行](using-cli.md) — 在终端里和灵犀对话，打印一次回答，或重新打开会话。
- [会话](sessions.md) — 继续最近的会话，挑选更早的一次，或从一份副本接着做。
- [常见任务](tasks.md) — 阅读仓库、修改一个 bug 或运行测试，并先决定灵犀可以做到哪一步。
- [桌面](using-desktop.md) — Mac 桌面应用在那台电脑上运行灵犀。其他桌面系统还不能下载。
- [iPhone 与 Android](using-mobile.md) — 手机应用是内部测试版。本站还没有公布商店链接。
- [权限](permissions.md) — 选择灵犀使用工具之前是否先询问你。权限模式和操作系统沙箱是两件事。

## 定制

- [项目说明](instructions.md) — 告诉灵犀这个仓库如何工作。这些文件就是磁盘上的普通 Markdown。
- [Skills](skills.md) — 一个 skill 是含有 SKILL.md 的目录。灵犀用目录名来显示它。
- [子 Agent](subagents.md) — 用 Markdown 文件定义子 Agent，或让灵犀写一份。/agents 不是管理界面。
- [Hooks](hooks.md) — Hook 是在会话事件发生时运行的命令、HTTP 调用或 MCP 工具。
- [MCP](mcp.md) — 用 lingxi mcp 把灵犀接到 MCP 服务器。项目里的服务器要等你批准后才会连接。
- [插件](plugins.md) — 从市场安装插件，或把插件放在 skills 目录里。

## 参考

- [设置文件](settings.md) — 用户设置在配置目录里。项目设置在仓库里。并不是每个键都按同一种方式合并。
- [环境变量](environment.md) — 一份你自己会设置的变量清单。灵犀还会为插件或后台会话设置变量，那些不用自己导出。
- [命令行参数](cli-flags.md) — 日常启动会用到的参数。lingxi --help 列出这个构建接受的全部参数，包括本页没有展开的那些。
- [斜杠命令](slash-commands.md) — 在会话里输入的命令。/help 列出这个构建会运行的那些。

## 在你的设备上

- [模型](models.md) — 为这次会话选择模型。灵犀把选择记在你的配置目录里。
- [远控](remote-control.md) — 用已登录的浏览器打开你自己电脑上的会话。电脑需要先允许这个浏览器。
- [在手机上执行](mobile-execution.md) — 在 iPhone 和 Android 上，命令运行在这台手机里的 Linux 环境中。
- [远控、Bot 和应用](related-products.md) — 已登录的浏览器可以打开一台已经在运行灵犀的电脑，包括它的 Bot、会话和本地应用。
- [帮助](help.md) — 先检查本机环境，再写信给灵犀支持。隐私请求使用同一个地址。

## API

- [API 参考](api-reference.md) — 按任务找到推荐 API，跟随完整集成指南，再检索对应的公开接口声明。

## Harness Runtime

- [Harness Runtime](harness.md) — 将 Rust 智能体运行时嵌入桌面、移动端或自定义宿主。
- [会话与轮次](harness-sessions.md) — 运行提示词、接收输出、取消任务并读取会话状态。
- [宿主生命周期与持久化](harness-lifecycle.md) — 组合宿主服务，并通过运行时持久化屏障完成关闭。
- [工具与权限](harness-tools.md) — 通过共享契约实现工具，并注册其能力。
- [插件](harness-plugins.md) — 加载插件组件、检查生命周期并处理卸载失败。
- [客户端协议与适配器](harness-client.md) — 在客户端之间共享命令、事件、展示与运行时适配器。
- [选择 Agent 集成方式](agent-sdk-guide.md) — 将熟悉的 Agent SDK 概念对应到灵犀实际的 Rust 运行时、模型客户端与产品协议，选择适合任务的集成层。
- [运行第一个 Agent 轮次](agent-first-turn.md) — 从已组装 Harness 出发，完成提示词、终态结果、后续上下文与有序关闭。
- [把流式输出接到界面](agent-ui-streaming.md) — 接通一个事件目标，呈现文本、工具活动和交互请求，并区分消息完成与轮次完成。
- [继续、恢复、分支与取消](agent-session-workflows.md) — 选择正确的会话操作，并区分取消、已保存历史与活动执行归属。
- [添加工具并处理审批](agent-tool-workflows.md) — 让工具从 schema 与宿主注册进入真实权限往返、结果呈现和清理流程。
- [组合 Skills、Hooks 与 MCP](agent-extension-recipes.md) — 选择合适扩展，配置范围清晰的生命周期检查，并核对外部能力已连接且可调用。
- [观察并行 Agent 与任务](agent-parallel-tasks.md) — 为独立工作定义清晰归属，并根据权威 Agent／任务状态构建进度与取消操作。

## LLM Client

- [LLM Client](llm-client.md) — 独立的 Rust 模型客户端，统一供应商协议、类型化请求、流式响应、图像、嵌入向量与模型目录。
- [请求与执行](llm-requests.md) — 构建类型化模型输入，通过草稿、准备、发送与收集阶段控制每一次实际调用。
- [流式响应与取消](llm-streaming.md) — 读取标准化事件或逐传输块观测，保留工具、原生内容、用量与终止错误。
- [图像 API](llm-images.md) — 通过独立于 Chat 的服务生成、编辑图像并查询原生图像任务。
- [供应商、目录与计价](llm-providers.md) — 绑定连接、按区域过滤、管理目录、访问原生资源，并依据观测事实估算成本。
- [附件与供应商文件](llm-files.md) — 区分宿主持久附件、账户限定的供应商上传文件、模型输入与生成产物。
- [云端转写与语音合成](llm-audio.md) — 独立于 Chat 使用精确连接的音频服务，明确格式、限制流大小并按发送状态处理错误。
- [实时会话与背压](llm-realtime.md) — 在宿主执行器运行有界双向供应商会话，保留工具、音频与中断生命周期。
- [供应商资源与托管工具](llm-native-services.md) — 显式绑定原生资源，区分供应商端执行、Harness 工具与后台工作。
- [输出、推理与缓存控制](llm-controls.md) — 应用已校验的请求级约定，不弱化 Schema，并区分推理预算与金额限制。
- [路由、失败与逐次结算](llm-execution-accounting.md) — 选择便利回退或宿主控制的单次调用，在解释输出前保留实际用量。

## Mobile Linux

- [移动 Linux 运行时](mobile-linux.md) — 在 Android 与 iOS 应用内嵌入 Linux 用户空间，提供命令、流式输出、PTY、原始标准输入输出与经过校验的 rootfs 生命周期。
- [Rust API](mobile-rust.md) — 命令、任务生命周期、PTY、原始标准输入输出、挂载与 rootfs 校验的共享契约。
- [Android · Kotlin](mobile-android.md) — 安装 Maven 分发包、暂存经过验证的 rootfs，并使用持久 Kotlin 运行时。
- [iOS · Swift](mobile-ios.md) — 接入 SwiftPM 与 XCFramework 产品、提供 iSH fakefs 资源，并通过 Swift 控制生命周期。
- [权限与完整性](mobile-authorization.md) — 宿主拥有的挂载策略、网络与资源执行约束、rootfs 信任及明确的平台限制。

## 产品集成

- [TypeScript Bridge 客户端](bridge-client.md) — 将 Node.js 宿主连接到本地 LingXi Bridge 服务，提交命令、消费事件并处理权限请求。
- [原生宿主集成](platform-integration.md) — 了解产品的 iOS 与 Android UniFFI 包装、生成绑定、原生回调及共享移动会话宿主。
- [桌面 · Electron 宿主](desktop-host.md) — 归属于会话的 Bridge sidecar、renderer IPC、事件重放、原生服务与签名打包。
- [CLI、TUI 与无界面宿主](cli-tui.md) — 使用产品命令行、区分终端与 print 模式，并在保留运行时准入的前提下集成确定输出。
- [Web · SDK 官网与 UI 原型](web-product.md) — 当前浏览器应用发布 SDK 文档，并在未连接运行时后端的情况下展示账号／控制台 UI。
- [原生构建、绑定与发布验收](platform-builds.md) — 构建匹配的 Rust／原生产物、选择分发变体，并区分编译、模拟器与实体设备验收。

## 系统机制

- [Agent 执行与恢复](agent-execution.md) — 追踪已准入轮次从宿主输入，到模型请求、流式工具执行及持久化终态的全过程。
- [配置、凭据与隔离](configuration-security.md) — 追踪设置来源，明确模型路由、授权、凭据存储与操作系统隔离的宿主职责。
- [会话标识与持久化存储](session-storage.md) — 理解 JSONL 图、稳定 UUID、冷恢复、分支及上下文压缩与对话存储回收的区别。
- [工具、Skills、Hooks、MCP 与 LSP](extensions-lifecycle.md) — 经可信注册表发现扩展，并在激活、更新、调用和清理过程中保持权限归属。
- [子 Agent 与后台任务](agents-background.md) — 区分前台与脱离前台工作中的 Agent 池准入、任务归属、邮箱交付与终态发布。
- [Cron、Workflow 与 Fusion](automation-workflows-fusion.md) — 理解计划任务、脚本 Agent 编排及多模型证据工作的实际执行路径。
- [附件、音频与实时宿主](multimodal-host.md) — 经显式供应商能力与原生资源负责人路由多模态输入和音频。
- [内联可视化集成](runtime-visualization.md) — 发布不可变可视化 revision，并在 Electron、Android 与 iOS 中经同一授权服务挂载。
- [系统架构与责任边界](system-overview.md) — 理解产品宿主、共享 Agent 运行时、模型 SDK 和本地执行层的关系，以及各层的责任与实现边界。

## 开发与运维

- [开发环境与依赖身份](development-workflow.md) — 初始化四仓环境、增量构建，并避免在 SDK 联合开发时混用本地与固定来源的 Rust 类型。
- [构建、签名与发行流程](build-release.md) — 构建各产品入口，区分依赖检查、签名打包、静态产物与已发布版本。
- [验证方法与证据边界](validation-evidence.md) — 理解文档检查、消费者编译、模拟测试、真实 Provider 调用与设备验收分别能证明什么。
- [诊断、排障与运行恢复](troubleshooting.md) — 将故障定位到责任层，恢复会话与运行资源，并收集不泄露凭据的有效证据。
