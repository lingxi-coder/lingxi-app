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

<a id="structured-output"></a>

## 严格输出与本地校验

OutputFormat 为 Text、JsonObject 或 JsonSchema { name, schema, strict }，独立于严格函数工具输入。编码器保留 Schema 约束并拒绝不支持或冲突的选项，不为调用成功而改写 Schema。本地校验禁用外部网络/文件解析。仅对完成响应解码；拒绝、截断、工具与暂停是不同结果。

- 流收集器先重建终止内容再校验；单个 JSON 增量不是独立文档。校验错误保留响应、事件与用量，供宿主结算。
- 第一方 Qwen 与 DeepSeek JSON Object 请求要求显式 system/user 文本出现 JSON/json；助手文本、工具描述与附件载荷不计入检查。严格 Schema 支持按模型/路由准入，而非按协议兼容性判断。

### 完成并校验同一输出约定

```rust
use lingxi_llm_client::{LlmClient, RequestOptions};
use lingxi_llm_client::protocol::{ChatRequest, ConversationMessage, OutputFormat};
use serde_json::{json, Value};

pub async fn structured_answer(
    client: &LlmClient,
    options: &RequestOptions,
) -> Result<Value, Box<dyn std::error::Error>> {
    let mut request = ChatRequest::new("gpt-6-luna");
    request.output_format = OutputFormat::JsonSchema {
        name: "answer".into(), strict: true,
        schema: json!({"type":"object","properties":{"answer":{"type":"string"}},
            "required":["answer"],"additionalProperties":false}),
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

- [llm-client/src/protocol/inference.rs](https://github.com/lingxi-coder/llm-client/blob/a363c5b3a237d53eea12c3abfb3b98648ef15d3e/src/protocol/inference.rs) — 已提交源码 (a363c5b3a237d53eea12c3abfb3b98648ef15d3e)
- [llm-client/src/protocol/request_controls.rs](https://github.com/lingxi-coder/llm-client/blob/a363c5b3a237d53eea12c3abfb3b98648ef15d3e/src/protocol/request_controls.rs) — 已提交源码 (a363c5b3a237d53eea12c3abfb3b98648ef15d3e)
- [llm-client/src/protocol/structured.rs](https://github.com/lingxi-coder/llm-client/blob/a363c5b3a237d53eea12c3abfb3b98648ef15d3e/src/protocol/structured.rs) — 已提交源码 (a363c5b3a237d53eea12c3abfb3b98648ef15d3e)
- [llm-client/src/codecs/inference.rs](https://github.com/lingxi-coder/llm-client/blob/a363c5b3a237d53eea12c3abfb3b98648ef15d3e/src/codecs/inference.rs) — 已提交源码 (a363c5b3a237d53eea12c3abfb3b98648ef15d3e)
- [llm-client/src/codecs/structured.rs](https://github.com/lingxi-coder/llm-client/blob/a363c5b3a237d53eea12c3abfb3b98648ef15d3e/src/codecs/structured.rs) — 已提交源码 (a363c5b3a237d53eea12c3abfb3b98648ef15d3e)
- [llm-client/src/codecs/cache.rs](https://github.com/lingxi-coder/llm-client/blob/a363c5b3a237d53eea12c3abfb3b98648ef15d3e/src/codecs/cache.rs) — 已提交源码 (a363c5b3a237d53eea12c3abfb3b98648ef15d3e)
- [llm-client/src/client/decisions.rs](https://github.com/lingxi-coder/llm-client/blob/a363c5b3a237d53eea12c3abfb3b98648ef15d3e/src/client/decisions.rs) — 已提交源码 (a363c5b3a237d53eea12c3abfb3b98648ef15d3e)
- [llm-client/src/protocol/decision.rs](https://github.com/lingxi-coder/llm-client/blob/a363c5b3a237d53eea12c3abfb3b98648ef15d3e/src/protocol/decision.rs) — 已提交源码 (a363c5b3a237d53eea12c3abfb3b98648ef15d3e)
- [llm-client/src/embeddings/mod.rs](https://github.com/lingxi-coder/llm-client/blob/a363c5b3a237d53eea12c3abfb3b98648ef15d3e/src/embeddings/mod.rs) — 已提交源码 (a363c5b3a237d53eea12c3abfb3b98648ef15d3e)
- [llm-client/src/token_count/request.rs](https://github.com/lingxi-coder/llm-client/blob/a363c5b3a237d53eea12c3abfb3b98648ef15d3e/src/token_count/request.rs) — 已提交源码 (a363c5b3a237d53eea12c3abfb3b98648ef15d3e)
- [llm-client/src/providers/openrouter/response_cache.rs](https://github.com/lingxi-coder/llm-client/blob/a363c5b3a237d53eea12c3abfb3b98648ef15d3e/src/providers/openrouter/response_cache.rs) — 已提交源码 (a363c5b3a237d53eea12c3abfb3b98648ef15d3e)
- [llm-client/tests/structured_output.rs](https://github.com/lingxi-coder/llm-client/blob/a363c5b3a237d53eea12c3abfb3b98648ef15d3e/tests/structured_output.rs) — 已提交源码 (a363c5b3a237d53eea12c3abfb3b98648ef15d3e)
- [llm-client/tests/decisions.rs](https://github.com/lingxi-coder/llm-client/blob/a363c5b3a237d53eea12c3abfb3b98648ef15d3e/tests/decisions.rs) — 已提交源码 (a363c5b3a237d53eea12c3abfb3b98648ef15d3e)
- [llm-client/tests/embeddings.rs](https://github.com/lingxi-coder/llm-client/blob/a363c5b3a237d53eea12c3abfb3b98648ef15d3e/tests/embeddings.rs) — 已提交源码 (a363c5b3a237d53eea12c3abfb3b98648ef15d3e)

- llm-client: HEAD a363c5b3a237d53eea12c3abfb3b98648ef15d3e; 所据文件指纹 sha256:fdc3f37adcc11c571e4db361c4ea665e8ee4d2644a8724745b22389fb43c41d7; 产品固定版本 a363c5b3a237d53eea12c3abfb3b98648ef15d3e

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
