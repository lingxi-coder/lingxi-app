# Cron、Workflow 与 Fusion

理解计划任务、脚本 Agent 编排及多模型证据工作的实际执行路径。

<a id="execution-matrix"></a>

## 组合支持

| 能力 | 桌面 | 移动端 |
| --- | --- | --- |
| Cron | 宿主 scheduler／trust 策略与 flag 允许时，构建、加载并启动 CronScheduler；子 runtime 不再拥有重复 scheduler。 | 持久计划与外部宿主触发执行。Rust 移动组合不运行持续 CronScheduler。 |
| Workflow | 已接通 TaskRegistryWorkflowLauncher 与 QuickJS 支持的 local_workflow task。 | MobileWorkflowLauncher 接入移动组装；执行机会仍受系统生命周期管理。 |
| Fusion | 生产服务连接尝试准入、计费、证据、实现与持久回执。 | 不能由共享 DTO／历史投影推断完整移动执行器。提供执行前应验证原生宿主入口。 |

<a id="cron-lifecycle"></a>

## 计划执行与系统约束

计划持久化、到期 occurrence 选择与轮次执行是独立阶段。Cron run_due 使用进程级锁串行化存储读改写，并在运行轮次前释放。宿主计划／信任策略控制自治轮次准入，持久结果保留来源 task 标识。

移动触发返回 session_id、result_text、终态 status 和 retryable。轮次预算为 180 秒，并受更外层系统 worker 生命周期约束。移动重复间隔至少 15 分钟，单次计划不受该限制。Rust 接受计划不保证精确系统唤醒时间，也不保证 iOS／Android 允许后台执行。

<a id="workflow-runtime"></a>

## Workflow 脚本与恢复

Workflow 接收内联 script、命名 workflow 或 scriptPath。scriptPath 优先，并以批准的 rooted 身份重新打开，而不是使用不可信路径名。脚本上限 512 KiB，经 QuickJS workflow runtime 执行。缺失 launcher 时报告明确的未接通失败。

成功启动通常返回 async_launched 与 task／run ID，随后完成以 task 通知到达。解析／编译诊断可能在未注册 task 时返回，需检查返回 error 与 task 标识。ResumeFromRunId 限同会话，并复用未变化的 Agent 调用结果；恢复编辑后的 workflow 前应停止旧 task。

### review-workflow.js · 完整 Workflow 脚本

```javascript
export const meta = {
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
  'Summarize this evidence and identify any unverified assumptions:\n' + evidence,
  { label: 'summarize', phase: 'Summarize' },
);
```

验证方式: 需环境验证

### Workflow 工具输入 · 参数保留 JSON 类型

```json
{
  "scriptPath": "./review-workflow.js",
  "args": {
    "question": "Which modules own the provider and host boundaries?"
  }
}
```

验证方式: 接口摘录

这是 Workflow 脚本，其主体在宿主 async wrapper 内运行；顶层 return 是有意设计，不是独立浏览器／Node 模块。Agent 调用使用真实宿主工具与模型服务，执行仍需凭据、授权和活动 launcher。JSON 载荷不能绕过授权。

<a id="fusion-lifecycle"></a>

## Fusion 证据与持久结果

桌面 Fusion 组合模型解析、panel／attempt 准入、计费与预算归属、证据收集及可选实现。多模型证据仍需综合及记录结果，并发完成并不等于一致。Attempt 保留模型／profile／账户归属用于计费与恢复。

FusionRecorder 与桌面生命周期保留持久发布回执。ShutdownReport.publications 返回排空期间观察到的回执。未完成发布通过所属协调器及重试屏障处理；不能仅因 analyst 或 implementer 模型流结束就报告成功。

<a id="automation-checks"></a>

## 验收场景

- 验证计划存储、重复到期 occurrence、自治准入拒绝、移动轮次超时及可重试／终态失败。Workflow 验证错误输入、缺失 launcher、编译错误、取消与同会话恢复。
- Fusion 检查覆盖池准入、计费和持久交接。真实多模型运行、原生后台交付及崩溃后发布需独立目标验收记录。

### Workflow 与 Fusion 检查

```bash
cargo test --locked -p tool-workflow --lib
cargo test --locked -p tasks --lib
cargo test --locked -p fusion --lib
cargo test --locked -p harness-runtime --features desktop --lib fusion_attempt
```

验证方式: 需环境验证

## 源码与文档基线

[源码仓库](https://github.com/lingxi-coder/harness-runtime)

- [harness-runtime/crates/runtime/src/desktop/assembly.rs](https://github.com/lingxi-coder/harness-runtime/blob/6548b9d5e702f8e9d42772412cd65b72881e24b4/crates/runtime/src/desktop/assembly.rs) — 已提交源码 (6548b9d5e702f8e9d42772412cd65b72881e24b4)
- [harness-runtime/crates/runtime/src/desktop/cron_management.rs](https://github.com/lingxi-coder/harness-runtime/blob/6548b9d5e702f8e9d42772412cd65b72881e24b4/crates/runtime/src/desktop/cron_management.rs) — 已提交源码 (6548b9d5e702f8e9d42772412cd65b72881e24b4)
- [harness-runtime/crates/runtime/src/mobile/host/automation.rs](https://github.com/lingxi-coder/harness-runtime/blob/6548b9d5e702f8e9d42772412cd65b72881e24b4/crates/runtime/src/mobile/host/automation.rs) — 已提交源码 (6548b9d5e702f8e9d42772412cd65b72881e24b4)
- [harness-runtime/crates/runtime/src/mobile/host/assembly.rs](https://github.com/lingxi-coder/harness-runtime/blob/6548b9d5e702f8e9d42772412cd65b72881e24b4/crates/runtime/src/mobile/host/assembly.rs) — 已提交源码 (6548b9d5e702f8e9d42772412cd65b72881e24b4)
- [harness-runtime/crates/cron/src/run_due.rs](https://github.com/lingxi-coder/harness-runtime/blob/6548b9d5e702f8e9d42772412cd65b72881e24b4/crates/cron/src/run_due.rs) — 已提交源码 (6548b9d5e702f8e9d42772412cd65b72881e24b4)
- [harness-runtime/crates/tools/workflow/src/lib.rs](https://github.com/lingxi-coder/harness-runtime/blob/6548b9d5e702f8e9d42772412cd65b72881e24b4/crates/tools/workflow/src/lib.rs) — 已提交源码 (6548b9d5e702f8e9d42772412cd65b72881e24b4)
- [harness-runtime/crates/tools/workflow/src/workflow_input_schema.json](https://github.com/lingxi-coder/harness-runtime/blob/6548b9d5e702f8e9d42772412cd65b72881e24b4/crates/tools/workflow/src/workflow_input_schema.json) — 已提交源码 (6548b9d5e702f8e9d42772412cd65b72881e24b4)
- [harness-runtime/crates/workflow/src/lib.rs](https://github.com/lingxi-coder/harness-runtime/blob/6548b9d5e702f8e9d42772412cd65b72881e24b4/crates/workflow/src/lib.rs) — 已提交源码 (6548b9d5e702f8e9d42772412cd65b72881e24b4)
- [harness-runtime/crates/workflow/src/description.rs](https://github.com/lingxi-coder/harness-runtime/blob/6548b9d5e702f8e9d42772412cd65b72881e24b4/crates/workflow/src/description.rs) — 已提交源码 (6548b9d5e702f8e9d42772412cd65b72881e24b4)
- [harness-runtime/crates/tasks/src/handlers/local_workflow.rs](https://github.com/lingxi-coder/harness-runtime/blob/6548b9d5e702f8e9d42772412cd65b72881e24b4/crates/tasks/src/handlers/local_workflow.rs) — 已提交源码 (6548b9d5e702f8e9d42772412cd65b72881e24b4)
- [harness-runtime/crates/runtime/src/desktop/fusion_services.rs](https://github.com/lingxi-coder/harness-runtime/blob/6548b9d5e702f8e9d42772412cd65b72881e24b4/crates/runtime/src/desktop/fusion_services.rs) — 已提交源码 (6548b9d5e702f8e9d42772412cd65b72881e24b4)
- [harness-runtime/crates/runtime/src/desktop/fusion_attempts.rs](https://github.com/lingxi-coder/harness-runtime/blob/6548b9d5e702f8e9d42772412cd65b72881e24b4/crates/runtime/src/desktop/fusion_attempts.rs) — 已提交源码 (6548b9d5e702f8e9d42772412cd65b72881e24b4)
- [harness-runtime/crates/runtime/src/desktop/fusion_recorder.rs](https://github.com/lingxi-coder/harness-runtime/blob/6548b9d5e702f8e9d42772412cd65b72881e24b4/crates/runtime/src/desktop/fusion_recorder.rs) — 已提交源码 (6548b9d5e702f8e9d42772412cd65b72881e24b4)
- [harness-runtime/crates/runtime/src/desktop/fusion_implement.rs](https://github.com/lingxi-coder/harness-runtime/blob/6548b9d5e702f8e9d42772412cd65b72881e24b4/crates/runtime/src/desktop/fusion_implement.rs) — 已提交源码 (6548b9d5e702f8e9d42772412cd65b72881e24b4)
- [harness-runtime/crates/fusion/src/orchestrator.rs](https://github.com/lingxi-coder/harness-runtime/blob/6548b9d5e702f8e9d42772412cd65b72881e24b4/crates/fusion/src/orchestrator.rs) — 已提交源码 (6548b9d5e702f8e9d42772412cd65b72881e24b4)

- harness-runtime: HEAD 6548b9d5e702f8e9d42772412cd65b72881e24b4; 所据文件指纹 sha256:92c43ec1558ef6090bd3e0c28f21110761b7855079f22272b843555f562fe25b; 产品固定版本 6548b9d5e702f8e9d42772412cd65b72881e24b4

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
