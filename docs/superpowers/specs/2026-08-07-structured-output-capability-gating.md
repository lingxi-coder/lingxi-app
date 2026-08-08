# 待办：结构化输出需要按 provider 能力门控

日期：2026-08-07
发现方式：真机 QA（DeepSeek + thinking 模式），用户两分钟内撞上
状态：**未修**。本地应用的三段 LLM 调用在受影响的 provider/模型组合上完全不可用。

## 症状

创建本地应用时弹出：

```
llm unavailable: invalid request: 400 {"error":{"message":
"Thinking mode does not support this tool_choice","type":"invalid_request_error"}}
```

## 根因

`apps/engine-mobile/src/local_apps_llm.rs:144` 用**强制工具调用**拿结构化输出：

```rust
Some(ToolChoice::Tool { name: tool_name.to_string() })
```

这是一条**硬编的 Claude 语义假设**：「强制 tool_choice 到哪个 provider 都能用」。DeepSeek 在
thinking 模式下不支持。而 `messages_create_side_query` 的文档明说 thinking 配置是**会话级共享**
的 —— 三次调用继承会话设置，于是出题、出方案、写码三步全挂。

**仓库里本来就有这套抽象，我没用。** `llm-client/src/config.rs:210` 的 `Capabilities` 带着
`reasoning` 和 `structured_output` 两个位，`provider_settings.rs:488/500` 不同 provider 的值确实
不同，`protocol.rs:875` 还有现成的门控先例（请求带 `response_format` 但 provider 不支持时报
`capability` 错）。`local_apps_llm.rs` 里 `Capabilities` 出现 4 次 —— **全在注释里，一次没真查过**。

这是「Claude 语义硬算、没按 provider 门控」这类旧账的又一例（llm-client 此前已记录 14 处）。

## 修法

1. 调用前查目标 profile 的 `Capabilities`。
2. `structured_output` 支持 → 用 `ResponseFormat::JsonSchema`（`protocol.rs:663`）。
3. 仅在支持强制工具**且**当前推理设置不冲突时才用 `ToolChoice::Tool`。
4. 都不可用时明确报**能力不匹配**，不要报 `llm unavailable`。

⚠️ 组合维度是 provider × 模型 × 推理开关的**三元组**，不是一个布尔位。

## 顺带

**错误分类偏了。** 400「你的请求形状我不支持」被报成 `LlmUnavailable`（模型够不到），用户会去查
网络，但重试多少次都一样。这类应归入「请求构造与能力不匹配」，与 `LlmOutputRejected`（模型答了但
答得不合格）也不是一回事。

**顺带核对 `provider_settings.rs` 里 DeepSeek 那条的能力位** —— 若它现在声称支持强制工具而实际在
thinking 下不支持，这个 bug 有一半在数据里，不在调用方。

## 为什么 20 轮评审没抓到

T8 特意设计 `LocalAppsModel` trait 让三段逻辑能用**假模型**单测 —— 设计本身是对的，代价是**真实请求
形状从没打到过真实 provider**。评审明确指出过「`ApiServiceModel` 零测试」，当时判断残余风险低，理由
是参数顺序已人工核对。判断错了：风险不在参数顺序，在**语义兼容性**，而那只有真机 + 真 provider 能暴露。
