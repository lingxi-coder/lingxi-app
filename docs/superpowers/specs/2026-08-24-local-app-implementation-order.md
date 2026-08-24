# Local App 实施总顺序

日期：2026-08-24
状态：两份 Local App design 的跨计划执行契约

关联设计：

- [`2026-08-23-create-app-conversational-flow-design.md`](./2026-08-23-create-app-conversational-flow-design.md)
- [`2026-08-23-local-app-interactive-verification-design.md`](./2026-08-23-local-app-interactive-verification-design.md)

## 目的

两份设计保持独立：create-flow 管「如何创建、定 profile、脚手架和构建」，verification 管「如何冒烟、标注、提交反馈和交互验收」。本文只负责三件事：

1. 定义实施先后；
2. 给共享修复和协议 bless 指定唯一 owner；
3. 禁止两个计划同时修改相同技能、workflow、host 或客户端状态机。

本文不复制两份 design 的业务细节。行为契约以各自 design 为准；跨计划顺序与共享文件所有权以本文为准。若两者冲突，先修正文，再继续实现，不在 executor 中临时解释。

## 权威边界

| 范围 | 权威文档 |
|---|---|
| 空壳创建、`CreateMode`、`request_id`、`scaffolded`、会话 pin | create-flow §0-§H |
| Web runtime profile、pnpm locks/snapshot、engine adapters | create-flow §I |
| Godot / NativeGame 产品边界 | create-flow §I.7；实现还需独立 design + threat model |
| `inspect_ui` / `capture_ui` contract、smoke spike | verification |
| annotation、build generations、iOS overlay/副驾驶条 | verification |
| 跨计划实施顺序、共享补丁 owner、协议 rebase | 本文 |

## 共享补丁：只实现一次

| 共享事项 | 唯一 owner | 下游如何处理 |
|---|---|---|
| 移动端 `LINGXI.md` guest→host memory probe | create-flow §0 | verification 删除自己的实现任务，只把它当已满足前置并保留回归验收 |
| `local-canvas-build` workspace lease + 运行中删除保护 | verification Phase 1b | create-flow §I.6 只断言补丁已存在，不再另写第二份实现 |
| `.lingxi` 从 workspace build key 输入中排除 | verification Phase 1b | runtime-profile 与 annotation 构建测试共同复用 |
| 固定 IIFE 下 Babylon + glTF + Havok 双端 spike | create-flow Phase 1.0 | verification 不改变公共 Vite contract；后续 smoke 测试复用结果 |
| smoke WebView registry/document-load/报告载体 spike | verification spike | create-flow 不替它选择挂载点；runtime profile 不被该未决项阻塞 |
| client protocol 8.0.0 首次 bless | create-flow §B | verification Phase 3 必须 rebase，不得从 7.0.0 独立 bless |
| annotation/build-generation 协议追加 | verification Phase 3 | 在 8.0.0 基线上按实际 contract diff 决定下一版本，并只生成一次两端 bindings |

## 下游防漂移契约

分开写文档不等于允许 verification 固定在某个旧 create-flow。verification 消费的是下面这组 post-create seam；create-flow 修改任一项时，**同一个文档/实现变更**必须更新 verification 的上游契约、阶段表、测试与状态转换表：

- `scaffolded` commit point 和空壳/已成形分界；
- manifest surface/runtime profile 的持久化与不可变性；
- workspace、app scope、init/pin session 语义；
- `local-app-build` / `local-canvas-build` workflow 集合与 lease/delete guard；
- pnpm/profile lock、build/restore 的权威来源；
- client protocol 中 create mode、`request_id`、`AppRecordDto.scaffolded` 与被删除的 identity-proposal 契约。

防漂移不能只靠 cross-link，必须有机器证据：

1. verification 的实现提交记录它所 rebase 的 create-flow baseline commit；
2. Phase 3 contract guard 断言上游 anchors 仍在，拒绝 major-7 snapshot 回退；
3. verification 基础测试分别通过当前 direct-create/profile 路径与 shell→host-confirmation→Scaffold 路径创建 app，再跑 build/inspect/capture；
4. 手写 fixture 只测损坏/legacy 分支，不能成为 happy-path 的唯一创建方式；
5. create-flow 改了 seam 而没有更新 verification/master 时，相关 PR 不得合并。

因此，create-flow 的后续变化会影响 verification，但影响会在 rebase/contract/端到端创建门上显式变红，而不是等到 Phase 4 真机才发现。

## 必须遵守的执行顺序

| 步骤 | 交付 | 进入条件 | 退出门 |
|---|---|---|---|
| 0 | 冻结两份 design 的共享接口 | 本文已合并 | 两份 design 都链接本文；不存在并行修改同一共享文件的活动分支 |
| 1 | create-flow §0-§H | 无 | `LINGXI.md` probe 修复、对话式空壳创建、协议 8.0.0、两端 bindings/客户端与真机验收全部通过 |
| **1'** | verification Phase 1a（两端 inspect/capture contract + `image-read`） | **无** | 两端 contract 通过；不动协议。**与步骤 1 零文件重叠**（`LocalAppWebView`/`image-read` 在 create-flow 全文 0 命中），可与步骤 1 并行 |
| 2 | verification Phase 1b（共享基础补丁） | 1、1' | `.lingxi` build-key、canvas workflow lease/delete guard 通过；不动协议 |
| 3a | create-flow Phase 1.0 IIFE spike | 2 | Babylon + glTF + Havok 在固定 pnpm/IIFE 下完成双端 production build + WebView smoke；失败则 runtime profile 停止 |
| 3b | verification smoke spike | 1' | 六条真机判据、WebView role/registry、fresh document load、权限与结构化报告载体有实测结论；失败不阻塞 3a/4 |
| 4 | create-flow Phase 1 Web runtime profiles | 3a 通过 | 五 profile、engine-free seed、receipt confirmation、snapshot/SBOM、workflow/build/真机性能门通过 |
| 5 | verification Phase 3 数据/协议基础 | 2、4 | 在协议 8.0.0 + profile-aware 代码上 rebase；annotation 原子存储、build generations、下一次 contract bless 与两端 bindings 通过 |
| 6 | verification Phase 2 smoke gate/workflow 改造 | 3b、4 | smoke gate 对所有 Web profile 有明确结果；无能力平台返回 `verification_unavailable`，不静默通过 |
| 7 | verification Phase 4/5 iOS 交互验收 | 5、6 | overlay、状态机、提交路由、热重载与最小副驾驶条完整真机闭环；视觉打磨最后单独评审 |
| 8 | Godot / NativeGame | desktop host 完成、create-flow §I.7 独立 design/threat model 通过 | desktop-only 持久模型/API、toolchain、安全 sandbox 与当前 OS export 验收通过 |

步骤 3a 与 3b 可以同时做**只读调查和独立 spike 取证**，但不得在同一时段修改 `LocalAppWebView`、`local_apps_host.rs`、`local_apps_build.rs` 或 workflow 文件。需要生产代码时按表中顺序串行合并。

verification Phase 3 在其自身逻辑上只依赖 Phase 1；本文把它排在 runtime profiles 之后，是为了避免 `AppRecord`、DTO、host、bindings 与客户端 adapter 同时发生两轮冲突修改。不得以「逻辑上可并行」为由绕过这个集成顺序。

## 共享文件写入规则

以下 ownership 任一时刻只能有一个计划处于写入状态：

- `client-protocol/**`、`clients/shared/src/protocol.ts`；
- `local-apps/src/{types,manifest,service,events}.rs` 与 storage fixtures；
- `apps/engine-mobile/src/local_apps_{host,build,mcp,tools,bridge}.rs`；
- `skills/create-local-app/**`；
- `tools/workflow/src/local_app*`、`tools/workflow/src/builtins.rs`、`tasks/src/handlers/local_workflow.rs`、`tasks/src/registry.rs`；
- iOS `LocalAppsStore` / `LocalAppWebView` / `RootView` 与对应 Android adapter/ViewModel。

每个步骤开始前必须：

1. rebase 到上一步已验证的 commit；
2. 重新查代码图与实际行号，不沿用 spec 中可能漂移的行号；
3. 将已由上游完成的共享补丁从本步骤改为断言/回归测试，禁止复制实现；
4. 协议变更先更新 contract index/goldens，再生成 UniFFI bindings，最后编两端客户端。

## 阻塞与继续规则

- verification smoke spike 未决只阻塞步骤 6/7，不阻塞 runtime profile 步骤 4。
- IIFE spike 失败阻塞步骤 4，并按本文默认顺序连带暂停步骤 5；create-flow §0-§H、verification Phase 1a/1b 与 smoke spike 可以继续。若决定在没有 runtime profiles 的情况下先交付 verification Phase 3，必须先更新本文和两份 design 的 rebase 顺序，不能口头跳过步骤 4。
- Android 没有 smoke host 能力时必须返回 `verification_unavailable`，不能把 iOS-only 成功推广到 Android。
- Godot 不得借用 WebView `AppSurface::Canvas` 或 `window.lingxi.v2` 绕过独立设计门。
- 若某一步需要改变另一份 design 的已锁定契约，先同时更新两份 design 与本文，再实现。

## 总体验收

最终完成不等于三份文档各自测试通过。还必须有一条跨计划真机链路：

1. 从「+」进入对话式空壳创建；
2. 用户在 host-native picker 确认 runtime profile；
3. pnpm 依赖准备、production build、smoke gate 完成；
4. 打开应用，框选问题并存储 annotation；
5. app scope 会话修改源码并重新构建；
6. 新 build generation 经过 smoke、页面加载新文档，annotation 状态正确清理；
7. Phaser 与 Babylon 各至少跑一遍上述闭环。
