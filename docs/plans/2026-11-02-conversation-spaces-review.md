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
