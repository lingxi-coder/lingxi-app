# 继续、恢复、分支与取消

选择正确的会话操作，并区分取消、已保存历史与活动执行归属。

<a id="choose-operation"></a>

## 按用户意图选择操作

| 意图 | 操作 | 标识行为 |
| --- | --- | --- |
| 现在继续提问 | 在已挂载 SessionHandle 上再次 run | 保留对话，先等待上一轮结束。 |
| 开始独立对话 | NewSession | 宿主创建并挂载会话，发送 SessionStarted。 |
| 打开保存历史 | ResumeSession | 指定目标；SessionResumed 确认挂载 id 和消息。 |
| 探索另一分支 | MobileHost 上的 ForkSession | MobileHost 创建新分支；创建成功本身不代表已挂载该分支。当前 Desktop Bridge 未处理此命令。 |
| 只停止当前工作 | CancellationToken／Cancel | 结束轮次，不删除历史，也不关闭宿主。 |

<a id="resume-over-the-protocol"></a>

## 经宿主协议恢复

1. 列出会话，让用户选择返回的会话 id。
2. 发送 ResumeSession，收到 SessionResumed 后再提交追问。
3. 用恢复快照重建消息，并保留权威标识。
4. SendPrompt 作用于已挂载连接，不要给普通实时提示词／取消命令添加 session_id。

### JSON · 按顺序分别发送命令

```json
{"type":"list_sessions","limit":20}
{"type":"resume_session","session_id":"<saved-session-id>"}
{"type":"send_prompt","text":"Continue the investigation","images":[],"turn_id":21}
```

验证方式: 接口摘录

这些对象说明顺序，不表示批处理。公共 Harness 门面没有 resume(session_id) 方法；挂载与持久化由宿主负责。

<a id="fork-a-conversation"></a>

## 分支而不覆盖原对话

本配方对应当前 MobileHost 实现。Desktop Bridge 未接通 ForkSession，因此桌面客户端在宿主实现前不应提供该命令，也不应等待 SessionForked。仅存在共享协议类型不能证明已支持。

ForkSession 的 SessionModeDto 值为 chat 或 code；按命令契约保留当前会话工作空间。应将结果作为新对话标识。生命周期命令携带来源 id，并不改变普通实时命令作用于已挂载连接的规则。

- 宿主确认新分支前，保留原会话入口。
- 不要通过原地编辑已保存 JSONL 或复用来源 id 来实现分支。

1. 在 MobileHost 上发送 ForkSession，等待 SessionForked，并保存返回的新分支 id。
2. 通过 ResumeSession 挂载该 id，或打开宿主中属于该分支的连接；等待 SessionResumed 或宿主权威的挂载确认。
3. 确认后再发送追问；仅创建分支后立即发提示词，仍可能作用于原来挂载的会话。

### JSON · MobileHost 分支到 code 模式

```json
{
  "type": "fork_session",
  "session_id": "<source-session-id>",
  "target_mode": "code"
}
```

验证方式: 接口摘录

<a id="cooperative-cancellation"></a>

## 协作取消，并等待 run 结束

下一次运行使用新的轮次令牌，因为已取消令牌会保持取消状态。协议上需要停止同一次用户操作时，应携带 turn_id，避免误取消稍后活动轮次。取消终态不回滚此前已经执行的工具。

### Rust · 将停止信号接到一个 run

```rust
use harness_runtime::{CancellationToken, HandleError, Harness, RunInput, TurnOutcome};

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
}
```

验证方式: 需环境验证 (harness-guide-cooperative-cancellation)

<a id="saved-is-not-running"></a>

## 已保存历史不等于执行负责人

| 观察结果 | 能证明什么 | 下一步检查 |
| --- | --- | --- |
| 会话出现在列表 | 存在可恢复记录。 | 挂载并检查 SessionResumed。 |
| 部分文本已保存 | 部分输出已保留。 | 检查终态及活动／恢复负责人。 |
| 已请求取消 | 宿主收到了停止信号。 | 等待终态与工具／权限清理。 |
| ShutdownReport.complete 为 false | 必要屏障尚未完成。 | 保留 errors／publications，顺序重试。 |

- [存储与持久发布](session-storage.md)
- [宿主关闭](harness-lifecycle.md#shutdown)

## 源码与文档基线

[源码仓库](https://github.com/lingxi-coder/harness-runtime)

- [harness-runtime/crates/runtime/src/api.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/runtime/src/api.rs) — 已提交源码 (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/crates/client/src/protocol/commands.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/client/src/protocol/commands.rs) — 已提交源码 (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/crates/client/src/protocol/events.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/client/src/protocol/events.rs) — 已提交源码 (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/crates/client/src/protocol/listings.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/client/src/protocol/listings.rs) — 已提交源码 (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/crates/runtime/src/desktop/sdk.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/runtime/src/desktop/sdk.rs) — 已提交源码 (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/crates/runtime/src/mobile/host.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/runtime/src/mobile/host.rs) — 已提交源码 (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/crates/session/src/jsonl/loader.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/session/src/jsonl/loader.rs) — 已提交源码 (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [lingxi-app/apps/bridge-server/src/server.rs](https://github.com/lingxi-coder/lingxi-app/blob/8409379b84c6616332e51978cfbc31c5ba712d78/apps/bridge-server/src/server.rs) — 已提交源码 (8409379b84c6616332e51978cfbc31c5ba712d78)
- [lingxi-app/apps/bridge-server/src/router.rs](https://github.com/lingxi-coder/lingxi-app/blob/8409379b84c6616332e51978cfbc31c5ba712d78/apps/bridge-server/src/router.rs) — 已提交源码 (8409379b84c6616332e51978cfbc31c5ba712d78)

- lingxi-app: HEAD 8409379b84c6616332e51978cfbc31c5ba712d78; 所据文件指纹 sha256:40c003672f89d603760e13d5912a3db44dce330055f42839b55ed26f18dc28e5
- harness-runtime: HEAD f975ab5804bb3af0e72beb3974e5c911ece44d53; 所据文件指纹 sha256:92c2108dc63215bc69ea9c11ea081795da883fdcb1c9c28b060aa9221a1b95a2; 产品固定版本 0e28910fd624638e6c5d1f92eca4899a7938d188

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
