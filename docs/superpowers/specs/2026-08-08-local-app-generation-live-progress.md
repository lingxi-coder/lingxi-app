# 本地应用生成：实时中间过程

日期：2026-08-08
起因：真机 QA。生成一个应用要几十秒到几分钟，界面只有一个 spinner 和
`Generating / generate`，用户无法判断它在干什么、还要多久、是不是卡死了。
目标：像对话界面一样把模型的中间产出实时展示出来。

## 现状

三段 LLM 调用（出题 / 出方案 / 写码）都走
`ApiService::messages_create_side_query` → `drive_non_stream`，**非流式**：
请求发出后到响应返回之间，引擎不知道任何中间状态，自然也推不出任何东西。

生成流水线只有 5 个粗粒度阶段（`local-apps/src/generation.rs`）：

| stage | percent |
|---|---|
| `scaffold` | 5 |
| `generate` | 25 |
| `validate` | 50 |
| `build` | 70 |
| `start_preview` / `awaiting_preview_approval` | 90 / 100 |

用户看到的 `Generating / generate` 就是 25% 那一档，整个写码调用期间纹丝不动。

## 关键发现：管道已经通了，缺的是内容

1. **`ApiService::stream_forced` 已经存在**（`llm-client/src/service.rs:3408`）：
   流式 + `tool_choice` 强制具名工具 + `max_tokens = None`（模型自身上限）。
   这正是 subagent 结构化输出走的路，语义与现有 `structured` 完全对应。
   ⇒ llm-client **不需要任何改动**。

2. **`AppGenerationProgress` 已经打通五层**，且
   `LocalAppsService::report_generation_progress` 的文档明确写着
   *"nothing is persisted"* —— 它只做一次长度校验和一次有序事件发射。
   Swift 侧 `LocalAppDesignerView` 已经在渲染 `progress.detail`。
   ⇒ client-protocol / uniffi 绑定 / Android **都不需要改动**。

这两点把工作量从"跨五层新功能"压到"两层内的改造"。

## 设计

### 数据流

```
ApiService::stream_forced
  → LlmEvent 流（Reasoning delta / Text delta / InputJsonDelta）
  → ApiServiceModel::structured 累积 + 通过 mpsc 发出
  → 执行器侧的排水任务（节流 + 截断）
  → LocalAppsService::report_generation_progress
  → AppEvent::GenerationProgress → ClientEvent::AppGenerationProgress
  → Swift store.generationProgress[appID]
  → 实时日志视图
```

### 各处的职责

**`ApiServiceModel::structured`（engine-mobile）**
- 用 `stream_forced` 取流；provider 拒绝 `tool_choice` 时降级到 `stream`
  （与当前非流式路径同一条自纠正逻辑，DeepSeek 实测必需）。
- 边收边累积 `InputJsonDelta` 的 partial JSON，流结束后一次性解析出
  tool call 的 input —— 校验语义与现在完全一致（仍然要求恰好一次、名字匹配）。
- 每收到一段可读文本（Reasoning / Text）就投进 sink。**sink 是同步的、
  非阻塞的**（`try_send`，满了就丢），流的推进绝不能被 UI 反压拖慢。

**sink（`Arc<dyn Fn(&str) + Send + Sync>`，可为 `None`）**
- 为 `None` 时行为与现在一致，只是走了流式取回。测试替身不受影响。

**排水任务（`local_apps_generation.rs` / `local_apps_profile.rs`）**
- 持有 `app_id`，把文本按 ~250ms 合并成一条 progress 事件。
- `detail` 只带**尾部若干字符**（受 `MAX_TEXT_VALUE_BYTES` 约束），
  界面要的是"正在发生什么"，不是全文回放。

**Swift —— 复用对话的渲染，不写第二个渲染器**

对话界面已经有全套：`ChatView` 里的
`ScrollViewReader + ScrollView + ForEach(convo.items)` 自动滚到底，
`MessageBubble` → `StructuredAIBlocks` 按 `ConversationMessageBlock`
（`.text` / `.thinking` / `.toolUse` …）分派，`AIText` 负责 markdown 与
代码块。这些都是纯视图模型，不绑定会话状态。

- 把 `ChatView` 里的滚动容器抽成共享的 `TranscriptScrollView`，
  参数是 `[ConversationRenderItem]`，行为（自动滚底、bottom 锚点）原样保留；
  `ChatView` 改为调用它，**渲染结果不变**。
- 生成页把累积的 delta 组装成一条 assistant `ConversationRenderItem.message`，
  其 `detail.blocks` 就是 `.thinking` / `.text`，交给同一个
  `MessageBubble` 渲染。
- ⇒ 两个界面共用一套滚动、气泡、markdown、代码高亮；以后 chat 侧改进
  自动同步到生成页。

**delta 的标记方式**：`stage` 在 Swift 里只被显示、从不被分支判断
（`progress.detail ?? progress.stage`，全仓仅两处），所以 delta 事件用
`stage = "llm_thinking" | "llm_text"`、`detail = 该段文本`即可自描述，
不必往 `detail` 里塞 JSON，也不必新增字段。

**store**：`generationProgress[appID]` 只存最新一条，无法累积。新增
`generationTranscript[appID]`，按 delta 的 kind 追加到当前块尾部；
终态事件（成功/失败）到达时清空。

### 生成页 = 对话页

不只是"看得见"，而是**能接着说**——这本来就是这次改造的产品目标
（一句话 brief → 问卷 → 方案 → 生成 → **持续对话不断完善**）。

生成页由两个共享组件拼成，两个都来自 chat：

```
┌────────────────────────────┐
│  TranscriptScrollView      │  ← 从 ChatView 抽出，chat 与生成页共用
│    MessageBubble(.thinking)│
│    MessageBubble(.text)    │
├────────────────────────────┤
│  Composer                  │  ← chat 的输入条，原样复用
└────────────────────────────┘
```

后端命令**已经存在**，不需要新增：
- `LocalAppsStore.requestRevision(appID:feedback:)` —— 发一条修订
- `LocalAppDetailView.showsRevisionInput(for:)` —— 何时允许输入

改动是把现有的简易 `revisionInputBar` 换成 `Composer`，并让它在生成页
也可用（生成过程中输入排队，生成结束后发出；或按 `showsRevisionInput`
的既有判断禁用——以该函数当前语义为准，不在本次改它）。

用户发出的每条修订也进入同一条 transcript（`.message` + user 角色），
于是"我说的"和"模型答的"在一个流里，和 chat 完全一致。

### 显式不做

- **不新增 ClientEvent 变体。** 现有事件够用；新增会波及 uniffi 绑定与
  Android，而 `#[non_exhaustive]` 不会传播到 Kotlin/Swift（见
  `sdd-review-lessons-2026-08-07`）。
- **不持久化中间过程。** 它是瞬时的诊断信息，不是应用状态。
- **不改校验语义。** 流式只改变"怎么拿回来"，不改变"什么算合格"。

## 风险

| 风险 | 处置 |
|---|---|
| 流式路径丢掉了 `tool_choice` 降级 | 与非流式同款 `rejects_tool_choice` 判定 + 降级到 `stream` |
| 高频事件压垮事件总线 | sink 用 `try_send` 且有界；排水侧按时间窗合并 |
| partial JSON 在流中途不可解析 | 只在流结束后解析一次，中途从不 parse |
| 截断的 `detail` 切碎多字节字符 | 按 `char` 取尾，不按 byte |

## 验收

1. 真机上新建一个应用，生成期间界面持续出现模型的推理/源码文本。
2. 生成成功的结果与改造前一致（同样的文件、同样的校验）。
3. provider 拒绝 `tool_choice` 时仍能完成（DeepSeek 真机）。
4. `cargo test --workspace --all-features --no-fail-fast` 全绿。
