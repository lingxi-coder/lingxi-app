# Vision Delegation v1：批量图片 Sidecar

**日期**：2026-08-21

**状态**：实施中
**范围**：图片；PDF/Document 延后到 Phase 2

## 1. 目标

用户选择的主模型始终负责最终回答、工具选择和工具循环。视觉委派只在主模型不能读取图片时提供感知证据：

- 主模型原生支持视觉：历史和媒体原样发送，不产生 side query。
- 主模型不支持视觉：把当前问题、有限相关上下文和当轮图片批量交给同 provider profile 的视觉 delegate。
- delegate 的结构化结果作为内部 `MediaAnalysis` meta 消息写回历史；主请求只看到分析和图片引用，不收到原始图片。
- 同一用户问题的后续工具循环只分析新出现且尚未覆盖的工具图片。
- local app 的 `chat` 和 `stream` 使用相同的两阶段 prepare；最终响应始终来自用户选择的主模型。

本设计解决非视觉模型静默丢图的问题，同时限制委派得到的权限、上下文、成本和缓存作用域。

## 2. 非目标与约束

- v1 只支持 JPEG、PNG、GIF 和 WebP 图片。
- 非原生支持的 PDF/Document 返回明确错误，不做隐藏降级。
- 不跨 provider profile 委派，不新增凭据、网络配置或外部依赖。
- delegate 不获得主系统提示、工具 schema、reasoning 或工具执行权限。
- `deepseek-v4-flash-vision-exp` 同时是可供用户选择的 beta 视觉模型和 DeepSeek profile 的内部 delegate；它进入 curated `/model` picker。
- 视觉模型的价格和能力来自当前 vendored provider catalog；缺失价格仍使用现有 unknown-model tier 并标记 `unpriced_models`，不把缺失值写成 `$0`。

## 3. 路由与公开数据模型

### 3.1 Provider 路由

`ProviderProfile` 增加可选 `vision_delegate`，JSON key 为 `visionDelegate`。registry 构建时验证：

- delegate 是同 profile 已声明模型；
- delegate 声明 `vision=true`；
- 解析不会跨 profile 或形成无意义的自委派。

调用方通过统一的 `MediaRoute { main, vision_delegate }` 和 `resolve_media_route` 获取结果，不自行猜测 profile、凭据或协议。

DeepSeek preset 把 `visionDelegate` 指向 `deepseek-v4-flash-vision-exp`。该 catalog 模型声明 `text+image`、`documents=false`，并按 provider catalog 的实际能力与价格展示 beta 状态；用户也可以直接选择该模型，原生视觉请求不会再次触发 delegate。

### 3.2 持久化分析块

`protocol::ContentBlock` 增加内部块：

```rust
MediaAnalysis { analysis: MediaAnalysis }
```

`MediaAnalysis` 固定包含：

- `question_key`
- `media_fingerprints`
- `model`
- `prompt_version`
- `created_at`
- `task_findings`
- `media`
- `cross_media_findings`
- `truncated`

每个 `MediaObservation` 包含 `fingerprint`、`label`、`description`、`ocr`、`relevant_facts` 和 `uncertainty`。

该块：

- 作为 `is_meta=true` 的用户消息写入 JSONL；
- 计入 message size；
- compaction 保留其文本证据；
- client adapter 不渲染，不增加客户端消息 DTO。

另增加 `QuerySource::VisionDelegation` 和可操作错误 `LlmError::MediaDelegationUnavailable { message }`。

## 4. 媒体收集、上限和改写

视觉收集与最终 provider 请求必须基于同一个“最新 100 个媒体”集合，防止出现已被替换成分析引用、但实际没有分析的图片。

收集范围：

- 顶层 `ContentBlock::Image`，包括 base64 和 URL；
- `ToolResult.content_blocks` 内的 `image` / `image_url`；
- v1 遇到非原生支持的 `Document` 时直接返回 Phase 2 错误。

fingerprint 规则：

- base64：对原始解码字节做 SHA-256；
- URL：对 `"url:" + url` 做 SHA-256。

非视觉主请求的改写规则：

- 顶层图片替换为 `[Image <short-fingerprint>; see media analysis]` 文本块；
- 工具结果里的图片替换为同语义的 JSON text block；
- session history 保留原始媒体，改写只作用于发往 provider 的 snapshot。

单个 side query 最多 20 张图片、24 MiB 已解码 base64。更大集合按原顺序分批，最多并发两批；单图超过 24 MiB 明确失败。URL 不在本地下载，其字节数不计入 base64 上限。

## 5. VisionDelegationService

`sidequery` 提供共享 `VisionDelegationService`，输入 `VisionPacket`，输出 `MediaAnalysis`、usage、elapsed、retry count 和实际 API call 数。

### 5.1 question key 与当轮媒体

- 主会话：最近一条非 meta 用户消息的 `MessageId`。
- local app：最近一条用户消息的规范化文本与有序媒体 fingerprints 的 SHA-256；缓存键额外包含 `app_id`、delegate model 和 prompt version。

“当轮新媒体”指最近真实用户消息里的图片，以及该消息之后工具结果产生的图片。历史图片已有同 delegate 和 prompt version 的分析时可复用；当轮缺失媒体做增量补分析。

### 5.2 有界上下文

delegate 只接收：

- 当前真实用户文本，最多 8 KiB；
- 此前最近两条非 meta user/assistant 文本，总计最多 8 KiB；
- 工具图片对应的工具名和最多 2 KiB 的模型可见结果摘要；
- 当前批次图片。

超限文本按 UTF-8 安全的 head+tail 方式截断。不得发送主系统提示、工具 schema、reasoning、旧原始媒体或其它完整历史。

### 5.3 固定请求与降级

请求固定使用同 profile 的 delegate model、空工具列表、`thinking=None`、`temperature=0`、`max_tokens=8192` 和 `QuerySource::VisionDelegation`。

`PROMPT_VERSION = 1` 的提示要求 JSON 返回逐媒体 description/OCR/relevant facts/uncertainty，以及 task/cross-media findings。结构化 JSON 解析失败但存在非空文本时，降级为单一 summary；空响应视为失败。

## 6. 主循环接入和一致性

streaming 与 non-streaming 主循环共用 pre-call prepare seam，并在所有重试、PTL 截断、compaction 恢复和 streaming fallback 中重新应用 request-only media rewrite。

持久化协议：

1. 锁内读取当前 `question_key`、有序媒体 fingerprints 和已有分析。
2. 释放锁后执行网络调用。
3. 完成后重新加锁，确认问题和媒体集合仍有效。
4. 仅在校验成功时一次性追加完整 `MediaAnalysis` meta 消息并写 JSONL。

取消、问题变化或媒体变化时丢弃结果，不写入部分分析。没有新图片时 side query 次数必须为零。原生视觉主模型的 side query 次数始终为零。

delegate usage 与实际批次数记录到同一 `CostTracker`，模型行使用 delegate model，telemetry 的 query source 为 `vision_delegation`。交互界面在调用期间显示可清除的“正在用 `<model>` 分析 N 张图片”状态；成功、错误和取消都清除。

## 7. Local app

`ApiServiceModel::chat` 和 `stream` 共用同一 prepare helper：

1. 解析 `MediaRoute`；
2. 原生视觉模型直接调用；
3. 非视觉模型先执行 delegate 或复用缓存；
4. 改写 app 请求并注入 `MediaAnalysis`；
5. 调用原选定主模型产生最终 chat/stream 响应。

`ChatRequest` 携带内部 `cache_scope=app_id`。进程内缓存使用 `HashMap + VecDeque`，键包含 `app_id + question_key + ordered fingerprints + delegate model + prompt version`，容量 128，FIFO 淘汰。不同 app 不共享；同 app 同问题命中；问题变化重新识别；进程重启后允许重新分析。

## 8. 设置与错误

- 桌面设置：`visionDelegationEnabled: Option<bool>`，消费端 `unwrap_or(true)`。
- 移动端：`MobileConfig.vision_delegation_enabled: bool`，iOS/Android launch FFI records 和设置页默认 `true`。
- 关闭开关、无 delegate、配置无效、图片过大、delegate API 失败、空输出分别产生可操作错误；底层 provider 错误经过现有安全格式化。
- 未被委派层处理的非视觉媒体请求仍由 llm-client 返回明确 capability error，绝不静默删除。

## 9. 验证要求

最低覆盖：

- 协议 round-trip、旧 JSONL 兼容和 message-size；
- delegate 缺失/跨 profile/非视觉目标校验；
- 顶层 base64/URL、嵌套工具图片收集、fingerprint 和改写；
- 20 张/24 MiB 分批、并发 2、100 媒体 cap 和 UTF-8 截断；
- JSON、文本 fallback、空响应、超限和取消；
- 主循环一次委派、工具图片增量委派、resume 复用和 stale-result 丢弃；
- local app chat/stream 两阶段调用和 128 条 app-scoped FIFO；
- delegate 成本、unknown pricing 警告和默认开/显式关设置；
- iOS/Android launch record 与绑定 round-trip。

最终门禁：

```text
cd lingxi-code && cargo fmt --all --check
cd lingxi-code && cargo test --workspace --all-features --no-fail-fast
cd lingxi-code && cargo clippy --workspace --all-targets --all-features -- -D warnings
cd clients/android && ./gradlew :app:assembleDebug :app:testDebugUnitTest
cd clients/ios && xcodegen generate && xcodebuild -project LingxiCode.xcodeproj -scheme LingxiCode -sdk iphonesimulator -destination 'platform=iOS Simulator,name=iPhone 15 Pro' test
```

## 10. Phase 2：PDF

PDF 委派不在 v1 暗中降级。Phase 2 先抽出公开、byte-oriented 的 renderer，再评估桌面和移动端的统一实现、页数/字节上限、缓存 fingerprint 和成本。原生 document-capable 主模型在 v1 仍原样接收 Document。
