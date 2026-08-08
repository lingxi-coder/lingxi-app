# 待办：结构化输出需要按 provider 能力门控

日期：2026-08-07（2026-08-08 用真 API 复测并更正根因）
发现方式：真机 QA（DeepSeek），用户两分钟内撞上
状态：**已缓解，未根治**。`254caa0a9` 加了「被拒则降级重试」的自纠正回退，本地应用在 DeepSeek
上已可用；按能力门控这件事仍未做。

## 症状

创建本地应用时弹出：

```
llm unavailable: invalid request: 400 {"error":{"message":
"Thinking mode does not support this tool_choice","type":"invalid_request_error"}}
```

## 实测矩阵（api.deepseek.com，真 key，逐条观测）

| 请求 | 结果 |
|---|---|
| thinking on + tools + `tool_choice=required` | **400** |
| thinking on + tools + `tool_choice={具名函数}` | **400** |
| **完全不发 `thinking` 字段** + tools + `tool_choice` | **400** |
| thinking on + tools + **无 `tool_choice`** | 200，`tool_calls` ✅ |
| thinking **off** + tools + `tool_choice=required` | 200，`tool_calls` ✅ |
| thinking on + `response_format: json_object` | 200，干净 JSON ✅ |

写码段的真实形状（files 数组 schema、8000 tokens）单独复测：无强制、thinking 默认开，
仍正常返回 `emit_sources`（3 文件 / 14 KB / `finish_reason: tool_calls`）。**降级路径是验证过的，
不是赌运气。**

## 根因

`local_apps_llm.rs` 用**强制工具调用**拿结构化输出，这是一条**硬编的 Claude 语义假设**：
「强制 `tool_choice` 到哪个 provider 都能用」。DeepSeek 在 thinking 模式下不支持。

⚠️ **初版此处的分析是错的，已更正。** 原文写「`messages_create_side_query` 的 thinking 配置
是会话级共享的，三次调用继承会话设置」。实测否掉了：**DeepSeek V4 服务端默认 thinking 就是开的**
（官方文档：*"Thinking mode is enabled by default, with the default effort being `high`"*），
而 `deepseek_legacy_model`（`llm-client/src/providers/openai.rs:61`）只映射了旧的
`deepseek-chat`/`deepseek-reasoner` 两个 id，原生的 `deepseek-v4-flash`/`deepseek-v4-pro`
**压根不发 `thinking` 字段**、直接吃服务端默认。所以「给 side query 关掉 thinking」这条路
根本不成立 —— 除非显式发 `thinking:{"type":"disabled"}`，而那等于废掉 V4 的推理能力。

**这个限制官方没文档化。** DeepSeek 的 thinking 模式指南只写「thinking 模式支持工具调用」，
对 `tool_choice` 只字未提。LangChain、pydantic-ai、opencode、claude-code-router 都独立提过
同一个 400，属于生态级的坑。

**另一半根因：我用了仓库里没人用的机制。** 全仓仅有的两个 `SideQueryRequest` 调用方
(`tools/web/src/web_fetch.rs:454`、`memory/src/selector.rs:64`) **都是 `tools: vec![]` +
`tool_choice: None`**，靠 `sidequery::decode_response` 从回复**文本**里 `serde_json::from_str`
拿结构。强制工具调用那套来自 **subagent** 路径（`agent/src/api.rs`、`agent/src/runner.rs:834`），
是 Anthropic 形状的。两种机制各有取舍（文本解析可移植但无强制力；强制工具有 wire 级保证但不可移植），
选强制工具本身可以辩护 —— 但当时代码注释宣称「这就是仓库既有的 side-query 机制，没有发明新东西」，
**那句话是假的**，已在 `local_apps_llm.rs` 更正。

`llm-client/src/config.rs:210` 的 `Capabilities` 带着 `reasoning` / `structured_output` 两个位，
`provider_settings.rs:488/500` 不同 provider 的值确实不同，`protocol.rs:875` 还有现成的门控先例。
`local_apps_llm.rs` 里 `Capabilities` 出现 4 次 —— **全在注释里，一次没真查过**。

## 修法

1. 调用前查目标 profile 的 `Capabilities`。
2. `structured_output` 支持 → 用 `ResponseFormat::JsonSchema`（`protocol.rs:663`）。
   对 DeepSeek 实测可用（见矩阵最后一行）。
3. 仅在支持强制工具**且**当前推理设置不冲突时才用 `ToolChoice`。
4. 都不可用时明确报**能力不匹配**，不要报 `llm unavailable`。

⚠️ 组合维度是 provider × 模型 × 推理开关的**三元组**，不是一个布尔位。实测已证明这点：
同一个 provider、同一个 model id，只因 `thinking` 开关不同，`tool_choice` 就一个 400 一个 200。
**所以 `provider_settings.rs` 里放一个静态的「DeepSeek 支持强制工具」布尔位一定是错的** ——
初版文档说「这个 bug 可能有一半在数据里」，实测结论是：不在数据里，静态数据表达不了这个三元组。

## 当前的缓解措施（`254caa0a9`）

先发强制，若错误同时含 `tool_choice` 和 `400` 则告警并**原样重试一次、不带指令**。
`extract_single_tool_call` 仍要求「恰好一次且名字匹配」，所以降级后若模型改用散文会响亮地失败成
`LlmOutputRejected`，不会静默返回垃圾。

代价：受影响 provider 上每次结构化调用多一次被拒的往返（400 在生成前返回，不烧 token），
一个应用 3 次 = 3 次浪费。可接受，且能力门控落地后即可移除。

## 顺带

**错误分类偏了。** 400「你的请求形状我不支持」被报成 `LlmUnavailable`（模型够不到），用户会去查
网络，但重试多少次都一样。这类应归入「请求构造与能力不匹配」，与 `LlmOutputRejected`（模型答了但
答得不合格）也不是一回事。

## 为什么 20 轮评审没抓到

T8 特意设计 `LocalAppsModel` trait 让三段逻辑能用**假模型**单测 —— 设计本身是对的，代价是**真实请求
形状从没打到过真实 provider**。评审明确指出过「`ApiServiceModel` 零测试」，当时判断残余风险低，理由
是参数顺序已人工核对。判断错了：风险不在参数顺序，在**语义兼容性**，而那只有真 provider 能暴露。

**补记（08-08）：连修复本身也差点重蹈覆辙。** 第一版 `Tool{name}` → `Required` 的改动，是从
错误文案推断出来的（以为它拒的是「具名」这种写法），发到真机上照样 400。真正定案靠的是拿
`DEEPSEEK_API_KEY` 直接打真 API 跑完整矩阵 —— 环境里一直有这个 key。
**能打真接口的时候不要靠读错误信息猜。**
