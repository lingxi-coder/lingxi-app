# 内联可视化集成

发布不可变可视化 revision，并在 Electron、Android 与 iOS 中经同一授权服务挂载。

<a id="host-opt-in"></a>

## 能力与宿主显式启用

桌面／移动组合仅在宿主 WebView 能呈现 widget 时通过 inline_visualization 显式启用。该 flag 同时注册 Visualization 与 visualize skill。CLI 和 headless 关闭它，两种能力均不声明。浏览器式呈现界面是显式前提。

runtime 在进程中为每个 config-home 共享一个 store，让发布工具与宿主服务一致地串行化写入。宿主转发 scheme 请求与 shell 消息；共享服务负责路由、授权、文档组装、CSP 与状态比较并交换。

<a id="revision-storage"></a>

## 不可变 revision 与确认状态

存储根为 <config-home>/visualizations/<root-session-uuid>，包含 index.json、不可变 revisions/<id>/<rev>.html 和版本化 state/<id>/<rev>.json。写入使用 rooted 原子 write／sync／rename，仅成功后确认。Advisory lock 在跨进程间串行化写入。

确认的 widget 状态区分 modelContent 与 privateContent。合并状态上限 16 KiB；会话 fragment 存储上限 64 MiB，每个可视化 ID 最多 50 revision。校验／配额／存储失败返回明确错误，之后 UI 不能宣称已保存。

<a id="mount-lifecycle"></a>

## 授权挂载生命周期

1. 请求 VisualizationService 按 root-session revision、主题、语言与展开状态挂载。它返回 capability token、generation 与文档 URL。
2. 将 WebView scheme 请求转发共享服务并保留其 status／CSP header。UI 不能通过读取任意 fragment 文件绕过服务。
3. 使用活动挂载标识与预期版本转发状态写入。过期挂载或版本冲突被拒绝；冲突时采用返回的胜出状态。
4. 替换或会话切换时卸载。会话删除及孤儿 sweep 经所属 runtime 清理存储，不由 UI 任意删除文件系统。

<a id="followup-context"></a>

## 后续上下文在发送时取快照

SendPrompt 可携带 visualization_context {id, revision}。runtime 在提示发送／排队时读取该 revision 已确认 modelContent，并在输入文本前加围栏。未知 session／id／revision 保持原文本不变。Widget privateContent 不提升到模型上下文。

### ClientCommand · 继续 revision 2

```json
{
  "type": "send_prompt",
  "text": "Explain the selected option",
  "images": [],
  "visualization_context": {
    "id": "viz-example",
    "revision": 2
  }
}
```

验证方式: 接口摘录

将示例 ID 替换为实际发布返回的 ID。该字段不授权读取其他对话可视化，虚构 ID 不会贡献后续上下文。

<a id="visualization-acceptance"></a>

## 跨平台验收

- 测试覆盖 revision 存储、token／generation 授权、过期状态拒绝、配额及共享宿主行为。按实际复制提示验证主题／语言、折叠与展开挂载及后续上下文。
- Electron 自定义 scheme 和 iOS／Android WebView 消息路由、触控、无障碍及 CSP 约束仍需实际宿主 UI 验收。共享 Rust 测试不能证明三端原生 viewer 均通过。

### 共享存储与宿主契约

```bash
cargo test --locked -p visualization --lib
cargo test --locked -p harness-runtime --features desktop --test inline_visualization
cargo test --locked -p harness-runtime --features desktop,mobile --test visualization_host
```

验证方式: 需环境验证

## 源码与文档基线

[源码仓库](https://github.com/lingxi-coder/harness-runtime)

- [harness-runtime/crates/runtime/src/inline_visualization.rs](https://github.com/lingxi-coder/harness-runtime/blob/44f2eef5baf8ae16254934dbca451cbfd528d955/crates/runtime/src/inline_visualization.rs) — 已提交源码 (44f2eef5baf8ae16254934dbca451cbfd528d955)
- [harness-runtime/crates/runtime/src/desktop_visualization.rs](https://github.com/lingxi-coder/harness-runtime/blob/44f2eef5baf8ae16254934dbca451cbfd528d955/crates/runtime/src/desktop_visualization.rs) — 已提交源码 (44f2eef5baf8ae16254934dbca451cbfd528d955)
- [harness-runtime/crates/runtime/src/mobile/visualization_host.rs](https://github.com/lingxi-coder/harness-runtime/blob/44f2eef5baf8ae16254934dbca451cbfd528d955/crates/runtime/src/mobile/visualization_host.rs) — 已提交源码 (44f2eef5baf8ae16254934dbca451cbfd528d955)
- [harness-runtime/crates/tools/ui/src/inline_visualization.rs](https://github.com/lingxi-coder/harness-runtime/blob/44f2eef5baf8ae16254934dbca451cbfd528d955/crates/tools/ui/src/inline_visualization.rs) — 已提交源码 (44f2eef5baf8ae16254934dbca451cbfd528d955)
- [harness-runtime/crates/visualization/src/store.rs](https://github.com/lingxi-coder/harness-runtime/blob/44f2eef5baf8ae16254934dbca451cbfd528d955/crates/visualization/src/store.rs) — 已提交源码 (44f2eef5baf8ae16254934dbca451cbfd528d955)
- [harness-runtime/crates/visualization/src/service.rs](https://github.com/lingxi-coder/harness-runtime/blob/44f2eef5baf8ae16254934dbca451cbfd528d955/crates/visualization/src/service.rs) — 已提交源码 (44f2eef5baf8ae16254934dbca451cbfd528d955)
- [harness-runtime/crates/visualization/src/document.rs](https://github.com/lingxi-coder/harness-runtime/blob/44f2eef5baf8ae16254934dbca451cbfd528d955/crates/visualization/src/document.rs) — 已提交源码 (44f2eef5baf8ae16254934dbca451cbfd528d955)
- [harness-runtime/crates/visualization/src/context.rs](https://github.com/lingxi-coder/harness-runtime/blob/44f2eef5baf8ae16254934dbca451cbfd528d955/crates/visualization/src/context.rs) — 已提交源码 (44f2eef5baf8ae16254934dbca451cbfd528d955)
- [harness-runtime/crates/visualization/src/checks.rs](https://github.com/lingxi-coder/harness-runtime/blob/44f2eef5baf8ae16254934dbca451cbfd528d955/crates/visualization/src/checks.rs) — 已提交源码 (44f2eef5baf8ae16254934dbca451cbfd528d955)
- [harness-runtime/crates/runtime/tests/inline_visualization.rs](https://github.com/lingxi-coder/harness-runtime/blob/44f2eef5baf8ae16254934dbca451cbfd528d955/crates/runtime/tests/inline_visualization.rs) — 已提交源码 (44f2eef5baf8ae16254934dbca451cbfd528d955)
- [harness-runtime/crates/runtime/tests/mobile/visualization_host.rs](https://github.com/lingxi-coder/harness-runtime/blob/44f2eef5baf8ae16254934dbca451cbfd528d955/crates/runtime/tests/mobile/visualization_host.rs) — 已提交源码 (44f2eef5baf8ae16254934dbca451cbfd528d955)

- harness-runtime: HEAD 44f2eef5baf8ae16254934dbca451cbfd528d955; 所据文件指纹 sha256:25269e99665402275451ca5949e4177e613143620f1cc8cabdbf422dcd7ab8a2; 产品固定版本 44f2eef5baf8ae16254934dbca451cbfd528d955

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
