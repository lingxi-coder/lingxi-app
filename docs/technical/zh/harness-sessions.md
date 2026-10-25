# 会话与轮次

运行提示词、接收输出、取消任务并读取会话状态。

<a id="run"></a>

## 运行一个轮次

RunInput 包含用户提示词和 Vec<PathBuf> 图像路径。图像由既有工作空间与模型实现解析。SessionHandle 将执行、权限决策和持久化委托给注入的 SessionService。

### Rust · shared session interface

```rust
use harness_runtime::{
    CancellationToken, HandleError, Harness, RunInput, TurnOutcome,
};

async fn run_turn(
    harness: &Harness,
    prompt: String,
    cancel: CancellationToken,
) -> Result<TurnOutcome, HandleError> {
    harness.session().run(
        RunInput { prompt, images: Vec::new() },
        cancel,
    ).await
}
```

验证方式: 编译检查 (harness-run-turn)

### RunInput

```text
pub struct RunInput { pub prompt: String, pub images: Vec<PathBuf> }
```

可克隆且有默认值的智能体执行输入。

### SessionHandle::run

```text
pub async fn run(&self, input: RunInput, cancel: CancellationToken) -> Result<TurnOutcome, HandleError>
```

通过组合中既有的智能体循环执行轮次。

<a id="outcomes"></a>

## 取消与结果

CancellationToken 从 tokio-util 重新导出。宿主保留一个副本，将令牌传给 run，并在用户中断时调用宿主副本的 cancel()。等待 run future 完成，让既有执行路径完成退出后再释放资源。

### TurnOutcome

```text
pub enum TurnOutcome { EndTurn, MaxTurns, Cancelled }
```

区分模型自然结束、达到轮次预算和取消。

### HandleError

```text
pub enum HandleError { ActionFailed(String), Unimplemented(String) }
```

宿主句柄的粗粒度失败类型，字符串携带可读原因。

取消是运行时执行契约的一部分。取消结果不代表所有外部副作用均已回滚。

<a id="output"></a>

## 接收流式输出

run 返回终态结果；流式输出发送到组装运行时时提供的 OutputStream。该接口包含文本、工具调用与结果、消息边界、通知和生命周期回调。需要客户端 DTO 的宿主可使用 client::adapter::AdapterOutputStream。

### OutputStream::emit_text

```text
async fn emit_text(&self, text: &Utf16JsonProjection)
```

将助手文本交给宿主输出目标。

<a id="snapshots"></a>

## 读取会话快照

### SessionHandle::id

```text
pub async fn id(&self) -> SessionId
```

当前挂载会话的标识。

### SessionHandle::transcript

```text
pub async fn transcript(&self) -> Vec<ConversationMessage>
```

读取既有的有序对话记录。

### SessionHandle::cost

```text
pub async fn cost(&self) -> CostSnapshot
```

读取累计用量与成本，包括 token 数量和 API 耗时。

公共门面访问已挂载的会话。创建、列出、恢复或切换会话由宿主组合及客户端命令负责。

<a id="session-errors"></a>

## 诊断失败轮次

1. 准入前记录已挂载 session ID 与宿主 turn_id。模型／供应商 profile 和 request ID 使用各自独立标识。
2. 随输出到达而消费；终态结果无法重建已丢弃的增量。用 tool_use_id 关联工具结果，不用显示名称。
3. 中断时取消所属令牌并等待 run。允许其他会话操作前，保留部分输出及返回结果或错误。
4. 冷恢复时，通过宿主组合加载持久化会话状态。一个 ID 加上空的新 runtime 并不等于恢复后的对话。

流协议检查包含截断流与工具结果配对。输出交付、权限响应路由和持久化发布需分别测试；TurnOutcome 单独不能证明这些屏障。

<a id="run-parameters"></a>

## 运行输入与返回值

| 值 | 含义 | 集成规则 |
| --- | --- | --- |
| RunInput.prompt: String | 传给已挂载执行路径的用户文本。 | 传入真实用户请求；系统指令由宿主配置负责。 |
| RunInput.images: Vec<PathBuf> | 由工作空间／模型实现解析的图像路径。 | 请求使用图像期间，保持执行宿主上的文件可读。 |
| CancellationToken | 共享取消信号，不是超时时长。 | 为停止按钮或宿主截止时间保留副本；取消后等待 run。 |
| Result<TurnOutcome, HandleError> | 执行终态结果，而不是流式助手文本。 | 从 OutputStream 读取输出，分别处理 outcome 与 error。 |

克隆 SessionHandle 会共享已挂载服务，不会创建新会话、分叉历史或授予并发轮次准入；这些决策由组合负责。

<a id="cancellation-recipe"></a>

## 连接停止按钮并保留结果

开始以下函数前，给调用方一个令牌副本。调用方可从界面或截止时间处理器取消该副本。函数等待既有 run 路径退出，并返回原始结果，包括在取消竞态中先完成的正常结果。

- EndTurn 表示执行路径自然结束，MaxTurns 表示轮次预算结束，Cancelled 表示明确取消。
- 错误或取消不会撤销工具已完成的文件写入、命令或外部请求。重试前检查该操作。
- 收到最终消息或错误边界前，将部分文本保留为部分文本。界面断开不代表运行时已停止。

### Rust · 完整函数，需要已组装宿主

```rust
use harness_runtime::{CancellationToken, HandleError, Harness, RunInput, TurnOutcome};

async fn submit(
    harness: &Harness,
    prompt: String,
    cancel: CancellationToken,
) -> Result<TurnOutcome, HandleError> {
    let session = harness.session();
    let session_id = session.id().await;
    let result = session.run(RunInput { prompt, images: vec![] }, cancel).await;
    let cost = session.cost().await;
    println!("session={session_id:?}, api_calls={}", cost.api_calls);
    result
}

// Before submit: let cancel = CancellationToken::new();
// Keep cancel.clone() in the host; call cancel.cancel() when the user stops.
```

验证方式: 需环境验证 (harness-cancellable-turn)

<a id="stream-rendering"></a>

## 渲染流并合并最终消息

1. 将文本增量追加到当前助手展示。通过提供的 UTF-16 JSON projection 保留源文本，避免把任意外部载荷经有损字符串往返转换。
2. 根据工具调用标识创建活动行。匹配结果到达时更新同一行；多个调用可能使用相同工具名。
3. 用最终消息边界或 MessageComplete DTO 确定助手行。已经渲染增量后，不要再次追加同一完整消息。
4. 需要累计统计时读取 cost()。对话快照、文本增量与成本更新承担不同职责。

- [事件字段与权限响应路由](harness-client.md#event-consumer)

## 源码与文档基线

[源码仓库](https://github.com/lingxi-coder/harness-runtime)

- [harness-runtime/crates/runtime/src/api.rs](https://github.com/lingxi-coder/harness-runtime/blob/a39da75f0985d61d90fffa255f7b4d90901b0e27/crates/runtime/src/api.rs) — 已提交源码 (a39da75f0985d61d90fffa255f7b4d90901b0e27)
- [harness-runtime/crates/runtime/src/desktop/sdk.rs](https://github.com/lingxi-coder/harness-runtime/blob/a39da75f0985d61d90fffa255f7b4d90901b0e27/crates/runtime/src/desktop/sdk.rs) — 已提交源码 (a39da75f0985d61d90fffa255f7b4d90901b0e27)
- [harness-runtime/crates/core/src/host/orchestrator.rs](https://github.com/lingxi-coder/harness-runtime/blob/a39da75f0985d61d90fffa255f7b4d90901b0e27/crates/core/src/host/orchestrator.rs) — 已提交源码 (a39da75f0985d61d90fffa255f7b4d90901b0e27)
- [harness-runtime/crates/client/src/adapter/output_stream.rs](https://github.com/lingxi-coder/harness-runtime/blob/a39da75f0985d61d90fffa255f7b4d90901b0e27/crates/client/src/adapter/output_stream.rs) — 已提交源码 (a39da75f0985d61d90fffa255f7b4d90901b0e27)

- harness-runtime: HEAD a39da75f0985d61d90fffa255f7b4d90901b0e27; 所据文件指纹 sha256:92c2108dc63215bc69ea9c11ea081795da883fdcb1c9c28b060aa9221a1b95a2; 产品固定版本 a39da75f0985d61d90fffa255f7b4d90901b0e27

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
