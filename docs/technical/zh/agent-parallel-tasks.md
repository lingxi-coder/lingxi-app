# 观察并行 Agent 与任务

为独立工作定义清晰归属，并根据权威 Agent／任务状态构建进度与取消操作。

<a id="split-by-owner"></a>

## 按清晰归属拆分独立工作

子任务能独立推进时，并行工作才有价值，例如 API 审查和界面审查。为每个 worker 指定输入、负责输出与完成条件。共享文件、注册表和外部副作用仍需协调。

| 标识 | 含义与用途 |
| --- | --- |
| session_id | 对话／工作空间归属，用于限定历史与客户端视图。 |
| agent_id | Agent 实例及其独立对话，检查时使用宿主返回的 id。 |
| task_id | 长期任务状态／输出归属，用于读取输出和请求停止。 |
| turn_id | 单次已准入前台动作的可选客户端关联标识，用于定向取消。 |

<a id="admission-before-execution"></a>

## 将容量视为准入问题

StateMachinePool 有显式 max_concurrent 容量。普通槽位在物理容量满时快速拒绝；组准入一次取得全部所需槽位，或完全不取得。选择重试策略前，应区分容量拒绝、供应商超时与工具失败。

- 准入与执行期间均保持取消可响应。
- 不要占住部分组槽位，再等待剩余槽位。
- 克隆会话句柄不是新 worker，应使用宿主实际的子 Agent／任务组合。

- [Agent 池与归属](agents-background.md)

<a id="background-owner"></a>

## 使用真正的后台任务负责人

桌面的 background-agent 装饰器将脱离前台的 Agent 工作接入 LocalAgent 任务状态、输出与邮箱处理。裸 pool spawner 不能替代持久组合。关闭视图、创建 Tokio future 或保留内存 Agent 记录，都不能单独建立可跨重启的后台执行。

1. 选择已接通任务 handler 与后台准入的宿主。
2. 保留准入后的 task id 与归属，并经该负责人读取状态和输出。
3. 区分邮箱交付与接收方实际消费消息。
4. 重启后先核对恢复归属与执行状态，再提供恢复操作。

<a id="inspect-progress"></a>

## 使用协议构建监视器

Desktop Bridge 返回 TaskRow 事件；提供 request_id 时，还返回携带该 id 的权威 TaskListComplete 回执。当前 MobileHost 忽略 request_id，只发送 TaskRow，没有任务列表完成回执。基于回执的快照协调仅适用于已实现该能力的宿主，不应在 MobileHost 上等待 TaskListComplete。

TaskOutput 返回包含 content、total_lines 与 truncated 的 TaskOutputChunk。后续读取应遵循宿主 offset 契约，不按 JavaScript 字符串长度推导偏移，也不能假定截断结果已交付全部行。Agent 列表／对话不同于任务输出，并限定在已挂载会话范围。

### JSON · 独立监视命令

```json
{"type":"task_list","request_id":"tasks-panel-1"}
{"type":"task_output","task_id":"<task-id>","offset":0}
{"type":"list_session_agents"}
{"type":"load_session_agent_transcript","agent_id":"<agent-id>"}
```

验证方式: 接口摘录

应使用宿主返回的 id，占位 id 不是可执行标识。空输出块或部分对话不是完成信号，应检查任务／Agent 状态。

<a id="stop-and-collect"></a>

## 停止后收集终态

1. 为选定的宿主返回 id 发送 TaskStop，不要误取消无关前台轮次。
2. 宿主报告终态前，保留条目并显示停止中／等待状态。
3. 保留部分输出、失败原因与持久完成／发布回执。
4. 通过生命周期负责人释放 observer 和任务／工作空间资源。

### JSON · 停止选定任务

```json
{"type":"task_stop","task_id":"<task-id>"}
```

验证方式: 接口摘录

- [后台执行与恢复](agents-background.md)
- [验证边界](validation-evidence.md)

存在 collaboration／task 类型，不代表桌面、移动端或跨设备执行完全等价。应检查实际发行平台的宿主接线。

## 源码与文档基线

[源码仓库](https://github.com/lingxi-coder/harness-runtime)

- [harness-runtime/crates/agent/src/pool.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/agent/src/pool.rs) — 已提交源码 (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/crates/agent/src/pool/capacity.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/agent/src/pool/capacity.rs) — 已提交源码 (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/crates/agent/src/handle.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/agent/src/handle.rs) — 已提交源码 (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/crates/runtime/src/desktop/background_agent.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/runtime/src/desktop/background_agent.rs) — 已提交源码 (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/crates/runtime/src/mobile/host.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/runtime/src/mobile/host.rs) — 已提交源码 (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/crates/tasks/src/registry.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/tasks/src/registry.rs) — 已提交源码 (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/crates/tasks/src/state.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/tasks/src/state.rs) — 已提交源码 (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/crates/client/src/protocol/commands.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/client/src/protocol/commands.rs) — 已提交源码 (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/crates/client/src/protocol/events.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/client/src/protocol/events.rs) — 已提交源码 (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/crates/client/src/protocol/listings.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/client/src/protocol/listings.rs) — 已提交源码 (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [lingxi-app/apps/bridge-server/src/router.rs](https://github.com/lingxi-coder/lingxi-app/blob/caf679253971034bc202c566a81c10d4c5af31c4/apps/bridge-server/src/router.rs) — 已提交源码 (caf679253971034bc202c566a81c10d4c5af31c4)
- [lingxi-app/apps/bridge-server/src/router/tasks.rs](https://github.com/lingxi-coder/lingxi-app/blob/caf679253971034bc202c566a81c10d4c5af31c4/apps/bridge-server/src/router/tasks.rs) — 已提交源码 (caf679253971034bc202c566a81c10d4c5af31c4)

- lingxi-app: HEAD caf679253971034bc202c566a81c10d4c5af31c4; 所据文件指纹 sha256:5172e409e969106eb5149a400160fbc916294195f79f186ce9f63e32444c54b5
- harness-runtime: HEAD f975ab5804bb3af0e72beb3974e5c911ece44d53; 所据文件指纹 sha256:92c2108dc63215bc69ea9c11ea081795da883fdcb1c9c28b060aa9221a1b95a2; 产品固定版本 e04c1837508014d68ea042d5c9fe3c8c15af5aa0

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
