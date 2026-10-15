# 路由、失败与逐次结算

选择便利回退或宿主控制的单次调用，在解释输出前保留实际用量。

<a id="resolution"></a>

## 路由解析与备用凭据

供应商身份、精确连接、连接组、协议、协议模型与计费模式不同。resolve()/complete() 解析模型别名与有序连接；complete_in 选择起始连接或已配置组。PreparedCall::prepare_on 要求精确连接。区域过滤、隐藏连接、协议模型身份与计费兼容性限制回退；不会猜测有歧义的模型行。

RequestOptions.protocol 通过明确登记的 protocol_profiles 覆盖连接默认协议。各替代连接拥有自己的端点、认证与能力策略。credential 与请求级 authenticator 仅属于所选主连接；命名备用连接必须在 fallback_credentials 中获得自己的凭据。不能因为端点看似属于同一账户而复用主密钥。

| 执行路径 | 重放策略 | 用途 |
| --- | --- | --- |
| chat().complete/stream | 仅在触发条件与执行安全允许时按配置顺序回退；不是同连接重试循环 | 应用便利请求 |
| snapshot.prepare_on → dispatch_once | 一个精确连接，一次生成发送；无自动重试/回退 | 宿主准入与实际调用账本 |
| 音频/图像/嵌入/原生修改操作 | 操作特定的单次发送与未知结果约定 | 独立服务生命周期 |

<a id="failure-classes"></a>

## 错误与宿主重试决策

LlmError::kind 分类类型化错误；ProviderResponse 保留 HTTP 状态、请求 ID、原生响应体、分类与可用 retry_after。FailoverTriggers 分别覆盖限流/配额、过载、供应商内部错误/超时、网络/传输超时与认证/权限。独立默认连接没有隐式回退链。

- InvalidRequest、UnsupportedCapability、ContextOverflow、RequestTooLarge 与 TLS 证书错误不会成为通用切换端点触发器。应相应修正输入/配置、由宿主压缩上下文或修复传输信任。
- 流返回后读取失败不会回退。续接固定原账户、连接、端点与模型；有副作用的托管请求可在发送前抑制重放。
- Transport 不得静默重试或跟随携带凭据的重定向。宿主先记录已发送但结果不确定的尝试，再决定是否接受下一次调用；错误本身不能证明未执行或零成本。

<a id="attempt-example"></a>

## 解码前捕获观测

准备固定路由与价格，解析/上传已准入附件，编码最终字节并认证，不发送生成请求。PreparedCall 只消耗一次。RequestDraft 允许 seal 前的最后同步宿主修改；dispatch_once_with 在发送前立即标记宿主准入。准备阶段仍可能进行附件 I/O 或 WebSocket 建连。

在 decode 前收集用量与推理事实，因为失败状态或工具载荷非法仍可能携带结算事实。finish 消耗收集结果并执行附件清理。所有路径都应保留账本，包括发送不确定、流用量不完整、内容校验失败与取消。

### 固定价格并清理资源的一次实际调用

```rust
use lingxi_llm_client::{LlmClient, RequestOptions};
use lingxi_llm_client::codecs::RequestMode;
use lingxi_llm_client::protocol::{ChatRequest, Submission};

pub async fn one_attempt(
    client: &LlmClient,
    request: &ChatRequest,
    options: &RequestOptions,
) -> Result<(), Box<dyn std::error::Error>> {
    let snapshot = client.snapshot();
    let call = snapshot.prepare_on("openai", request, options, RequestMode::Complete).await?;
    let prices = call.pricing_snapshot();
    let received = call.dispatch_once().await?;
    let collected = received.collect().await?;
    let usage = collected.usage_report().clone();
    let inference = collected.inference_report().clone();
    // Persist these observations in the host before interpreting application content.
    let estimate = prices.estimate(&usage, &inference, Submission::Interactive);
    let response = collected.decode();
    collected.finish().await;
    println!("Usage: {usage:?}; estimate: {estimate:?}");
    println!("{}", response?.message.text());
    Ok(())
}
```

验证方式: 编译检查 (llm-attempt-accounting)

<a id="pricing-and-accounts"></a>

## 固定价格与独立账户报告

UsageReport 区分 Missing、Partial、Complete 与 Invalid 观测；缺失字段不是零。FrozenPricing 仅在用量与执行事实支持有效价格时估算，保留币种与所选模型/连接。实际等级未知时，仅请求 Fast 不足以定价。Anthropic 服务器 fallback-summary 成本有独立组件/完整性处理；部分组件不能当作完整总额。

client.account_usage(profile, query) 不生成模型输出。AccountQuery 区分 API/auth-user 身份、普通与 management/service 凭据、精确 selector 作用域及 UTC 时间范围。AccountMetric 对余额、token 历史、成本历史、配额窗口与订阅区分 available、unsupported、credential-required、not-reported 与 failed；账户汇总不是逐次账本或最终发票。

### 独立于生成查询精确账户

```rust
use lingxi_llm_client::{AccountIdentity, AccountQuery, AccountSnapshot, LlmClient};
use lingxi_llm_client::protocol::Secret;

pub async fn account_snapshot(
    client: &LlmClient,
    credential: Secret<String>,
) -> Result<AccountSnapshot, Box<dyn std::error::Error>> {
    let mut query = AccountQuery::new(AccountIdentity::ApiKey);
    query.credential = Some(credential);
    // Supply management_credential/selectors separately if this source requires them.
    Ok(client.account_usage("openai", &query).await?)
}
```

验证方式: 编译检查 (llm-account-query)

execution_contracts、failover 与计价测试使用模拟观测验证本地行为。本手册不声称已验证真实扣费、供应商发票、账户余额或完整设备验收。

## 源码与文档基线

[源码仓库](https://github.com/lingxi-coder/llm-client)

- [llm-client/src/client/executor.rs](https://github.com/lingxi-coder/llm-client/blob/7333dbc3795b6bad893cb7287088d80931538630/src/client/executor.rs) — 已提交源码 (7333dbc3795b6bad893cb7287088d80931538630)
- [llm-client/src/client/resolve.rs](https://github.com/lingxi-coder/llm-client/blob/7333dbc3795b6bad893cb7287088d80931538630/src/client/resolve.rs) — 已提交源码 (7333dbc3795b6bad893cb7287088d80931538630)
- [llm-client/src/client/prepared.rs](https://github.com/lingxi-coder/llm-client/blob/7333dbc3795b6bad893cb7287088d80931538630/src/client/prepared.rs) — 已提交源码 (7333dbc3795b6bad893cb7287088d80931538630)
- [llm-client/src/client/route.rs](https://github.com/lingxi-coder/llm-client/blob/7333dbc3795b6bad893cb7287088d80931538630/src/client/route.rs) — 已提交源码 (7333dbc3795b6bad893cb7287088d80931538630)
- [llm-client/src/client/options.rs](https://github.com/lingxi-coder/llm-client/blob/7333dbc3795b6bad893cb7287088d80931538630/src/client/options.rs) — 已提交源码 (7333dbc3795b6bad893cb7287088d80931538630)
- [llm-client/src/client/frozen_pricing.rs](https://github.com/lingxi-coder/llm-client/blob/7333dbc3795b6bad893cb7287088d80931538630/src/client/frozen_pricing.rs) — 已提交源码 (7333dbc3795b6bad893cb7287088d80931538630)
- [llm-client/src/protocol/usage_report.rs](https://github.com/lingxi-coder/llm-client/blob/7333dbc3795b6bad893cb7287088d80931538630/src/protocol/usage_report.rs) — 已提交源码 (7333dbc3795b6bad893cb7287088d80931538630)
- [llm-client/src/protocol/provider.rs](https://github.com/lingxi-coder/llm-client/blob/7333dbc3795b6bad893cb7287088d80931538630/src/protocol/provider.rs) — 已提交源码 (7333dbc3795b6bad893cb7287088d80931538630)
- [llm-client/src/protocol/llm.rs](https://github.com/lingxi-coder/llm-client/blob/7333dbc3795b6bad893cb7287088d80931538630/src/protocol/llm.rs) — 已提交源码 (7333dbc3795b6bad893cb7287088d80931538630)
- [llm-client/src/account/service.rs](https://github.com/lingxi-coder/llm-client/blob/7333dbc3795b6bad893cb7287088d80931538630/src/account/service.rs) — 已提交源码 (7333dbc3795b6bad893cb7287088d80931538630)
- [llm-client/src/account/types.rs](https://github.com/lingxi-coder/llm-client/blob/7333dbc3795b6bad893cb7287088d80931538630/src/account/types.rs) — 已提交源码 (7333dbc3795b6bad893cb7287088d80931538630)
- [llm-client/src/account/execution.rs](https://github.com/lingxi-coder/llm-client/blob/7333dbc3795b6bad893cb7287088d80931538630/src/account/execution.rs) — 已提交源码 (7333dbc3795b6bad893cb7287088d80931538630)
- [llm-client/tests/execution_contracts.rs](https://github.com/lingxi-coder/llm-client/blob/7333dbc3795b6bad893cb7287088d80931538630/tests/execution_contracts.rs) — 已提交源码 (7333dbc3795b6bad893cb7287088d80931538630)
- [llm-client/tests/failover.rs](https://github.com/lingxi-coder/llm-client/blob/7333dbc3795b6bad893cb7287088d80931538630/tests/failover.rs) — 已提交源码 (7333dbc3795b6bad893cb7287088d80931538630)
- [llm-client/tests/server_fallback_pricing.rs](https://github.com/lingxi-coder/llm-client/blob/7333dbc3795b6bad893cb7287088d80931538630/tests/server_fallback_pricing.rs) — 已提交源码 (7333dbc3795b6bad893cb7287088d80931538630)

- llm-client: HEAD 7333dbc3795b6bad893cb7287088d80931538630; 所据文件指纹 sha256:51afdece5bf1e2d135f676f447d6ff43c84aaa464328f96ba19fa1e5010928af; 产品固定版本 7333dbc3795b6bad893cb7287088d80931538630

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
