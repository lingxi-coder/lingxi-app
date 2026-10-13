# 供应商、目录与计价

绑定连接、按区域过滤、管理目录、访问原生资源，并依据观测事实估算成本。

<a id="profiles"></a>

## 供应商连接配置

provider 标识服务供应商；profile 标识连接，包括协议、端点、模型、认证策略和可选原生服务路由。内置配置覆盖 OpenAI、Anthropic、Gemini、DeepSeek、Kimi、Qwen、MiniMax、GLM/Z.AI、xAI、OpenRouter 与 GitHub Copilot。

内置协议包括 OpenAI Responses、Chat Completions、Anthropic Messages 与 Gemini。Azure、Bedrock、Vertex 与 Foundry 有可配置连接的适配器；适配器并不代表内置账户或端点。

- 通过 client.providers() 查看可见连接，通过 client.chat().models() 查看对话模型，通过 client.images().models() 查看图像模型。
- 创建客户端前选择 Region::ChinaMainland 或 Region::International。列表、解析与回退均遵守该区域；未声明 regions 的自定义连接允许两者。
- Kimi、Qwen 与 MiniMax 的区域连接使用不同凭据。连接必须与账户和密钥所在区域一致，区域间密钥不可互换。

CapabilitySupport::Unknown 与 Unsupported 不同。目录元数据、已实现适配器、账户权限和成功的真实调用分别代表不同的事实。

<a id="managed-catalog"></a>

## 管理持久化配置

### Managed client · async-host excerpt / 托管客户端片段

```rust
use lingxi_llm_client::{builtin_providers, LlmClientBuilder};
use lingxi_llm_client::protocol::Region;

let profiles = builtin_providers()?.into_iter()
        .filter(|profile| profile.profile_name == "openai").collect::<Vec<_>>();
let (client, config) = LlmClientBuilder::new(&profiles)?
    .with_region(Region::International)
    .build_managed()?;
config.set_config_dir("./config").await?;
let snapshot = client.snapshot();
let profile = snapshot.profile("openai");
```

验证方式: 接口摘录

### LlmClientBuilder::build_managed

```text
pub fn build_managed(self) -> Result<(LlmClient, ClientConfigManager), BuildError>
```

返回共享客户端与独立的配置管理器。

### ClientConfigManager::set_config_dir

```text
pub async fn set_config_dir(&self, path: impl AsRef<Path>) -> Result<(), ProviderStoreError>
```

加载并持久化 providers.json。每次启动时调用以恢复配置。

### ClientConfigManager::add_provider

```text
pub async fn add_provider(&self, profile: ProviderProfile) -> Result<(), ProviderStoreError>
```

添加或完整替换一个连接配置。

### ClientConfigManager::sync_provider

```text
pub async fn sync_provider(&self, name: &str, credential: Option<&Secret<String>>) -> Result<usize, ProviderStoreError>
```

使用该连接账户的凭据及受支持的目录 API 刷新模型。

### ClientConfigManager::set_model_visibility

```text
pub async fn set_model_visibility(&self, name: &str, wire: &str, visible: bool) -> Result<(), ProviderStoreError>
```

设置所选模型行的可见性。

配置 v3 会以 UnsupportedVersion 拒绝 v1/v2，不会静默迁移。更新为后续操作发布新快照；进行中的调用保留已捕获的配置。

<a id="native-services"></a>

## 供应商原生资源

使用 client.provider::<T>(profile) 将精确连接绑定到供应商客户端。公开客户端包括 OpenAiClient、AnthropicClient、GoogleClient、DeepSeekClient、KimiClient、QwenClient、MiniMaxClient、ZhipuClient、XaiClient、OpenRouterClient 与 GithubCopilotClient。

原生资源保留独立类型与支持边界：文件、检索、批处理、后台任务、远程工具、语音和实时会话因供应商而异。完整资源类型与签名见生成的当前 Rustdoc，作用域与生命周期限制见供应商资源章节。实时音频设备采集与播放仍由宿主负责。

### Provider binding · excerpt with an existing client / 供应商绑定片段

```rust
use lingxi_llm_client::providers::OpenAiClient;

let openai = client.provider::<OpenAiClient>("openai")?;
let audio = openai.audio();
let batches = openai.batches();
let retrieval = openai.retrieval();
let background = openai.background();
```

验证方式: 接口摘录

### LlmClient::provider

```text
pub fn provider<T: ProviderClient>(&self, profile_name: &str) -> Result<T, ProviderBindingError>
```

拒绝不匹配的供应商与连接绑定，不会推测其他路由。

### EmbeddingService::embed

```text
pub async fn embed(self, profile_name: &str, input: &EmbeddingRequest, options: &RequestOptions) -> Result<EmbeddingResponse, EmbeddingError>
```

通过启用的独立路由嵌入文本。校验模型限制、维度与任务支持，不会自动拆分。

<a id="pricing"></a>

## 用量与固定价格

将实际执行连接、UsageReport 与 InferenceReport 一起保留。FrozenPricing 在发送前捕获所选模型行，因此后续目录更新不会改变该次调用的价格。quote() 支持准入报价；estimate() 要求完整有效的用量与已确认的执行事实。

### FrozenPricing::capture

```text
pub fn capture(profile: &ProviderProfile, display_model: &str, request_model: &str) -> Result<Self, LlmError>
```

从配置中捕获无歧义的精确模型行。

### FrozenPricing::quote

```text
pub fn quote(&self, context: &PricingContext) -> Result<PriceQuote, LlmError>
```

为所选费率报价，不将其表示为实际用量。

### FrozenPricing::estimate

```text
pub fn estimate(&self, report: &UsageReport, inference: &InferenceReport, submission: Submission) -> Result<CostEstimate, LlmError>
```

用固定费率估算已观测调用并保留币种。部分用量、未公布价格或未知的实际 Fast 等级会返回 CostUnavailable。

### LlmClient::estimate_local_tokens_in

```text
pub fn estimate_local_tokens_in(&self, profile: &str, request: &ChatRequest) -> Result<LocalTokenEstimate, LocalTokenCountError>
```

使用已启用且已映射的分词器离线估算可见输入；明确报告遗漏项，不代表供应商结费用量。

推理 effort 改变 token 消耗，不改变声明的单位费率。目录与成本估算不代表真实供应商可用性、最终账单或宿主结算账本。

<a id="authentication-hosting"></a>

## 认证与云托管边界

供应商兼容网关不会因共享协议而成为官方供应商。精确端点与供应商身份检查限定原生资源、决策、托管工具与账户 API。Builder codec 支持与模型元数据证明本地能力；账户授权与区域端点访问仍独立。

| 路由/认证类型 | 宿主职责 | SDK 职责 |
| --- | --- | --- |
| API key / bearer | 选择账户并刷新/存储有效凭据 | 按所选路由一次性编码配置请求头 |
| 受限 ChatGPT / Copilot 路由 | 提供有授权的账户 token 与 grant 状态 | 执行允许的路由/资源策略；不等同通用 API key |
| Bedrock AWS SigV4 | 提供凭据与精确签名配置 | 编码托管协议并认证最终字节 |
| Vertex / Azure / Foundry | 提供 project/location 或 deployment/resource 与适当凭据 | 使用已声明包装协议；原生服务需单独启用 |

<a id="catalog-provenance"></a>

## 目录来源与验证

仓库中的供应商 TOML 与能力矩阵描述当前实现/准入约定，按供应商、模型、协议记录 supported、unsupported 或 unknown。目录同步可添加观测模型，不编造价格或原生能力。模型从静态目录移除后可保留观测身份，但不保留撤回的静态价格。

- 验证结果记录所选连接、协议模型、源码基线与账户区域。编译/模拟测试证明公共 API/协议行为；真实请求单独证明某一账户在当时接受了某一操作。
- 本地 SDK HEAD 与产品依赖修订不同会显示于手册源码数据。工作树 path 消费者与 Git 固定安装分别验证，不能为示例编译而改生产清单或锁文件。

- [当前协议能力矩阵源码](https://github.com/lingxi-coder/llm-client)
- [逐次用量与账户查询的区别](llm-execution-accounting.md)

## 源码与文档基线

[源码仓库](https://github.com/lingxi-coder/llm-client)

- llm-client/src/client/store.rs — 未提交源码 (ce32fd07402c13fbfcfa185942162a87907ea0a1)
- llm-client/src/client/resolve.rs — 未提交源码 (ce32fd07402c13fbfcfa185942162a87907ea0a1)
- [llm-client/src/configuration/model.rs](https://github.com/lingxi-coder/llm-client/blob/ce32fd07402c13fbfcfa185942162a87907ea0a1/src/configuration/model.rs) — 已提交源码 (ce32fd07402c13fbfcfa185942162a87907ea0a1)
- [llm-client/src/configuration/sync.rs](https://github.com/lingxi-coder/llm-client/blob/ce32fd07402c13fbfcfa185942162a87907ea0a1/src/configuration/sync.rs) — 已提交源码 (ce32fd07402c13fbfcfa185942162a87907ea0a1)
- llm-client/src/protocol/provider.rs — 未提交源码 (ce32fd07402c13fbfcfa185942162a87907ea0a1)
- [llm-client/src/client/frozen_pricing.rs](https://github.com/lingxi-coder/llm-client/blob/ce32fd07402c13fbfcfa185942162a87907ea0a1/src/client/frozen_pricing.rs) — 已提交源码 (ce32fd07402c13fbfcfa185942162a87907ea0a1)
- [llm-client/src/token_count/request.rs](https://github.com/lingxi-coder/llm-client/blob/ce32fd07402c13fbfcfa185942162a87907ea0a1/src/token_count/request.rs) — 已提交源码 (ce32fd07402c13fbfcfa185942162a87907ea0a1)
- llm-client/data/capability-matrix-protocols.json — 未提交源码 (ce32fd07402c13fbfcfa185942162a87907ea0a1)
- llm-client/data/capability-audit.json — 未提交源码 (ce32fd07402c13fbfcfa185942162a87907ea0a1)
- llm-client/data/providers/openai.toml — 未提交源码 (ce32fd07402c13fbfcfa185942162a87907ea0a1)
- llm-client/data/providers/anthropic.toml — 未提交源码 (ce32fd07402c13fbfcfa185942162a87907ea0a1)
- llm-client/data/providers/qwen.toml — 未提交源码 (ce32fd07402c13fbfcfa185942162a87907ea0a1)
- llm-client/tests/regions.rs — 未提交源码 (ce32fd07402c13fbfcfa185942162a87907ea0a1)

- llm-client: HEAD ce32fd07402c13fbfcfa185942162a87907ea0a1; 所据文件指纹 sha256:a78ca8575ddb70589a8cc04ff4f043825bd52db40d751181584ea10118bfe2b4; 产品固定版本 87db6e234e32cbc251a128d6b370cdda36c4d0c0

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
