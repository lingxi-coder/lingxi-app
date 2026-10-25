# 流式响应与取消

读取标准化事件或逐传输块观测，保留工具、原生内容、用量与终止错误。

<a id="events"></a>

## 消费事件

ModelStream 自带异步 next()，无需导入 StreamExt。打开流或读取流时均可能失败。应一直读取到 None，包括 End 事件之后，再将报告视为最终结果。

### 完整流式函数

```rust
use lingxi_llm_client::protocol::{ChatRequest, StreamEvent};
use lingxi_llm_client::{LlmClient, RequestOptions};

async fn stream_text(
    client: &LlmClient,
    request: &ChatRequest,
    options: &RequestOptions,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut stream = client.chat().stream_in("openai", request, options).await?;
    while let Some(event) = stream.next().await {
        match event? {
            StreamEvent::TextDelta { text, .. } => print!("{text}"),
            StreamEvent::End { stop_reason, .. } => eprintln!("{stop_reason:?}"),
            _ => {}
        }
    }
    eprintln!("Profile: {}", stream.executed_profile());
    eprintln!("Usage: {:?}", stream.usage_report());
    eprintln!("Complete: {}", stream.usage_is_complete());
    Ok(())
}
```

验证方式: 编译检查 (llm-stream-text)

此显示示例忽略非文本事件。启用工具的应用在构造下一轮助手消息时，必须保留工具调用、推理签名、供应商原生内容与块边界。

<a id="event-handling-map"></a>

## 各类事件应该如何处理

| 事件 | 用途 | 不能据此推断 |
| --- | --- | --- |
| Start | 保存模型与可用响应身份 | 已经生成最终答案或可复用续接 |
| TextDelta / TextDeltaJsUtf16 | 更新显示块；有 UTF-16 sidecar 时保留精确内容 | 完整响应或可解析的 JSON 文档 |
| ToolCallDelta / BlockEnd | 按原块累计参数；执行前检查终止装配结果 | 可以执行尚未完整的参数 |
| ProviderContent / ThoughtSignature | 在助手历史中保存可重放内容与签名 | 仅用于显示的推理文本可以替换原生重放内容 |
| ProviderEvent / NativeControl | 单独保留供应商观测或控制数据 | 应发送给本地函数执行的调用 |
| End | 检查 stop_reason、usage 与 inference，随后继续读到 None | 每个终止响应都是成功且完整的最终答案 |

<a id="stream-result-checklist"></a>

## 正确构造最终消息

1. 界面显示文本时，用 StreamAccumulator 保留事件。按原始块索引保存内容，因为索引可能不连续，不同输出类型也可能交错出现。
2. 读取结束时检查 StreamAssembly.terminal、unfinished_blocks 与 incomplete_tools。MaxTokens 属于终止结果，但答案或工具参数仍可能被截断。
3. 在决定显示错误、执行工具或安排下一次调用前，保存 usage_report 与 inference_report。已经输出内容后的读取失败，不等于请求从未发送。
4. 为下一请求保存完整重建的助手消息。如果只保存了拼接显示文本，不应补造缺失签名、工具调用或供应商原生块。

- [无损装配与未完成块](llm-streaming.md#lossless-assembly)
- [失败后的逐次结算](llm-execution-accounting.md)

<a id="stream-ui-behavior"></a>

## 保持界面响应，同时保留协议语义

界面可合并更新，避免每个 token 都重绘，但应保留事件顺序与结算观测。界面消费滞后时限制队列长度，而不是无限累积字符串或事件。

用户停止时丢弃流并记录本次部分结果。之后重试是带新应用尝试身份的新请求，不保证从最后一个显示字符精确续传。只有完成的响应才提供供应商作用域续接。

<a id="batches"></a>

## 观测每个传输块

next_batch 最多读取一个传输块，返回事件、当前用量、推理事实与 finished 标记。即使事件为空，用量也可能变化；终止错误与最新用量同时保留。应先记录观测，再校验内容或让出执行权。

### Chunk observation · excerpt with an existing stream / 传输块观测片段

```rust
while let Some(batch) = stream.next_batch().await {
    // Persist these facts in the host before interpreting events.
    let usage = batch.usage;
    let inference = batch.inference;
    eprintln!("{usage:?} {inference:?}");
    for event in batch.events {
        let event = event?;
        // Dispatch or retain event in your application.
        eprintln!("{event:?}");
    }
}
```

验证方式: 接口摘录

### ModelStream::next

```text
pub async fn next(&mut self) -> Option<Result<StreamEvent, LlmError>>
```

读取一个标准化事件。

### ModelStream::next_batch

```text
pub async fn next_batch(&mut self) -> Option<StreamBatch>
```

读取一次结算观测，解码后不会等待下一个传输块。

### StreamBatch

```text
pub struct StreamBatch { pub events: Vec<Result<StreamEvent, LlmError>>, pub usage: UsageReport, pub inference: InferenceReport, pub finished: bool }
```

将事件与结算事实一起保留，包括仅用量传输块与终止错误。

### ModelStream::usage_is_complete

```text
pub fn usage_is_complete(&self) -> bool
```

估算实际成本前确认用量完整且内部一致。

<a id="cancel"></a>

## 取消与时限

- 丢弃 ModelStream 会释放底层字节流并断开读取。宿主保留已观测用量，并负责中断调用的结算。
- 流已返回后，读取失败不会触发自动回退。宿主可根据重试策略重新准备调用。
- 通过 RequestOptions.total_timeout 限制整条流的总时间。未设置时没有默认总时限。
- 仅完成的响应会提供续接引用。中断的流不会提供有效续接状态。

<a id="structured-websocket"></a>

## 结构化输出与 WebSocket

collect_structured_json 与 collect_structured<T> 先重建最终响应，再校验 JSON、Schema 或反序列化。单个增量并不是独立 JSON 文档。ResponsesSession 提供连接复用与续接；准备阶段在宿主准入前建立连接，发送阶段仅发出一次 response.create。

### ModelStream::collect_structured_json

```text
pub async fn collect_structured_json(self, format: &OutputFormat) -> Result<StructuredStreamResult<JsonPayload>, StructuredStreamError>
```

消费并校验最终 JSON，同时保留重建的响应与事件。

### ModelStream::continuation

```text
pub fn continuation(&self) -> Option<&ContinuationRef>
```

完整读取流之后获取绑定账户的续接状态。

PreparedCall 的 WebSocket 执行需要实现 connect_websocket 的 Transport。发送失败不会重发当前调用。宿主负责重连、取消以及是否准备下一次调用。

<a id="lossless-assembly"></a>

## 保留标准与原生流数据

宿主需要完整助手消息时，使用 StreamAccumulator 或 ModelStream::collect_response。组装保留稀疏块索引、完成顺序、思考签名、原生重放内容与精确工具输入 JSON。失败时部分组装暴露未完成块与不完整工具，不能当作成功终止工具请求。

| 事件族 | 消费者处理 |
| --- | --- |
| TextDelta / TextDeltaJsUtf16 | 按块显示并组装；存在精确 UTF-16 单元时保留 |
| ReasoningDelta / ThoughtSignature / RedactedThinking | 思考显示与重放签名/不透明数据职责不同 |
| ToolCallDelta / BlockEnd | 累积精确参数与供应商 ID；仅执行已校验完成的调用 |
| ProviderContent / Native / NativeDelta | 保留类型化/原生载荷与协议归属 |
| ProviderEvent / NativeControl | 仅观测/控制；不是可重放消息或本地工具调用 |
| Inference / WebSearch / FileSearch / End | 保留事实、归因与停止原因；继续读取直到 EOF |

最终 End 事件与完整 UsageReport 是不同事实。供应商流可能遗漏用量或在内容之后失败；估算成本需检查 usage_is_complete() 与最终推理事实。model_stream 与 structured_output 测试验证本地投影，不证明真实流延迟。

## 源码与文档基线

[源码仓库](https://github.com/lingxi-coder/llm-client)

- llm-client/src/client/stream.rs — 未发布源码 (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- llm-client/src/client/session.rs — 未发布源码 (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- llm-client/src/protocol/llm.rs — 未发布源码 (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- llm-client/src/stream_assembly.rs — 未发布源码 (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- llm-client/src/framing/sse.rs — 未发布源码 (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- llm-client/src/framing/eventstream.rs — 未发布源码 (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- llm-client/tests/model_stream.rs — 未发布源码 (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- llm-client/tests/structured_output.rs — 未发布源码 (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- llm-client/src/protocol/message.rs — 未发布源码 (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- llm-client/src/protocol/usage_report.rs — 未发布源码 (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)

- llm-client: HEAD ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b; 所据文件指纹 sha256:c68d002b005c15c21681b54d3f7efe358c4c1be6ec7cb07501009d1d11938e38; 产品固定版本 ff71b2ec1139bc67cc8719d78b1735aca967bbe8

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
