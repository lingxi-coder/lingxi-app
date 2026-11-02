# 合并后 Chat / Code 集成审查记录

本记录为进行中的审查台账，不能替代最终验收。产品约束与架构见[设计文档](../architecture/conversation-spaces-context-design.md)，续作顺序见[交接文档](2026-11-01-conversation-spaces-handoff.md)。原始日志保存在任务 worktree 的 `acceptance/conversation-spaces-20261101/`，不作为源码提交。

## 必须保持的行为

- 普通 New Chat 在当前空间创建无 Project 会话，session mode 创建即固定。
- UI 切换空间、项目、会话或页面，不取消已接纳的 turn，不把审批、恢复和音频移交给其他 owner。
- 恢复使用捕获的精确 transcript 来源；逻辑 scope 与运行 cwd 分离。
- CLI 公共入口仅 Code；私有已认证后台执行保留捕获的固定身份。
- 验证只覆盖 Desktop default、Android Direct、iOS Full，各一个 flavor；不引入旧数据兼容。

## 已修复并验证的 Desktop 问题

提交 `484104889` 包含以下修复：

| 触发条件 | 原问题 | 修订行为 |
| --- | --- | --- |
| Global 调度目标或 Project 会话执行过 cd | 调度目录被当作逻辑归属，普通 Global 目标遗漏或 Project 目标查错目录。 | 按持久 owner 的逻辑 scope 发现目标，执行使用实际 cwd 与精确来源。 |
| 在 Chat 空间编辑 Code 定时任务的标题或时间 | 当下 UI mode 覆盖原任务 mode，或原目标被错误拒绝。 | Host 从 SDK 读取可信的既有配置，未改变目标时保留固定 mode/source。 |
| 历史打开排队期间再次 New Chat / 切空间 | 旧请求提前改变模式，或迟到结果重新激活旧 owner。 | Host queue、runtime activation 与 renderer 分别检查导航 generation，只有实际打开目标才触发页面跳转。Bot 同样先准备、后条件激活。 |
| 调度会话首次绑定后、首条消息前取消 | NewSession 已有 run 来源收据，但空 transcript 不可恢复。 | 全部新调度 owner 在绑定前持久化可恢复空锚点。 |
| 编辑表单回传 Host 填充的来源字段 | 严格 IPC schema 拒绝保存，或表单携带过期元数据。 | 表单只提交用户字段，Host 重新校验可信来源。 |

175 项相关回归与 Node/renderer 类型检查通过；真实 Electron 首轮完整运行暴露的 fixture 问题另行处理，不作为这次定向验证已经覆盖的内容。

提交 `316ac8980` 修复首条输入后的 Sidebar 重复项：空草稿保持可见，提交/保存后的同一 UUID 会话替换草稿副本。真实 Electron first-session fixture 已验证各阶段唯一行与焦点保持。

## SDK、协议与移动端闭环要求

以下项目正在集成，只有最终 pin 下的测试和原生验证完成后才能标记通过：

1. 冷/热恢复、fork 与后台 handoff 将捕获的 root/leaf authority 贯穿到首次 writer mount；拒绝同路径替换及同 UUID 副本接管。
2. Cron 先独立持久预留 owner，child 创建成功后才绑定 task/run，App 确认归属后才启动模型、hook 或 deferred tools。首次创建失败保留 reservation，但不能把尚不存在的路径绑定成永久 TaskSession。
3. 独立 reservation ledger 不随任务或历史裁剪而消失；scope 目录、事务目录与锁身份共同固定，损坏原文不得被默认为空后覆盖。
4. 每 UUID 的短准入预留、取消终结屏障和严格 reconciliation 覆盖 foreground 与 scheduler；其他 UUID 的新建及已有 turn 继续运行。
5. 模型/工具已经运行但终态持久化失败时报告结果未知，不能作为 Skipped、网络重试或一次新的执行。
6. 项目新增、导入和重导入不能覆盖本机 owner ledger；scope/storage 根稳定，不随会话 cwd 改变。

## 验收边界

当前宿主 Swift、隔离 JVM、静态类型与源级测试只证明各自运行的源码路径，不能替代实际 UniFFI metadata、目标架构库、原生链接及设备执行。最终顺序为 SDK 回归和提交、App immutable pin/locked resolver、协议与 FFI 重生成、三端单 flavor 验证、独立复审并修复。

Desktop 全量首轮允许 localhost 后为 3810 通过、27 失败、16 跳过。fixture 合同对齐提交 `03059116d` 后，中间完整运行达到 3841 通过、3 失败、16 跳过；三项为已单独修复验证的 Bot test double，以及等待最终 Bridge binary 和 Rust producer fixture 的依赖。`e8b88315b`、`c4adeaa69` 进一步覆盖稳定 `/cd` 与精确选择，相关定向 145/145 和 Node types 通过。最终 pin 后仍要完成真实联调，禁止为了绿色结果跳过真实失败或加入缺字段兼容。

SDK 最新冻结 session 门禁为 457 unit + 177 integration/doc，共 634 项通过（`sdk/final-session-full-r3.log`）；目录发现定向 16/16、client protocol 28/28、snapshot 9/9 通过。组合 runtime/UniFFI、mobile-only、App pin 与原生验证仍待完成，不能据此宣称整条链已验收。

归属准入覆盖产品实际同一 App 进程；合作的 SDK writer 使用 UUID lease 与各 scope 持久事务。可观测文件替换必须拒绝或报告结果不确定，不宣称具备跨所有 scope 的跨进程原子 claim，或抵御绕过锁的同用户进程在最后检查之后更换命名空间。

## 2026-11-02 SDK successor 与 Desktop metadata 增量

Harness 本轮 successor 为 `9c8f4f6c4c6cffd2d14056225ebd80d2210b0efe`，此前 `186ecf40` 的结果为历史基线。新冻结源码完整 session lib **464/464**、session integrations **177/177** 通过；fresh Desktop prompt → authority → capture/fork **1/1** 通过。补充回归覆盖 fresh header authority、active metadata incarnation、非 LF 尾片段、staging 替换与发布后不确定状态。App 已更新 immutable pin，最终绑定、原生编译和设备测试仍需单独闭环。

Desktop metadata 不再由 Node 直接 append transcript：热 owner 使用原连接的 SDK writer，冷来源使用持 UUID lease 的私有 Rust subprocess。确认操作复用对话框打开时捕获的 token。真实 WS 回归发现并修复旧 socket 延迟关闭误拒绝新请求的问题；socket 归属覆盖 RPC 和 media response，旧 connect 失败不关闭新连接。新 pin 下 BridgeClient 完整测试为 **360 pass / 5 emulator skip / 0 fail**（365 项），build/typecheck 通过；metadata subprocess observer 定向 **9/9**。它们不能替代 Host/UI 或实际 Rust subprocess/native 验证。

后续独立 review 确认三项 SDK 缺口，当前正在同一 successor 中集中修复：合法空锚点 rename 必须保留可恢复 marker 与验证后的 cwd；retry/tombstone 发布 transcript 后 sidecar 清理失败必须报告不确定并禁止继续用已删除 parent；compaction 不得把发布后的不确定结果当作可忽略 GC 失败。9c8f host FFI 构建已安全中断、无最终库与绑定，不能计为原生通过。待这组修复及调用方复审闭合后再更新最终 pin。

Desktop Host/UI 收口提交 `4b14b4735`：重命名与归档确认复用首次捕获 token，取消、导航、dispose 和中途出现新 runtime 时不改用其他来源；归档按固定 logical scope 管理任务，不为预检启动冷会话。Host 聚焦 92/92、Node/Web 类型检查、真实 Electron archive/rename 2/2 通过。独立复审发现的缓存标题同 UUID sibling P2 已修，回退同时比较 UUID/path/mode，增量 rename 回归 3/3 与 Node 类型通过。默认 Desktop 构建和另外五组真实 Electron 交互（首条输入、runtime lifecycle、picker、gestures、storage）均通过；它们使用 fixture bridge，并不替代最终真实 Bridge binary 联调。

当前 scheduler pause API 只按 task ID 更新，列表后并发改目标不具有 expected-target CAS；任务暂停与 session archive 不是跨文件原子事务，部分暂停后失败明确报告已完成的暂停数量。本轮不引入新的全局事务协议。

独立复审继续确认两组必须先修的来源风险，最终 native 构建继续等待：Desktop audio reservation 原先只防 cache eviction，同 UUID 来源替换或 `/cd` 仍可重挂录音 owner；单次听写也须捕获原 runtime。SDK FileHistory 把旧目录 A 的相对文件名在当前 cwd B 展开，可能删除或覆盖 B 的同名业务文件；新格式须保留原绝对来源，无法证明出处的相对记录直接拒绝。SDK durable side-record 和 metadata backstop 也必须受原 authority/uncertain fence 保护，不得绕过或先污染缓存。上述修复正在同一轮集中推进，不能把 9c8f 或历史通过数字视为最终验收。

Desktop 音频源码检查点为 `7305467d6` 与 `4df650e07`：runtime reservation 覆盖 dictation/flow/realtime 启动和运行，同 UUID 来源替换、`/cd`、archive、clear、close/restart 不能重挂原音频 owner。operation promise 返回 failed 仍不视为资源已关闭；原 native manager 汇总 parent/child identity，只有 never-admitted 或全部任务/精确关闭完成才释放，错误 flight 回执不能释放。最终定向 258/258、Node/Web 类型、diff-check 通过；此前真实 Electron dictation 1/1。独立静态复审本组无剩余确定性阻断。旧 archive 生命周期 fixture 更新 `461367206` 两文件 15/15。完整 default Desktop suite 正在按当前源码执行，仍需区分 provisional SDK pin 与待到位的真实 Rust producer/binary。
