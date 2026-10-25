# 输出、推理与缓存控制

应用已校验的请求级约定，不弱化 Schema，并区分推理预算与金额限制。

<a id="inference-controls"></a>

## Thinking、effort 与服务等级

ThinkingConfig 包含可选 mode（Disabled/Enabled/Adaptive）、budget（Tokens/Dynamic）与 effort。ServiceTier 独立选择 Standard/Fast。所选模型行与连接声明的协议映射决定可编码组合；不支持的显式控制会准入失败。缺失元数据表示未知，不表示支持。

| 控制项 | 含义 | 结算含义 |
| --- | --- | --- |
| ThinkingBudget::Tokens | 供应商推理 token 预算或目标，由声明的 min/max/hard_limit 描述 | 不是金额预算；宿主仍需控制支出准入 |
| ReasoningEffort | 当前 mode 下明确支持的等级 | 可能改变 token 消耗；本身不修改单位价格 |
| ServiceTier::Fast | 端点/模型特定的快速协议选项 | 区分请求与实际执行等级；执行等级未知时不能确认 Fast 价格 |

InferenceFeatures 覆盖会清理与模式不兼容的默认值。请求级观测 fast_capability 仅影响所选主模型，不影响共享配置或备用账户。InferenceReport 保留供应商报告的执行事实。

<a id="output-contract-choice"></a>

## 选择恰当的输出约定

精确 JSON Schema 源文使用 JsonPayload::parse；由自己配置界面构造的小型 Schema 可用 from_generated_value。供应商 JSON 和未完整的工具参数应保留为携带源文的数据，直到到达完整且已校验的应用边界。

| 约定 | 用途 | 需要校验 |
| --- | --- | --- |
| OutputFormat::Text | 供人阅读的文本 | 终止原因与应用内容要求 |
| OutputFormat::JsonObject | 由应用自行检查字段的 JSON 对象 | 对象根节点、应用必需字段与值类型 |
| OutputFormat::JsonSchema | 路由允许的类型化对象/数组约定 | 原始 Schema、最终响应、Schema 匹配与目标类型反序列化 |
| ToolSpec.strict | 约束函数工具参数 | 工具参数校验独立于最终答案输出约定 |

<a id="structured-result-errors"></a>

## 处理结构化结果失败

| StructuredOutputErrorKind | 含义 | 动作 |
| --- | --- | --- |
| Incomplete | 不是完整最终答案，或响应仍包含工具调用 | 先处理拒绝、截断或工具流程，再解析应用数据 |
| NoContract | 请求使用了文本约定 | 显式设置所需 JSON output_format |
| InvalidJson / SchemaMismatch | 最终输出 JSON 非法或违反所请求 Schema | 保留响应与用量，不将其当作应用数据接受 |
| InvalidSchema / Deserialization | 本地 Schema 或目标 Rust 类型无法接受该值 | 修正应用约定/类型，不静默弱化供应商约束 |

StructuredOutputError 保留响应。StructuredStreamError 对读取失败、缺少 End、大小上限与校验失败还保留已观测事件。记录内容错误时不应丢弃相关用量事实。

<a id="schema-design-recipe"></a>

## 让 Schema 真正服务应用

1. 从应用实际消费的对象出发，使用稳定字段名、显式类型及必要字段。枚举值应与应用真实接受的 ID 一致。
2. 发送前选择具有所需严格 Schema 支持的路由。兼容协议或模型名称本身不能证明支持所有 Schema 关键字。
3. 将同一 OutputFormat 用于请求和 structured_json/structured。流式响应先收集完整结果，不将各个文本增量独立解析为 JSON。
4. 将校验失败与传输失败、重试策略分开。已完成但不合业务要求的结果，不能成为绕过约定或自动切换账户的理由。

<a id="structured-output"></a>

## 严格输出与本地校验

OutputFormat 为 Text、JsonObject 或 JsonSchema { name, schema, strict }，独立于严格函数工具输入。编码器保留 Schema 约束并拒绝不支持或冲突的选项，不为调用成功而改写 Schema。本地校验禁用外部网络/文件解析。仅对完成响应解码；拒绝、截断、工具与暂停是不同结果。

- 流收集器先重建终止内容再校验；单个 JSON 增量不是独立文档。校验错误保留响应、事件与用量，供宿主结算。
- 第一方 Qwen 与 DeepSeek JSON Object 请求要求显式 system/user 文本出现 JSON/json；助手文本、工具描述与附件载荷不计入检查。严格 Schema 支持按模型/路由准入，而非按协议兼容性判断。

### 完成并校验同一输出约定

```rust
use lingxi_llm_client::{LlmClient, RequestOptions};
use lingxi_llm_client::protocol::{ChatRequest, ConversationMessage, JsonPayload, OutputFormat};

pub async fn structured_answer(
    client: &LlmClient,
    options: &RequestOptions,
) -> Result<JsonPayload, Box<dyn std::error::Error>> {
    let mut request = ChatRequest::new("gpt-6-luna");
    request.output_format = OutputFormat::JsonSchema {
        name: "answer".into(), strict: true,
        schema: JsonPayload::parse(r#"{"type":"object","properties":{"answer":{"type":"string"}},
            "required":["answer"],"additionalProperties":false}"#)?,
    };
    request.messages.push(ConversationMessage::user_text("Give a short greeting."));
    let response = client.chat().complete_in("openai", &request, options).await?;
    Ok(response.structured_json(&request.output_format)?)
}
```

验证方式: 编译检查 (llm-structured-output)

<a id="finite-decisions"></a>

## 有限选项单选决策

DecisionRequest 在一个或多个问题间共享文本/图像上下文，每个问题与选项 ID 必须非空且唯一。DecisionService 使用已验证的严格结构化输出路由。models() 列出准入模型；任意目录 structured_output 标记、兼容网关或账户不足以准入。

### 保留调用报告的有限选项问题

```rust
use lingxi_llm_client::{LlmClient, RequestOptions};
use lingxi_llm_client::protocol::{DecisionContextPart, DecisionOption, DecisionQuestion, DecisionRequest, DecisionResult};

pub async fn classify(
    client: &LlmClient,
    options: &RequestOptions,
) -> Result<DecisionResult, Box<dyn std::error::Error>> {
    let request = DecisionRequest {
        model: "gpt-6-luna".into(),
        context: vec![DecisionContextPart::Text { text: "The build completed successfully.".into() }],
        questions: vec![DecisionQuestion {
            id: "build_status".into(), prompt: "What is the build status?".into(),
            options: vec![
                DecisionOption { id: "passed".into(), label: "Passed".into() },
                DecisionOption { id: "failed".into(), label: "Failed".into() },
            ],
        }],
    };
    Ok(client.decisions().decide_in("openai", &request, options).await?)
}
```

验证方式: 编译检查 (llm-decision)

Refused、Incomplete、InvalidResult 与 ProviderWithReport 在可用时保留调用报告。每个原问题必须返回已提供的选项 ID；不合法完成结果不会弱化约束或回退。report.usage 对应最后一次尝试，prior_attempts 分开保留更早已报告的用量。

<a id="caches-and-embeddings"></a>

## 提示缓存、响应缓存与嵌入

ChatRequest.prompt_cache 包含 TTL 与原请求中的显式断点；空策略保留供应商默认行为。原生 OpenAI Responses 缓存控制、Gemini 上下文缓存资源与 OpenRouter 网关响应缓存策略具有独立约定。响应缓存 HIT/MISS 观测来自已知响应头，不能从零用量推断。不能因某一路由支持缓存而在其他路由编码同一字段。

client.embeddings().embed 使用独立配置路由，并遵守 ServiceSetting 继承、启用与禁用语义。文本向量保持输入顺序；索引或数量非法、非有限值与维度不一致均报错。客户端不会拆分、归一化、截断、重试或借 Chat 模型目录解释嵌入模型 ID。多模态 Gemini embedding 是独立类型化原生 API。

- 本地 tokenizer 估算不进行凭据、网络或远程文件解析，并明确列出不支持/遗漏内容。仅估算首选路由，不能作为权威结费用量。
- structured_output、decisions、inference_controls 与 embeddings 测试验证模拟协议约定与错误保留。严格输出准确性、真实缓存命中率与供应商模型权限需要真实账户证据。

## 源码与文档基线

[源码仓库](https://github.com/lingxi-coder/llm-client)

- [llm-client/src/protocol/inference.rs](https://github.com/lingxi-coder/llm-client/blob/ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b/src/protocol/inference.rs) — 已提交源码 (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- [llm-client/src/protocol/request_controls.rs](https://github.com/lingxi-coder/llm-client/blob/ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b/src/protocol/request_controls.rs) — 已提交源码 (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- [llm-client/src/protocol/structured.rs](https://github.com/lingxi-coder/llm-client/blob/ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b/src/protocol/structured.rs) — 已提交源码 (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- [llm-client/src/codecs/inference.rs](https://github.com/lingxi-coder/llm-client/blob/ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b/src/codecs/inference.rs) — 已提交源码 (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- [llm-client/src/codecs/structured.rs](https://github.com/lingxi-coder/llm-client/blob/ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b/src/codecs/structured.rs) — 已提交源码 (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- [llm-client/src/codecs/cache.rs](https://github.com/lingxi-coder/llm-client/blob/ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b/src/codecs/cache.rs) — 已提交源码 (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- [llm-client/src/client/decisions.rs](https://github.com/lingxi-coder/llm-client/blob/ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b/src/client/decisions.rs) — 已提交源码 (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- [llm-client/src/protocol/decision.rs](https://github.com/lingxi-coder/llm-client/blob/ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b/src/protocol/decision.rs) — 已提交源码 (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- [llm-client/src/embeddings/mod.rs](https://github.com/lingxi-coder/llm-client/blob/ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b/src/embeddings/mod.rs) — 已提交源码 (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- [llm-client/src/token_count/request.rs](https://github.com/lingxi-coder/llm-client/blob/ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b/src/token_count/request.rs) — 已提交源码 (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- [llm-client/src/providers/openrouter/response_cache.rs](https://github.com/lingxi-coder/llm-client/blob/ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b/src/providers/openrouter/response_cache.rs) — 已提交源码 (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- [llm-client/tests/structured_output.rs](https://github.com/lingxi-coder/llm-client/blob/ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b/tests/structured_output.rs) — 已提交源码 (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- [llm-client/tests/decisions.rs](https://github.com/lingxi-coder/llm-client/blob/ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b/tests/decisions.rs) — 已提交源码 (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- [llm-client/tests/embeddings.rs](https://github.com/lingxi-coder/llm-client/blob/ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b/tests/embeddings.rs) — 已提交源码 (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- [llm-client/src/protocol/json_payload.rs](https://github.com/lingxi-coder/llm-client/blob/ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b/src/protocol/json_payload.rs) — 已提交源码 (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- [llm-client/src/client/stream.rs](https://github.com/lingxi-coder/llm-client/blob/ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b/src/client/stream.rs) — 已提交源码 (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)

- llm-client: HEAD ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b; 所据文件指纹 sha256:c68d002b005c15c21681b54d3f7efe358c4c1be6ec7cb07501009d1d11938e38; 产品固定版本 ff71b2ec1139bc67cc8719d78b1735aca967bbe8

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
