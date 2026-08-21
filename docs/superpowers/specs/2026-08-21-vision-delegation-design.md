# 视觉委派（Vision Delegation）设计

**日期**：2026-08-21
**状态**：设计已确认，待实现计划
**问题**：用户选中无视觉能力的模型（如 DeepSeek 的 `deepseek-v4-pro` / `deepseek-v4-flash`）时，任何带图片或 PDF 的请求都会硬失败。

---

## 1. 问题陈述

### 1.1 今天的行为

`llm-client/src/protocol.rs:853` 的 `validate_capabilities` 在任何网络 I/O 之前遍历请求的 content block，遇到模型不支持的媒体就返回错误：

```rust
ContentBlock::Image { .. } | ContentBlock::ImageUrl { .. } if !capabilities.vision => {
    return Err(LlmError::UnsupportedCapability { capability: "vision".to_string() });
}
ContentBlock::Document { .. } if !capabilities.documents => {
    return Err(LlmError::UnsupportedCapability { capability: "documents".to_string() });
}
```

调用点在 `llm-client/src/client.rs:366` 和 `:787`（均在 `DefaultLlmClient` 内）。

`Capabilities.vision` / `.documents` 由 models.dev 快照的 `modalities.input` 推导（`llm-client/src/catalog/map.rs:33`）。`llm-client/data/models-dev/deepseek.json` 里四个模型（`deepseek-chat`、`deepseek-reasoner`、`deepseek-v4-flash`、`deepseek-v4-pro`）的 `modalities.input` 均为 `["text"]`，因此 DeepSeek 下任何媒体请求必然命中上面的分支。

### 1.2 可用的委派目标

DeepSeek 已发布 `deepseek-v4-flash-vision-exp`（DeepSeek-V4-Flash-Vision-Exp），运行在**同一个 API 平台**上：

- base URL：`https://api.deepseek.com`（与现有 preset 一致，`presets.rs:64`）
- 凭据：`DEEPSEEK_API_KEY`（与现有 preset 一致，`presets.rs:70`）
- 协议：OpenAI 兼容 Chat Completions
- 支持格式：**JPEG / PNG / GIF / WebP**。**不支持 PDF。**

因为路由与凭据完全复用，委派不需要引入第二套鉴权或网络配置。

> 实现前须核对官方文档确认最终 model id 与定价，不得依据搜索结果填写 `cost` 块。
> 来源：<https://api-docs.deepseek.com/guides/vision/>

### 1.3 三条汇流线

已核实，三个入口最终都经过同一个咽喉点：

```
主循环 ─┐
工具结果 ─┼→ ConversationMessage 历史 ─┐
local app llm.chat ──(无历史，app 自带)─┼→ ApiService → DefaultLlmClient → validate_capabilities ✗
side_query ───────────────────────────┘
```

- **主循环 / 工具结果**：媒体进入 `ConversationMessage` 历史并持久化到 JSONL。
- **local app `llm.chat`**：`ChatRequest.messages` 的文档注释为 *"Conversation so far, oldest first"* —— 历史由 app 自身持有并每次重发，**引擎侧无持久化会话**。`ChatPart::Image` / `ChatPart::Document` 已是 app 面向的一等公民（`apps/engine-mobile/src/local_apps_llm.rs:134`）。
- 二者都经由 `ApiService`（`ApiServiceModel` 持有 `Arc<ApiService>`，`local_apps_llm.rs:169`）。

这两条线的状态模型不同，因此存储策略必须不同（见 §4）。

---

## 2. 已确认的决策

| # | 决策 | 取值 |
|---|---|---|
| D1 | 作用范围 | **通用机制**：每个 provider 在路由表中声明可选的委派模型，DeepSeek 是第一个填表项，而非特例分支 |
| D2 | 委派原语 | **`side_query`，主循环阻塞等待**（非后台 agent） |
| D3 | 描述存储 | **挂在 `ContentBlock::Image` / `Document` 块自身**（由「旁挂兄弟块」修订而来，见 §3.1） |
| D4 | 覆盖面 | 用户消息图片 + 工具结果图片 + local app 截图 + PDF/Document |
| D5 | 失败策略 | **维持硬报错**，但错误文案必须指名真实阻碍与对应动作 |
| D6 | 可见性 | **默认开 + 可关 + 计入成本 + TUI 进度行** |
| D7 | 委派模型可见性 | `deepseek-v4-flash-vision-exp` **在 `/model` 选择器中露出** |
| D8 | 提取策略 | **任务相关**：把当前用户问题一起送给视觉模型 |
| D9 | 重识别边界 | **只有当轮新进入的媒体做任务相关识别**；历史中的老媒体沿用其上次描述 |

---

## 3. 数据模型

### 3.1 描述挂在媒体块自身

`ContentBlock::Image` / `Document` 各增加一个可选字段：

```rust
Image {
    source: ImageSource,
    /// Delegated vision description, when the selected model lacks vision and a
    /// delegate produced one. `None` on every image that never needed
    /// delegation — so existing JSONL and snapshots stay byte-identical.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    description: Option<MediaDescription>,
},
Document {
    source: DocumentSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    description: Option<MediaDescription>,
},
```

```rust
pub struct MediaDescription {
    /// 提取出的描述正文。
    text: String,
    /// 产出它的委派模型 id（证据 + 将来换 delegate 时的判据）。
    model: String,
    /// 这段描述所服务的那个用户问题的摘要键。新问题使其失效；
    /// 同一问题下的工具循环轮复用它。
    question_key: u64,
    /// 产出时刻。`protocol` crate 不依赖 chrono；沿用既有惯用法
    /// （`messages.rs:235` 的 `set_at`、`secret.rs:122` 的 `created_at`），
    /// 格式化走 `protocol/src/iso8601.rs`。
    created_at: std::time::SystemTime,
}
```

**为什么挂在块上而不是旁挂兄弟块**：兄弟 `Text` 块会与它的图**走散** —— `compaction/src/strip_media.rs:45` 把 Image 换成占位符时会留下孤儿描述；历史截断、重排同理。挂在块上使二者物理不可分离，且免去了配对用的 content-hash。

**为什么这不触及客户端**：`client-adapter/src/turn.rs:180` 的注释确认 —— *"No `MessageBlockDto::Image`/`Document` — image and document input are uniform inline wire DTOs elsewhere (decision §0.8), not scrollback blocks. Drop them... preserved through the engine/JSONL for resume/replay byte parity but render-skipped here."* `ContentBlock::Image` 从不进入客户端 DTO，只活在引擎内部与 JSONL 中。**因此无需改动 iOS / Android / Electron / shared 四端，也无需重新生成 uniffi 绑定。**

**wire 向后兼容**：`#[serde(default, skip_serializing_if = "Option::is_none")]` 是该 enum 的既有惯用法（`ToolUse.provider_id`、`ToolResult.provider_tool_use_id`）。老 JSONL 反序列化得到 `None`；`None` 不序列化，**现有 JSONL 与快照保持逐字节一致，无需 re-bless**。

**已知代价**：`ContentBlock::Image { source: ... }` 的**字面量构造点**全部需要补字段（约 12 处，多数在测试中：`protocol/src/messages.rs`、`tui/src/replay.rs`、`protocol/src/message_size.rs` 等）。模式匹配（`Image { .. }` 或具名解构）不受影响。

**连带同步点**：

- `protocol/src/message_size.rs:34` —— 描述须计入消息体积。
- `sidequery/src/provider_side_query.rs:466` —— 转换时对 `description` 的处理。
- `compaction/src/strip_media.rs:45` —— 换占位符时**保留描述正文**（顺带修复 compaction 现有的信息损失）。

---

## 4. 能力路由

### 4.1 声明

两层各加一个字段：

```rust
// llm-client/src/catalog/presets.rs —— 内置 provider 的硬编码路由
struct Preset {
    // ...existing...
    /// Model id within THIS slice to delegate image understanding to when the
    /// selected model lacks vision. `None` = this provider has no vision
    /// sibling; media requests hard-error.
    vision_delegate: Option<&'static str>,
}

// llm-client/src/config.rs —— 运行时 profile
pub struct ProviderProfile {
    // ...existing...
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vision_delegate: Option<String>,
}
```

DeepSeek 的 preset 条目补上 `vision_delegate: Some("deepseek-v4-flash-vision-exp")`。

**硬约束：委派目标必须是同一 slice 内的 model id。** 这保证同 `base_url` / 同 `credential` / 同 `protocol`，委派调用完全复用主 route。

**为什么手写声明而非自动推导**：自动推导（「取 slice 中第一个 `modalities.input` 含 `image` 的模型」）在 openrouter 这类含数百个视觉模型的 slice 上是任意且可能极昂贵的选择。`presets.rs` 的模块注释已确立原则 —— *"The routing table is the source of truth for wire/auth and overrides the snapshot's advisory `api`"*。委派目标属于路由。

**副作用（非新增机制）**：字段落在 `ProviderProfile` 上，因此用户自定义的 provider profile 天然可配 `vision_delegate`。它仍限于同 profile 内的模型，不引入跨 provider 委派。

### 4.2 解析顺序

请求发出前按序判定：

1. 请求不含 `Image` / `ImageUrl` / `Document` 块 → 原样放行。**绝大多数请求走此路，零开销。**
2. 选定模型 `capabilities.vision`（或 `.documents`）为真 → 原样放行。*用户直接选 `deepseek-v4-flash-vision-exp` 作主模型时走此路，不触发委派。*
3. 委派开关关闭 → 硬报错（D5）。
4. 该 profile 的 `vision_delegate` 为 `None` → 硬报错，文案指名 provider（D5）。
5. 以上均通过 → 执行委派（§5）。

### 4.3 DeepSeek 的 catalog 改动

- `llm-client/data/models-dev/deepseek.json` 新增 `deepseek-v4-flash-vision-exp`，`modalities.input` 为 `["text", "image"]`（**不含 `pdf`**，见 §1.2）。
- `llm-client/src/catalog/presets.rs:290` 的 `assert_eq!(count("deepseek"), 4)` → `5`。
- `cost` 块须依官方定价填写（见 §1.2 注）。
- 该模型在 `/model` 选择器中正常露出（D7）。

---

## 5. 委派执行

### 5.1 注入点：一个，不是 N 个

不钩 `user_with_images` / `user_with_documents` 等构造器 —— 工具结果与 local app 都不经过它们，会漏。

正确的 seam 是**组装 `LlmRequest` 之前、持有完整会话历史的那一刻**：

```
扫描历史中所有 Image/Document 块
  ├─ 无媒体块，或全部 description 对当前问题有效 → 直接放行   ← 稳态，纯内存扫描
  └─ 存在需要识别的媒体
       ├─ 选定模型有 vision → 放行（原生送图）
       └─ 选定模型无 vision → 并发委派 → 写回 description → 落盘 → 再组装
```

判据（含 D8/D9）：一个媒体块需要委派，当且仅当

```
description.is_none()
  || (该块属于当轮新进入的媒体 && description.question_key != current_question_key)
```

第二项即 D9：**只有当轮新进入的媒体**会因问题变化而重新识别；历史中的老媒体沿用其上次描述。

**「当轮新进入的媒体」的精确定义**（消除歧义 —— 否则「最后一条用户消息里的媒体」与「自上次组装请求以来新增的媒体」会导出两种不同实现）：

> 指**最近一条 `ConversationMessage::User` 消息中携带的媒体块**，以及**自该条用户消息之后由工具结果新产生的媒体块**。

即：以最近一条用户消息为界，界之后的所有媒体都算「当轮新进入」，界之前的都算「老媒体」。这样定义使得：

- 用户发图并提问 → 该图为当轮新进入，做任务相关识别。
- agent 随后 Read 出一张截图 → 也在界之后，同样为当轮新进入，按同一问题识别。
- 用户下一次提问 → 界前移，上述两张图都变成老媒体，沿用各自描述，不重烧。

该 seam 白拿三件事：

- 用户图、工具结果图、任何其他途径进入历史的媒体一视同仁，无需逐个钩构造点。
- 用户中途从视觉模型切到 DeepSeek：老图 `description` 为 `None`，切换后首轮自动补识别；切回去原图仍在，直接原生送。
- **递归天然不成立**：委派使用 delegate 模型，其 `capabilities.vision == true`，命中 §4.2 第 2 条放行。无需额外防递归旗标。

### 5.2 委派调用

复用现成的 `SideQueryRequest`：

```rust
SideQueryRequest {
    model: profile.vision_delegate.clone(),      // "deepseek-v4-flash-vision-exp"
    profile: Some(same_profile),                 // 同 profile → 同 route / credential
    messages: vec![/* 单张媒体 + 任务相关提取指令 */],
    query_source: QuerySource::MediaDescription, // 新增变体
    thinking: None,                              // 对齐既有 utility side query
    tools: vec![],
    tool_choice: None,
    // ...
}
```

使用 `ProviderSideQueryClient::from_service`。按其模块文档，该构造复用父 `ApiService` —— *"including its exact provider route, OAuth/keychain credential, message normalization, prompt-cache boundaries, headers, and retry behavior"*。**委派不需要任何新的鉴权或路由代码。**

`sidequery/src/purposes.rs` 的 `QuerySource` 新增 `MediaDescription` 变体。先例：`WebFetchApply`（`tools/web/src/web_fetch.rs:469`）—— 同样是一次路由到另一模型的次级调用，带独立 COGS 标签。`/cost` 的归集因此是白拿的。

多个媒体**并发**委派：主循环多等 1 轮，而非 N 轮。

### 5.3 提取指令（D8）

指令须同时包含两半：

1. **任务相关**：围绕当前用户问题从媒体中提取信息。
2. **全量兜底**：无论问题为何，OCR 出所有可见文字。

兜底的作用是防止问题问偏导致描述完全无用。

**`question_key` 的必要性**：在 agent 循环中，「轮」与「问题」不是一回事 —— 用户问一句，agent 可能连跑 8 轮工具调用，这 8 轮的用户问题是同一个。若按字面「每轮重识别」，同一媒体会为同一问题识别 8 次。以 `question_key` 为键后：

- 成本形状从 `O(媒体 × 轮数)` 降为 `O(媒体 × 用户提问数)`；
- 叠加 D9（老媒体不重识别）后，进一步接近 `O(媒体总数)`；
- 任务相关性完整保留：每个新问题都会为当轮新媒体拿到定制描述。

**天然成本闸门**：compaction 后 `strip_media` 将媒体替换为占位符，媒体消失即不再委派。长会话自行衰减。

### 5.4 送出时的替换

组装 `LlmRequest` 时（`engine/src/prompt.rs:81` 的转换点），`Image { source, description: Some(d) }` 降为文本块，**原始媒体不进请求**。历史中的原图不动。

### 5.5 PDF 链路

DeepSeek 视觉模型不接受 PDF（§1.2），故 Document 多一跳：

```
Document(pdf) → pdf_render::render_pdf_pages (pdftoppm) → 每页 JPEG → 并发委派 → 合并为分页描述
```

`tools/file/src/pdf_render.rs` 是现成的，错误分类（`Empty` / `TooLarge` / `Unavailable` / `PasswordProtected` / `Corrupted` / `NoOutput` / `Unknown`）齐备。

**`pdftoppm` 是运行时系统二进制依赖**（poppler-utils，非 Rust crate）：

- 桌面端：通常可用；缺失时 `PdfRenderError::Unavailable`。
- **移动端 rootfs：基本一定没有。** 按 D5 硬报错，文案必须区分「缺 poppler-utils」与「模型不支持 PDF」（§6.2）。

**连带修复 `is_pdf_supported`**：`tools/file/src/pdf_read.rs:35` 现为

```rust
/// claude-code isPDFSupported: every model EXCEPT one whose id contains `claude-3-haiku`.
pub fn is_pdf_supported(model: &str) -> bool {
    !model.to_ascii_lowercase().contains("claude-3-haiku")
}
```

这是从 claude-code 直译的 Claude-only 判据，对 DeepSeek 返回 `true`，导致 Read 工具照常吐出 `ContentBlock::Document`，随后在 `validate_capabilities` 硬失败。改为按 `capabilities.documents || vision_delegate.is_some()` 判定。其测试（`pdf_read.rs:104-107`）把错误语义钉成了「正确」，须重写（§7.2）。

### 5.6 local app（无历史线）

`ChatRequest.messages` 由 app 持有并每次重发，引擎侧无可写回的历史。该线使用 `ApiServiceModel` 内的 **content-hash → 描述** 缓存，在调用 `ApiService` 前做 per-call 替换。哈希键兜住「同一截图在 app 多轮对话中被反复重发」。

进程内缓存，不落盘。引擎重启后 app 的老截图会重识别一次 —— 该线可接受：app 会话本就短命，且落盘需为一个引擎并不拥有的会话发明存储。

---

## 6. 失败与可见性

### 6.1 新增 `LlmError` 变体

不修改 `UnsupportedCapability { capability: String }`：它只有一个字段，`llm-client/tests/protocol_test.rs` 中有三处 `if capability == "vision"` 式具名解构会被打断；且 `llm-client/src/service.rs:1893` 将**变体本身**映射为错误码 `"unsupported_capability"`，往 `capability` 串塞引导文案会污染该码。

跟随邻居 `CostUnavailable { message }` 的形状新增变体：

```rust
/// Media was present but could not be understood: the model lacks vision and
/// delegation was unavailable or failed.
#[error("{message}")]
MediaDelegationUnavailable {
    /// User-facing explanation naming the actual blocker and the fix.
    message: String,
},
```

纯增量；现有 8 个 `UnsupportedCapability` 构造点与其测试不变。`service.rs` 增加一条映射臂给新错误码。

### 6.2 四种失败，四种文案（D5）

现状下四种情况说的是同一句 `unsupported capability: vision`，用户无从下手。每种须指名真实阻碍与对应动作：

| 阻碍 | 文案要点 |
|---|---|
| provider 未声明 `vision_delegate` | 「provider `zai` 下没有可用于图像识别的模型。请换用支持视觉的模型。」 |
| 开关被关闭 | 「图像委派已在 settings 中关闭。当前模型 `deepseek-v4-pro` 无法识别图像。」 |
| 委派调用失败 | 「用 `deepseek-v4-flash-vision-exp` 识别图像失败：\<底层错误\>」—— 底层错误须透出，不得吞掉。 |
| PDF 缺 `pdftoppm` | 「无法处理 PDF：本机缺少 poppler-utils（`pdftoppm`）。」**不得表述为「模型不支持 PDF」** —— 两者对应的用户动作完全不同，移动端尤易误导。 |

### 6.3 开关（D6）

落在 `engine/src/settings/schema.rs` 的 `SettingsJson`，一个 `#[serde(default)]` bool，**默认开**。默认开意味着 `Default` 实现须显式给 `true`，不能依赖 `bool::default()`。

### 6.4 可见性（D6）

- **成本**：`QuerySource::MediaDescription` 打标 → `/cost` 归集。
- **进度**：主循环阻塞等待期间，TUI 显示「正在用 `deepseek-v4-flash-vision-exp` 识别 N 张图片」。缺此行时用户只会觉得发图变卡而不知原因。

---

## 7. 测试策略

### 7.1 构建门必须带 `--all-features`

**`cargo build --workspace` 对本改动是瞎的。** `apps/engine-mobile` 的 local-apps 模块全部在 `#[cfg(feature = "uniffi")]` 之后，而 local app 是三条线之一。只跑 `--workspace` 会使 `ApiServiceModel` 那条线的改动**根本未被编译**，却报告全绿。

### 7.2 三个既有测试在钉 bug，须重写而非改绿

1. `llm-client/src/catalog/presets.rs:290` —— `assert_eq!(count("deepseek"), 4)` → `5`。
2. `tools/file/src/pdf_read.rs:104-107` —— 钉的是「除 `claude-3-haiku` 外都支持 PDF」这套 Claude-only 语义，该语义本身是错的。换成按 `capabilities.documents` 判定的新测试，并**补一条 DeepSeek case**（老代码下返回 `true`，即 bug 现场）。
3. `compaction/src/strip_media.rs` 的占位符测试 —— 现会丢弃描述，改为保留。

### 7.3 「诚实性」测试（防止看着通过但未生效）

- **零委派断言**：主模型有 vision + 请求带图 → 断言 side query 调用次数 **`== 0`**。仅断言「结果正确」会让一个每次都委派的实现也变绿。
- **不递归**：委派调用自身经过 `ApiService`，断言其未再触发一次委派。
- **JSONL 逐字节不变**：`description: None` 的 Image 块序列化结果须与改动前**完全一致**（`skip_serializing_if` 生效的证明，也是无需 re-bless 快照的前提）。`Some(..)` 的块须能 round-trip。
- **老 JSONL 可读**：取改动前的 fixture（无 `description` 字段）反序列化，断言得到 `None` 而非报错。
- **`question_key` 语义**：同一问题下的多轮工具循环 → 断言仅委派 1 次；换新问题 → 断言重新委派；**老媒体在新问题下不重识别**（D9，单独钉住）。
- **一条 brief 未暗示的破坏用例**：把 `vision_delegate` 配为**不存在的 model id**，断言得到干净的 `MediaDelegationUnavailable`，而非 panic 或 502 透传。

### 7.4 覆盖矩阵

三条线 × 两种媒体，每格均需覆盖：

| | 图片 | PDF / Document |
|---|---|---|
| 主循环（用户发送） | ✓ | ✓ |
| 工具结果（Read 等） | ✓ | ✓ |
| local app `llm.chat` | ✓ | ✓ |

local app 两格**仅在开启 `uniffi` feature 时可编译**，见 §7.1。

---

## 8. 改动清单

**新增**

- `MediaDescription` 结构（`protocol` crate）
- `QuerySource::MediaDescription` 变体（`sidequery/src/purposes.rs`）
- `LlmError::MediaDelegationUnavailable` 变体（`llm-client/src/error.rs`）+ `service.rs` 映射臂
- 委派模块（归属 `sidequery` crate）
- `deepseek-v4-flash-vision-exp` catalog 条目

**修改**

- `protocol/src/messages.rs` —— `ContentBlock::Image` / `Document` 加 `description` 字段；约 12 处字面量构造点
- `protocol/src/message_size.rs:34` —— 描述计入体积
- `llm-client/src/catalog/presets.rs` —— `Preset.vision_delegate` + DeepSeek 条目 + `count` 测试
- `llm-client/src/config.rs` —— `ProviderProfile.vision_delegate`
- `llm-client/src/protocol.rs:853` —— 闸门放行已委派的媒体
- `llm-client/data/models-dev/deepseek.json` —— 新模型
- `engine/src/prompt.rs:81` —— 送出时的替换
- `engine/src/settings/schema.rs` —— 开关
- `tools/file/src/pdf_read.rs:35` —— `is_pdf_supported` 改为按能力判定 + 测试重写
- `compaction/src/strip_media.rs:45` —— 占位符保留描述
- `sidequery/src/provider_side_query.rs:466` —— `description` 的转换处理
- `apps/engine-mobile/src/local_apps_llm.rs` —— `ApiServiceModel` 的 per-call 替换 + hash 缓存
- TUI —— 委派进度行

**不改动**

- iOS / Android / Electron / clients-shared 四端（`ContentBlock::Image` 从不进客户端 DTO）
- uniffi 绑定（无 wire DTO 变化）
- 现有 JSONL 与快照（`skip_serializing_if` 保证逐字节一致）
- `LlmError::UnsupportedCapability` 及其 8 个构造点

---

## 9. 未决事项

1. **`deepseek-v4-flash-vision-exp` 的确切 model id 与定价** —— 实现前须核对 <https://api-docs.deepseek.com/> 官方文档，不得依据搜索结果填写。
2. **`question_key` 的具体摘要算法** —— §5.1 已定义「当轮」的边界（以最近一条用户消息为界）；仍需在实现计划中定下对**哪些字节**取摘要：仅最近一条用户消息的文本块？是否含其中的附件引用？空文本（纯图片消息）时取什么？
3. **PDF 分页描述的合并格式** —— 多页 JPEG 各自委派后如何拼接为单段描述。
