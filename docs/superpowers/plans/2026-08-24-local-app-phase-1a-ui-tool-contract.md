# 本地应用 Phase 1a：两端 UI 工具契约 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让 `inspect_ui` 返回元素几何、canvas 矩形和运行时错误账本，让 `capture_ui` 支持区域裁剪，并让 engine-mobile 能读回图片——两端一致，全程不碰协议。

**Architecture:** 三块互相独立的改动。①`engine-mobile` 打开 `tool-file` 的 `image-read` feature（一行依赖 + 一条行为测试）。②两端 WebView 注入的 JS 增加字段：元素 `rect`、`canvases[]`、`documentState`、`viewport`、`runtimeErrors`（含 `console.error`），并在客户端组装时执行载荷预算。③`capture_ui` 增加可选 `rect`，序列化进既有的不透明 `AppUiRequestDto.value`，两端各自用**修正后的**裁剪数学取图。iOS 与 Android 是近似副本，每个特性都成对落地。

**Tech Stack:** Rust（engine-mobile / client-protocol 只读）、Swift（WKWebView + 注入 JS）、Kotlin（WebView + PixelCopy）、JavaScript（注入脚本）。

**Spec:** `docs/superpowers/specs/2026-08-23-local-app-interactive-verification-design.md`

## Global Constraints

- **本阶段完全不碰协议。** 不改 `client-protocol/**`、不改 `clients/shared/src/protocol.ts`、不动 contract index / goldens / bless。新数据一律走既有的不透明字段：`ResolveAppUiRequest.result_json`（`commands.rs:438`）与 `AppUiRequestDto.value`（`local_apps.rs:774`），二者都是 `Option<String>`。
- **两端必须一致。** iOS `clients/ios/Sources/LocalApps/LocalAppWebView.swift` 与 Android `clients/android/app/src/main/java/com/lingxi/code/localapps/LocalAppWebView.kt` 的注入脚本是近似副本；既有测试注释写明「the two scripts are near-copies, so **both are pinned or neither is**」。任何脚本契约改动必须同时更新两端的钉桩测试。
- **`result_json` 的 256 KiB 是硬失败，不是截断。** iOS `LocalAppWebView.swift:407-411` 在 `resultJSON.utf8.count > 256 * 1_024` 时返回 `.failure(local_apps_error_ui_invalid_result)`。本阶段新增字段必须被预算住。
- **载荷预算（spec 定值）**：`runtimeErrors` 8 条，`message` 200 字符 / `source` 120 字符，超出只留 `runtimeErrorsDropped`；`canvases` 16 条，超出只留 `canvasCount`；`elements` 仍 200 条。组装后若超 **200 KiB**，按 `elements` → `canvases` → `runtimeErrors` 逐段降级并写 `truncated: [<段名>]`。
- **裁剪长边上限 1024 像素，且绝不放大。** 既有 `snapshotWidth` 是从整个 view 的 bounds 推的（`LocalAppWebView.swift:482-488`），直接复用会把裁剪区放大。
- **跑测试必须把完整输出写进文件再 grep。** 只 grep `FAILED` 会丢掉 `failures:` 块里的测试名。Rust 用 `--no-fail-fast`；**测试总数下降即使 0 failures 也是红旗**。
- **Gradle 只在失败时打印测试数。** `BUILD SUCCESSFUL in 2s` = 任务被缓存跳过，不是跑过了。真实数量读 `clients/android/app/build/test-results/**/*.xml`，强制重跑用 `--rerun-tasks`。
- **不要跑 `cargo fmt`**（会在无关文件产生噪声 diff）。
- **不要用 `xcodebuild ... test-without-building`**：它会静默复用陈旧 test bundle。

---

### Task 1: engine-mobile 打开 `image-read`

标注管线最终要让 agent 用 `Read` 打开落盘的裁剪图。`engine-mobile` 现在是**裸依赖** `tool-file`，而 `image-read` 默认关闭（`tools/file/Cargo.toml:47-49` 注释：*"OFF by default (engine-mobile/minimal stay lean and never compile the image codecs)"*），engine-desktop 开了（`Cargo.toml:121`）。不开的话 `Read` 命中 NUL 扫描返回 `format_binary`，不是图片。

**Files:**
- Modify: `lingxi-code/apps/engine-mobile/Cargo.toml:216`
- Modify: `lingxi-code/tools/file/src/lib.rs`（新增 `IMAGE_READ_ENABLED`）
- Test: `lingxi-code/apps/engine-mobile/tests/local_app_image_read.rs`（新建）

**Interfaces:**
- Consumes: 无
- Produces: `tool_file::IMAGE_READ_ENABLED: bool`（能力探针常量）；并改变 `Read` 对图片文件的实际行为

- [ ] **Step 1: 给 `tool-file` 加一个能力探针**

`shell_test_ctx` 用的是 `make_dummy_fs()`（`tool-api/src/test_support.rs:840`），
所以「经 registry 读一个真文件」在单测里读不到真盘。改用编译期能力常量——
它同样会因为 feature 关着而变红，而且不依赖任何 harness 细节。

在 `lingxi-code/tools/file/src/lib.rs` 顶部加：

```rust
/// Whether this build of `tool-file` can decode images for `FileRead`.
///
/// A consumer that hands the model an image BY PATH (the local-app annotation
/// crop does) needs this on, or `Read` falls to the NUL scan and returns
/// `format_binary` — a silent failure where the call succeeds and the model
/// sees no picture. Exposed as a const so a downstream crate can assert its
/// own build has it, which a behavioural test cannot do through a dummy FS.
pub const IMAGE_READ_ENABLED: bool = cfg!(feature = "image-read");
```

- [ ] **Step 2: 写失败测试**

新建 `lingxi-code/apps/engine-mobile/tests/local_app_image_read.rs`：

```rust
//! Phase 1a — the annotation pipeline hands the agent a cropped JPEG BY PATH,
//! so the mobile Read tool must decode images. Without the `image-read`
//! feature it falls to the NUL scan and returns `format_binary`: the tool call
//! succeeds and the agent sees no picture.

#[test]
fn engine_mobile_builds_tool_file_with_image_read() {
    assert!(
        tool_file::IMAGE_READ_ENABLED,
        "engine-mobile depends on tool-file without the `image-read` feature, \
         so Read on an annotation .jpg returns the binary notice instead of an \
         image. Add features = [\"image-read\"] to the tool-file dependency."
    );
}
```

⚠️ 这里用 `cargo test -p engine-mobile` 的原因之一是**特性合一**：只有在
engine-mobile 自己的依赖图里编译时，这个常量才反映 engine-mobile 的请求。
不要用 `cargo test --workspace` 来验证它——那样别的 crate 打开 feature 也会让它变绿。

- [ ] **Step 3: 跑测试确认它失败**

```bash
cd lingxi-code && cargo test -p engine-mobile --all-features \
  --test local_app_image_read --no-fail-fast 2>&1 | tee /tmp/t1.log
```
Expected: FAIL，断言消息里出现「without the `image-read` feature」。

- [ ] **Step 4: 打开 feature**

`lingxi-code/apps/engine-mobile/Cargo.toml:216`，把

```toml
tool-file = { path = "../../tools/file" }
```

改成

```toml
tool-file = { path = "../../tools/file", features = ["image-read"] }
```

`tool-file` 已经是 engine-mobile 的普通依赖，测试里可以直接 `tool_file::` 引用，
不需要额外的 dev-dependency。

- [ ] **Step 5: 跑测试确认通过**

```bash
cd lingxi-code && cargo test -p engine-mobile --all-features \
  --test local_app_image_read --no-fail-fast 2>&1 | tee /tmp/t1.log
```
Expected: PASS。

- [ ] **Step 6: 跑全量确认没有回归**

```bash
cd lingxi-code && cargo test -p engine-mobile --all-features --no-fail-fast 2>&1 | tee /tmp/t1-all.log
grep -c "^test result" /tmp/t1-all.log   # 记下测试二进制数，后续任务对照
grep -A 20 "failures:" /tmp/t1-all.log || echo "no failures block"
```
Expected: 0 failed。**测试总数不得下降。**

- [ ] **Step 7: 提交**

```bash
git add lingxi-code/tools/file/src/lib.rs lingxi-code/apps/engine-mobile/Cargo.toml \
        lingxi-code/apps/engine-mobile/tests/local_app_image_read.rs
git commit -m "Let the mobile engine decode an image it is handed by path

The annotation flow gives the agent a cropped screenshot as a workspace
path, and engine-mobile depended on tool-file without image-read, so Read
hit the NUL scan and returned the binary notice instead of a picture -- a
silent failure where the call succeeds and the agent sees nothing. The
capability is now a const a consumer can assert about its own build, which
a behavioural test cannot do through the dummy filesystem test contexts use."
```

---

### Task 2: iOS `inspect_ui` 返回元素几何与 canvas 矩形

`getBoundingClientRect()` 已经在算了，但只留了一个 `visible: bool`（`LocalAppWebView.swift:675-684`）。没有几何，用户框出来的矩形无法映射到元素。`canvasCount` 同理要扩成带矩形的列表——宿主冒烟门要只比较画布像素，不能被外围 DOM 动画骗过。

**Files:**
- Modify: `clients/ios/Sources/LocalApps/LocalAppWebView.swift`（`snapshot()`，约 `:663-687`）
- Test: `clients/ios/Tests/LocalAppsStoreTests.swift`（新增一条，紧邻 `testUIInspectionCrossesShadowRootsAndResolvesTheNativeControl`，`:312`）

**Interfaces:**
- Consumes: 既有 `deepQuery(selector, limit)`、`candidates()`、`clean()`、`isSensitive()`
- Produces: `inspect` 的 `result_json` 新增
  - `elements[].rect: [x, y, w, h]`（CSS 像素，取整）
  - `canvases: [{ rect: [x, y, w, h] }]`（≤16 条）
  - `canvasCount: number`（保留，兼容）
  - `documentState: "loading" | "interactive" | "complete"`
  - `viewport: { width, height, offsetLeft, offsetTop, scale }`（CSS 像素）

- [ ] **Step 1: 写失败测试**

在 `clients/ios/Tests/LocalAppsStoreTests.swift` 里新增：

```swift
/// Phase 1a — a user-drawn rectangle can only be mapped onto elements if the
/// snapshot carries geometry. `getBoundingClientRect()` was already being
/// computed and then discarded down to a `visible` boolean.
///
/// The Android twin is `LocalAppWebViewTest.ui inspection reports element and
/// canvas geometry`; the two scripts are near-copies, so both are pinned or
/// neither is.
func testUIInspectionReportsElementAndCanvasGeometry() {
    let source = LocalAppWebViewController.executionSource(requestJSON: "{}")
    for token in [
        // Element rect, integer CSS pixels.
        "rect: [Math.round(rect.left), Math.round(rect.top), Math.round(rect.width), Math.round(rect.height)]",
        // Canvas rects, bounded at 16 — the host gate compares only these pixels.
        "canvases: deepQuery('canvas', 16).map",
        // The legacy count stays for compatibility.
        "canvasCount:",
        // Readiness and the coordinate frame the rect is expressed in.
        "documentState: document.readyState",
        "offsetLeft: Math.round(vv.offsetLeft)",
        "scale: vv.scale",
    ] {
        XCTAssertTrue(source.contains(token), "missing geometry contract: \(token)")
    }
    // Geometry is ADDED to the element shape, not substituted for `visible`:
    // a reader that only asks "is it on screen" must keep working.
    XCTAssertTrue(
        source.contains("visible: rect.width > 0 && rect.height > 0,"),
        "`visible` must survive alongside the new rect, not be replaced by it"
    )
}
```

- [ ] **Step 2: 跑测试确认它失败**

```bash
cd clients/ios && xcodebuild test \
  -scheme LingxiCode \
  -destination "platform=iOS Simulator,name=iPhone 16,OS=latest" \
  -only-testing:LingxiCodeTests/LocalAppsStoreTests/testUIInspectionReportsElementAndCanvasGeometry \
  2>&1 | tee /tmp/t2.log
```
Expected: FAIL，`missing geometry contract:` 开头的断言。
（模拟器名以 `xcrun simctl list devices available` 实际可用的为准。）

- [ ] **Step 3: 改注入 JS**

`LocalAppWebView.swift` 的 `snapshot()`，把整个函数体换成：

```javascript
          const vvOf = () => {
            const vv = window.visualViewport;
            return vv
              ? { width: Math.round(vv.width), height: Math.round(vv.height),
                  offsetLeft: Math.round(vv.offsetLeft), offsetTop: Math.round(vv.offsetTop),
                  scale: vv.scale }
              : { width: Math.round(window.innerWidth), height: Math.round(window.innerHeight),
                  offsetLeft: 0, offsetTop: 0, scale: 1 };
          };
          const rectOf = element => {
            const rect = element.getBoundingClientRect();
            return [Math.round(rect.left), Math.round(rect.top), Math.round(rect.width), Math.round(rect.height)];
          };
          const snapshot = () => ({
            title: clean(document.title),
            url: location.href,
            documentState: document.readyState,
            viewport: vvOf(),
            // The host gate compares ONLY the pixels inside these rects, so a
            // DOM spinner cannot stand in for a frozen canvas. `canvasCount`
            // stays for compatibility with readers that only counted.
            canvases: deepQuery('canvas', 16).map(c => ({ rect: rectOf(c) })),
            canvasCount: deepQuery('canvas', 64).length,
            elements: candidates().slice(0, 200).map(element => {
              const rect = element.getBoundingClientRect();
              const sensitive = isSensitive(element);
              return {
                elementId: clean(element.id) || null,
                role: roleOf(element) || null,
                name: nameOf(element) || null,
                value: sensitive ? null : clean(element.value),
                checked: typeof element.checked === 'boolean' ? element.checked : null,
                disabled: !!element.disabled,
                visible: rect.width > 0 && rect.height > 0,
                rect: [Math.round(rect.left), Math.round(rect.top), Math.round(rect.width), Math.round(rect.height)],
              };
            }),
          });
```

- [ ] **Step 4: 跑测试确认通过**

同 Step 2 的命令。Expected: PASS。

- [ ] **Step 5: 跑 LocalApps 全套确认没有回归**

```bash
cd clients/ios && xcodebuild test -scheme LingxiCode \
  -destination "platform=iOS Simulator,name=iPhone 16,OS=latest" \
  -only-testing:LingxiCodeTests/LocalAppsStoreTests 2>&1 | tee /tmp/t2-all.log
grep -E "Test Suite .* (passed|failed)" /tmp/t2-all.log | tail -3
```
Expected: 0 failures。

- [ ] **Step 6: 提交**

```bash
git add clients/ios/Sources/LocalApps/LocalAppWebView.swift clients/ios/Tests/LocalAppsStoreTests.swift
git commit -m "Report the element geometry inspect_ui was already computing

The snapshot called getBoundingClientRect and then kept only a visible
boolean, so a rectangle a user draws over the app had nothing to resolve
against. Canvas rects come along for the same reason: the host gate has to
compare the drawn pixels and not be satisfied by a spinner next to them."
```

---

### Task 3: Android `inspect_ui` 返回元素几何与 canvas 矩形（Task 2 的孪生）

**Files:**
- Modify: `clients/android/app/src/main/java/com/lingxi/code/localapps/LocalAppWebView.kt`（`buildLocalAppUiExecutionScript`，`:508` 起；`snapshot` 在 `:630-646`）
- Test: `clients/android/app/src/test/java/com/lingxi/code/localapps/LocalAppWebViewTest.kt`（新增一条，紧邻 `:212` 的 `ui inspection crosses shadow roots and resolves the native control`）

**Interfaces:**
- Consumes: Kotlin 侧既有 `buildLocalAppUiExecutionScript(requestJson: String): String`
- Produces: 与 Task 2 **逐字相同**的 JSON 形状

- [ ] **Step 1: 写失败测试**

在 `LocalAppWebViewTest.kt` 新增：

```kotlin
/**
 * Phase 1a twin of the iOS `testUIInspectionReportsElementAndCanvasGeometry`.
 * The two injected scripts are near-copies, so both are pinned or neither is.
 */
@Test
fun `ui inspection reports element and canvas geometry`() {
    val inspectScript = buildLocalAppUiExecutionScript("""{"action":"inspect"}""")
    listOf(
        "rect: [Math.round(rect.left), Math.round(rect.top), Math.round(rect.width), Math.round(rect.height)]",
        "canvases: deepQuery('canvas', 16).map",
        "canvasCount:",
        "documentState: document.readyState",
        "offsetLeft: Math.round(vv.offsetLeft)",
        "scale: vv.scale",
    ).forEach { token ->
        assertTrue("missing geometry contract: $token", inspectScript.contains(token))
    }
}
```

- [ ] **Step 2: 跑测试确认它失败**

```bash
cd clients/android && ./gradlew :app:testPlayDebugUnitTest --rerun-tasks \
  --tests '*LocalAppWebViewTest*' 2>&1 | tee /tmp/t3.log
```
Expected: FAIL，`missing geometry contract:`。
⚠️ `BUILD SUCCESSFUL in 2s` 说明任务被缓存跳过——必须带 `--rerun-tasks`，并核对
`clients/android/app/build/test-results/**/*.xml` 里的实际数量。

- [ ] **Step 3: 改注入 JS**

把 Task 2 Step 3 的 JavaScript **逐字**搬进 `buildLocalAppUiExecutionScript` 里对应的
`snapshot` 定义处（`LocalAppWebView.kt:630-646`）。两端脚本必须字符级一致，否则钉桩
测试的 token 会在一端命中、另一端漏掉。

- [ ] **Step 4: 跑测试确认通过**

同 Step 2 的命令。Expected: PASS。

- [ ] **Step 5: 跑两个 flavor 的全量**

```bash
cd clients/android && ./gradlew :app:testPlayDebugUnitTest :app:testDirectDebugUnitTest --rerun-tasks 2>&1 | tee /tmp/t3-all.log
python3 - <<'PY'
import glob, xml.etree.ElementTree as ET
t=f=0
for p in glob.glob('clients/android/app/build/test-results/**/*.xml', recursive=True):
    r=ET.parse(p).getroot(); t+=int(r.get('tests',0)); f+=int(r.get('failures',0))+int(r.get('errors',0))
print('tests', t, 'failures', f)
PY
```
Expected: failures 0，tests 数不低于改动前。

- [ ] **Step 6: 提交**

```bash
git add clients/android/app/src/main/java/com/lingxi/code/localapps/LocalAppWebView.kt clients/android/app/src/test/java/com/lingxi/code/localapps/LocalAppWebViewTest.kt
git commit -m "Mirror the geometry contract on Android

The two injected scripts are near-copies and the existing tests say so:
both are pinned or neither is."
```

---

### Task 4: iOS 运行时错误账本（含 `console.error`）

`read_logs(log="runtime")` 只是读文件，文件不存在还返回空 tail，证明不了页面没异常。**而且只挂 `error`/`unhandledrejection` 是不够的**：两套脚手架都把树包在 React `ErrorBoundary` 里（`local-apps/templates/vite-react-static-v1/app/error-boundary.jsx` 只有 `getDerivedStateFromError`，没有 `componentDidCatch`、没有 `reportError`），React 19 的默认 `onCaughtError` 路由到 `console.error`，**永远不到 `window.onerror`**。于是渲染崩溃的 app 会显示兜底页——有 DOM、有配色文字、无未捕获异常——**全绿出厂**。

**Files:**
- Modify: `clients/ios/Sources/LocalApps/LocalAppWebView.swift`（document-start 用户脚本 `bridgeSourceTemplate`，`:1068` 起；`snapshot()` 增加 `runtimeErrors`）
- Test: `clients/ios/Tests/LocalAppsStoreTests.swift`

**Interfaces:**
- Consumes: Task 2 的 `snapshot()`
- Produces: `result_json.runtimeErrors: [{ message, source, line, column, at_ms, kind }]`，`kind ∈ {"error","rejection","console"}`；`result_json.runtimeErrorsDropped: number`

- [ ] **Step 1: 写失败测试**

```swift
/// Phase 1a — criterion 6 is structurally blind without `console.error`.
/// Both scaffolds wrap the tree in a React ErrorBoundary whose only hook is
/// `getDerivedStateFromError`; React 19 routes a caught render error to
/// `console.error` and never to `window.onerror`. A crashed app then renders
/// its fallback and passes "has elements", "frame is not uniform" and "no
/// uncaught exception" all at once.
func testDocumentStartInstallsABoundedRuntimeErrorLedger() {
    let source = LocalAppWebViewController.bridgeSourceTemplate
    for token in [
        "addEventListener('error'",
        "addEventListener('unhandledrejection'",
        // The one that actually catches a React ErrorBoundary.
        "console.error = function",
        "kind: 'console'",
        // Bounded on both axes, and cleared per document.
        // The bound itself, spelled the way the code spells it.
        "const cap = 8",
        "__lingxiRuntimeErrors.length >= cap",
        "__lingxiRuntimeErrorsDropped",
    ] {
        XCTAssertTrue(source.contains(token), "missing runtime-error ledger: \(token)")
    }
    let snapshotSource = LocalAppWebViewController.executionSource(requestJSON: "{}")
    XCTAssertTrue(
        snapshotSource.contains("runtimeErrors:"),
        "the ledger must surface in the inspect snapshot, not only in the page"
    )
}
```

⚠️ 若 `bridgeSourceTemplate` 当前是 `private static let`，把它改成 `internal`（与
`executionSource` 同样的理由：`:573-575` 的注释已经说明为什么这些要对测试可见）。

- [ ] **Step 2: 跑测试确认它失败**

```bash
cd clients/ios && xcodebuild test -scheme LingxiCode \
  -destination "platform=iOS Simulator,name=iPhone 16,OS=latest" \
  -only-testing:LingxiCodeTests/LocalAppsStoreTests/testDocumentStartInstallsABoundedRuntimeErrorLedger \
  2>&1 | tee /tmp/t4.log
```
Expected: FAIL。

- [ ] **Step 3: 在 document-start 脚本里装账本**

在 `bridgeSourceTemplate` 的 IIFE 开头（`if (window.lingxi?.v2) return;` 之后）插入：

```javascript
      // Criterion 6's evidence. A React ErrorBoundary swallows a render crash
      // into console.error and never reaches window.onerror, so the console
      // hook is the one that actually catches the shipped templates' failure
      // mode. Bounded on count AND per-field length: `result_json` is capped
      // at 256 KiB and FAILS rather than truncating.
      if (!window.__lingxiRuntimeErrors) {
        window.__lingxiRuntimeErrors = [];
        window.__lingxiRuntimeErrorsDropped = 0;
        const cap = 8;
        const trim = (v, n) => String(v == null ? '' : v).replace(/\s+/g, ' ').slice(0, n);
        const push = entry => {
          if (window.__lingxiRuntimeErrors.length >= cap) { window.__lingxiRuntimeErrorsDropped += 1; return; }
          window.__lingxiRuntimeErrors.push(entry);
        };
        window.addEventListener('error', e => push({
          kind: 'error', message: trim(e.message, 200), source: trim(e.filename, 120),
          line: e.lineno | 0, column: e.colno | 0, at_ms: Date.now(),
        }));
        window.addEventListener('unhandledrejection', e => push({
          kind: 'rejection',
          // Never expand an arbitrary rejection value — take a safe string only.
          message: trim(e.reason && e.reason.message ? e.reason.message : e.reason, 200),
          source: '', line: 0, column: 0, at_ms: Date.now(),
        }));
        const nativeConsoleError = console.error.bind(console);
        console.error = function () {
          try {
            push({ kind: 'console', message: trim(Array.from(arguments).map(a => (a && a.message) ? a.message : a).join(' '), 200),
                   source: '', line: 0, column: 0, at_ms: Date.now() });
          } catch (ignored) { /* never let the ledger break the page */ }
          return nativeConsoleError.apply(console, arguments);
        };
      }
```

在 Task 2 的 `snapshot()` 里增加两个字段（放在 `viewport` 之后）：

```javascript
            runtimeErrors: (window.__lingxiRuntimeErrors || []).slice(0, 8),
            runtimeErrorsDropped: window.__lingxiRuntimeErrorsDropped || 0,
```

账本随 document 生存：document-start 脚本每次导航都重新执行，`window` 是新的，所以
reload 天然清空；同一 document 内重复 inspect 不清空（不要在 `snapshot()` 里清）。

- [ ] **Step 4: 跑测试确认通过**

同 Step 2。Expected: PASS。

- [ ] **Step 5: 提交**

```bash
git add clients/ios/Sources/LocalApps/LocalAppWebView.swift clients/ios/Tests/LocalAppsStoreTests.swift
git commit -m "Catch the console error a React boundary turns a crash into

The runtime criterion only watched error and unhandledrejection, and both
shipped scaffolds wrap the tree in an ErrorBoundary whose only hook is
getDerivedStateFromError. React 19 routes that to console.error, so a
crashed app rendered its fallback and passed every check at once."
```

---

### Task 5: Android 运行时错误账本（Task 4 的孪生）

**Files:**
- Modify: `clients/android/app/src/main/java/com/lingxi/code/localapps/LocalAppWebView.kt`（`buildLingxiV1Bootstrap`，`:1421`；以及 `buildLocalAppUiExecutionScript` 里的 `snapshot`）
- Test: `clients/android/app/src/test/java/com/lingxi/code/localapps/LocalAppWebViewTest.kt`

**Interfaces:** 与 Task 4 **逐字相同**的 JSON 形状与 JS。

- [ ] **Step 1: 写失败测试**

```kotlin
/** Phase 1a twin of iOS `testDocumentStartInstallsABoundedRuntimeErrorLedger`. */
@Test
fun `document start installs a bounded runtime error ledger`() {
    val bootstrap = buildLingxiV1Bootstrap("phone")
    listOf(
        "addEventListener('error'",
        "addEventListener('unhandledrejection'",
        "console.error = function",
        "kind: 'console'",
        "const cap = 8",
        "__lingxiRuntimeErrors.length >= cap",
        "__lingxiRuntimeErrorsDropped",
    ).forEach { token ->
        assertTrue("missing runtime-error ledger: $token", bootstrap.contains(token))
    }
    assertTrue(
        buildLocalAppUiExecutionScript("""{"action":"inspect"}""").contains("runtimeErrors:")
    )
}
```

Android 的 document-start 脚本由 `internal fun buildLingxiV1Bootstrap(formFactor: String): String`
构造（`clients/android/app/src/main/java/com/lingxi/code/localapps/LocalAppWebView.kt:1421`）——
已经是 `internal`，测试可直接调用，无需改可见性。`formFactor` 传 `"phone"` 即可；
账本代码与它无关。

- [ ] **Step 2: 跑测试确认它失败**

```bash
cd clients/android && ./gradlew :app:testPlayDebugUnitTest --rerun-tasks \
  --tests '*LocalAppWebViewTest*' 2>&1 | tee /tmp/t5.log
```
Expected: FAIL。

- [ ] **Step 3: 把 Task 4 Step 3 的 JavaScript 逐字搬过来**

包括 `snapshot()` 里那两个字段。

- [ ] **Step 4: 跑测试确认通过**

同 Step 2。Expected: PASS。

- [ ] **Step 5: 提交**

```bash
git add clients/android/app/src/main/java/com/lingxi/code/localapps/LocalAppWebView.kt clients/android/app/src/test/java/com/lingxi/code/localapps/LocalAppWebViewTest.kt
git commit -m "Mirror the runtime-error ledger on Android"
```

---

### Task 6: 载荷预算与 `truncated`（两端）

新增字段最坏约 40 KB（`runtimeErrors` 8×(200+120) 字符、`canvases` 16 条、200 个元素各 4 个整数），而 `result_json` 超 256 KiB 是**硬失败**。必须在客户端组装时预算住，否则文本密集的 Ionic 应用 + 一个在抛异常的页面会把载荷推过上限，`inspect_ui` 返回一个既不是「未挂载」也不是超时的错误——**app 抛的异常越多，判据 6 越观测不到**。

**Files:**
- Modify: `clients/ios/Sources/LocalApps/LocalAppWebView.swift`（`snapshot()` 尾部）
- Modify: `clients/android/app/src/main/java/com/lingxi/code/localapps/LocalAppWebView.kt`（同一处）
- Test: 两端各一条

**Interfaces:**
- Consumes: Task 2–5 产出的全部字段
- Produces: `result_json.truncated: string[]`（可能含 `"elements"`/`"canvases"`/`"runtimeErrors"`）

- [ ] **Step 1: 两端各写一条失败测试**

iOS（`LocalAppsStoreTests.swift`）：

```swift
/// Phase 1a — `result_json` over 256 KiB is a hard failure, not a truncation
/// (`LocalAppWebView.swift:407-411`). The ledger must not be able to silence
/// the criterion it exists to feed.
func testSnapshotDegradesInAFixedOrderAndSaysSo() {
    let source = LocalAppWebViewController.executionSource(requestJSON: "{}")
    for token in [
        "const BUDGET = 200 * 1024",
        "for (const seg of ['elements', 'canvases', 'runtimeErrors'])",
        "truncated.push(seg)",
        "truncated: []",
    ] {
        XCTAssertTrue(source.contains(token), "missing payload budget: \(token)")
    }
}
```

Android（`LocalAppWebViewTest.kt`）：

```kotlin
/** Phase 1a twin of iOS `testSnapshotDegradesInAFixedOrderAndSaysSo`. */
@Test
fun `snapshot degrades in a fixed order and says so`() {
    val script = buildLocalAppUiExecutionScript("""{"action":"inspect"}""")
    listOf(
        "const BUDGET = 200 * 1024",
        "for (const seg of ['elements', 'canvases', 'runtimeErrors'])",
        "truncated.push(seg)",
        "truncated: []",
    ).forEach { token ->
        assertTrue("missing payload budget: $token", script.contains(token))
    }
}
```

- [ ] **Step 2: 跑两端测试确认失败**

```bash
cd clients/ios && xcodebuild test -scheme LingxiCode \
  -destination "platform=iOS Simulator,name=iPhone 16,OS=latest" \
  -only-testing:LingxiCodeTests/LocalAppsStoreTests/testSnapshotDegradesInAFixedOrderAndSaysSo 2>&1 | tee /tmp/t6-ios.log
cd ../android && ./gradlew :app:testPlayDebugUnitTest --rerun-tasks --tests '*LocalAppWebViewTest*' 2>&1 | tee /tmp/t6-android.log
```
Expected: 两端都 FAIL。

- [ ] **Step 3: 两端把 `snapshot()` 改成先建后削**

```javascript
          const snapshot = () => {
            const out = {
              title: clean(document.title),
              url: location.href,
              documentState: document.readyState,
              viewport: vvOf(),
              canvases: deepQuery('canvas', 16).map(c => ({ rect: rectOf(c) })),
              canvasCount: deepQuery('canvas', 64).length,
              runtimeErrors: (window.__lingxiRuntimeErrors || []).slice(0, 8),
              runtimeErrorsDropped: window.__lingxiRuntimeErrorsDropped || 0,
              elements: candidates().slice(0, 200).map(element => {
                const rect = element.getBoundingClientRect();
                const sensitive = isSensitive(element);
                return {
                  elementId: clean(element.id) || null,
                  role: roleOf(element) || null,
                  name: nameOf(element) || null,
                  value: sensitive ? null : clean(element.value),
                  checked: typeof element.checked === 'boolean' ? element.checked : null,
                  disabled: !!element.disabled,
                  visible: rect.width > 0 && rect.height > 0,
                  rect: [Math.round(rect.left), Math.round(rect.top), Math.round(rect.width), Math.round(rect.height)],
                };
              }),
              truncated: [],
            };
            // 256 KiB is a HARD failure on the result channel, so leave room for
            // the JSON envelope and degrade in a fixed order rather than dying.
            const BUDGET = 200 * 1024;
            const truncated = out.truncated;
            const size = () => JSON.stringify(out).length;
            for (const seg of ['elements', 'canvases', 'runtimeErrors']) {
              if (size() <= BUDGET) break;
              if (seg === 'elements') out.elements = out.elements.slice(0, 50);
              else if (seg === 'canvases') out.canvases = [];
              else out.runtimeErrors = [];
              truncated.push(seg);
            }
            return out;
          };
```

⚠️ 上面 `elements` 的 map 回调与 Task 2 Step 3 里的**逐字相同**（含 `rect:` 那一行），
两端也必须逐字相同——钉桩测试是按子串匹配的，改一个空格就会在一端命中、另一端漏掉。

- [ ] **Step 4: 跑两端测试确认通过**

同 Step 2 的命令。Expected: 两端 PASS。

- [ ] **Step 5: 提交**

```bash
git add clients/ios/Sources/LocalApps/LocalAppWebView.swift clients/ios/Tests/LocalAppsStoreTests.swift \
        clients/android/app/src/main/java/com/lingxi/code/localapps/LocalAppWebView.kt \
        clients/android/app/src/test/java/com/lingxi/code/localapps/LocalAppWebViewTest.kt
git commit -m "Budget the snapshot so the ledger cannot silence its own criterion

The result channel fails outright over 256 KiB rather than truncating, and
the new fields add roughly forty kilobytes. Left alone, the more runtime
errors an app produced the less observable the runtime-error criterion
became."
```

---

### Task 7: `capture_ui` 增加可选 `rect`（引擎侧）

`capture_ui` 现在只收 `{app_id}`，并且显式拒绝裁剪（`local_apps_host.rs:3603-3605` 的注释）。区域标注要的是那一块的像素。**新参数走既有的不透明 `value`，不动 DTO、不动协议。**

**Files:**
- Modify: `lingxi-code/apps/engine-mobile/src/local_apps_mcp.rs:924-928`（工具 schema）
- Modify: `lingxi-code/apps/engine-mobile/src/local_apps_host.rs:3597-3613`（`capture_ui`）
- Test: `lingxi-code/apps/engine-mobile/src/local_apps_host.rs` 的 `#[cfg(test)] mod tests`（若无则新建 `tests/local_app_capture_rect.rs`）

**Interfaces:**
- Consumes: 既有 `AppUiRequestDto`、`self.request_ui`
- Produces: 当 `rect` 存在时，`AppUiRequestDto.value = Some(r#"{"rect":{"x":..,"y":..,"width":..,"height":..}}"#)`；无 `rect` 时 `value` 仍为 `None`（整帧，行为不变）

- [ ] **Step 1: 写失败测试**

新建 `lingxi-code/apps/engine-mobile/tests/local_app_capture_rect.rs`：

```rust
//! Phase 1a — a region annotation needs the pixels of the region. The rect
//! rides the existing opaque `value` string, so no DTO and no protocol move.

#![allow(clippy::unwrap_used)]

use engine_mobile::local_apps_host::capture_ui_value_for_test;

#[test]
fn capture_without_a_rect_keeps_the_whole_frame_behaviour() {
    let value = capture_ui_value_for_test(&serde_json::json!({ "app_id": "demo" })).unwrap();
    assert_eq!(value, None, "no rect means the whole view, exactly as before");
}

#[test]
fn capture_with_a_rect_serializes_it_into_the_opaque_value() {
    let value = capture_ui_value_for_test(&serde_json::json!({
        "app_id": "demo",
        "rect": { "x": 10, "y": 20, "width": 120, "height": 80 }
    }))
    .unwrap()
    .expect("a rect must produce a value payload");
    let parsed: serde_json::Value = serde_json::from_str(&value).unwrap();
    assert_eq!(parsed["rect"]["x"], 10);
    assert_eq!(parsed["rect"]["width"], 120);
}

#[test]
fn capture_rejects_a_non_finite_or_non_positive_rect() {
    for bad in [
        serde_json::json!({ "x": 0, "y": 0, "width": 0, "height": 10 }),
        serde_json::json!({ "x": -1, "y": 0, "width": 10, "height": 10 }),
    ] {
        let out = capture_ui_value_for_test(&serde_json::json!({ "app_id": "demo", "rect": bad }));
        assert!(out.is_err(), "invalid rect must be refused host-side: {bad}");
    }
}
```

- [ ] **Step 2: 跑测试确认它失败**

```bash
cd lingxi-code && cargo test -p engine-mobile --all-features \
  --test local_app_capture_rect --no-fail-fast 2>&1 | tee /tmp/t7.log
```
Expected: FAIL（`capture_ui_value_for_test` 未定义）。

- [ ] **Step 3: 实现**

在 `local_apps_host.rs` 里新增（放在 `capture_ui` 上方）：

```rust
/// Build the opaque `value` payload for a capture request.
///
/// The rect rides `AppUiRequestDto.value` — an `Option<String>` the wire
/// already carries — so a region crop costs no DTO change. Shape and
/// finiteness are checked here; CLAMPING to the viewport happens on the
/// client, which is the only side that knows the real viewport.
pub fn capture_ui_value_for_test(input: &Value) -> Result<Option<String>, String> {
    capture_ui_value(input)
}

fn capture_ui_value(input: &Value) -> Result<Option<String>, String> {
    let Some(rect) = input.get("rect") else { return Ok(None) };
    let field = |name: &str| -> Result<f64, String> {
        rect.get(name)
            .and_then(Value::as_f64)
            .filter(|v| v.is_finite())
            .ok_or_else(|| format!("capture_ui rect.{name} must be a finite number"))
    };
    let (x, y, w, h) = (field("x")?, field("y")?, field("width")?, field("height")?);
    if x < 0.0 || y < 0.0 || w <= 0.0 || h <= 0.0 {
        return Err("capture_ui rect must have non-negative origin and positive size".into());
    }
    Ok(Some(
        serde_json::json!({ "rect": { "x": x, "y": y, "width": w, "height": h } }).to_string(),
    ))
}
```

把 `capture_ui` 里的 `value: None,` 改成：

```rust
            value: capture_ui_value(&input)?,
```

并把 `local_apps_mcp.rs:927` 的 schema 换成：

```rust
                json!({"type":"object","properties":{
                    "app_id": app_id.clone(),
                    "rect": {"type":"object","description":"Optional region to crop, in viewport CSS pixels. Omit for the whole view.",
                             "properties":{"x":{"type":"number"},"y":{"type":"number"},
                                           "width":{"type":"number"},"height":{"type":"number"}},
                             "required":["x","y","width","height"],"additionalProperties":false}
                },"required":["app_id"],"additionalProperties":false}),
```

- [ ] **Step 4: 跑测试确认通过**

```bash
cd lingxi-code && cargo test -p engine-mobile --all-features \
  --test local_app_capture_rect --no-fail-fast 2>&1 | tee /tmp/t7.log
```
Expected: 3 passed。

- [ ] **Step 5: 确认没有回归，且确实没碰协议**

```bash
cd lingxi-code && cargo test -p engine-mobile -p client-protocol --all-features --no-fail-fast 2>&1 | tee /tmp/t7-all.log
grep -A 20 "failures:" /tmp/t7-all.log || echo "no failures block"
git diff --name-only | grep -E "client-protocol|protocol\.ts" && echo "PROTOCOL TOUCHED — STOP" || echo "protocol untouched, good"
```
Expected: 0 failed，且最后一行是 `protocol untouched, good`。

- [ ] **Step 6: 提交**

```bash
git add lingxi-code/apps/engine-mobile/src/local_apps_mcp.rs lingxi-code/apps/engine-mobile/src/local_apps_host.rs lingxi-code/apps/engine-mobile/tests/local_app_capture_rect.rs
git commit -m "Let a capture ask for one region without moving the wire

The rect travels in AppUiRequestDto.value, an Option<String> the protocol
already carries, so a region crop costs no DTO change and no bless. The
host checks shape and finiteness; clamping stays on the client, which is
the only side that knows the real viewport."
```

---

### Task 8: iOS 按 `rect` 裁剪（修正后的数学）

⚠️ **不能复用既有的长边计算。** `snapshotWidth` 是从**整个 view 的 bounds** 推的
（`LocalAppWebView.swift:482-488`），直接复用会把裁剪区**放大**：393×852 pt / 3× 屏下
`targetPointEdge = 341.3`、`snapshotWidth = 157.4` pt，此时把 `configuration.rect` 设成
118×74 的区域，WebKit 会把它拉到 157.4 pt 宽（约 472 px），1.33× 模糊放大，还把 170 KiB
的 JPEG 预算花在放大出来的像素上。

**Files:**
- Modify: `clients/ios/Sources/LocalApps/LocalAppWebView.swift`（`captureFrame`，`:462-520`）
- Test: `clients/ios/Tests/LocalAppsStoreTests.swift`

**Interfaces:**
- Consumes: Task 7 的 `value` JSON（`{"rect":{...}}`）
- Produces: 纯函数 `LocalAppWebViewController.snapshotWidthPoints(rect:capPoints:) -> CGFloat`，供测试与 `captureFrame` 共用

- [ ] **Step 1: 写失败测试**

```swift
/// Phase 1a — reusing the whole-view ladder for a crop UPSCALES it. A tall
/// narrow crop must be capped on its own long edge and never enlarged.
func testCropIsCappedOnItsOwnLongEdgeAndNeverUpscaled() {
    let cap: CGFloat = 1_024 / 3   // 1024 px cap on a 3x screen, in points
    // Tall crop: long edge is the height, so the cap applies to height.
    let tall = LocalAppWebViewController.snapshotWidthPoints(
        rect: CGRect(x: 0, y: 0, width: 120, height: 800), capPoints: cap)
    XCTAssertEqual(tall, 120 * (cap / 800), accuracy: 0.01)
    // Small crop: already under the cap, so it must be left alone.
    let small = LocalAppWebViewController.snapshotWidthPoints(
        rect: CGRect(x: 0, y: 0, width: 118, height: 74), capPoints: cap)
    XCTAssertEqual(small, 118, accuracy: 0.01, "a crop under the cap must not be enlarged")
}
```

- [ ] **Step 2: 跑测试确认它失败**

```bash
cd clients/ios && xcodebuild test -scheme LingxiCode \
  -destination "platform=iOS Simulator,name=iPhone 16,OS=latest" \
  -only-testing:LingxiCodeTests/LocalAppsStoreTests/testCropIsCappedOnItsOwnLongEdgeAndNeverUpscaled \
  2>&1 | tee /tmp/t8.log
```
Expected: FAIL（`snapshotWidthPoints` 未定义）。

- [ ] **Step 3: 实现**

在 `LocalAppWebViewController` 里新增：

```swift
    /// Long-edge cap for a snapshot region, in POINTS.
    ///
    /// The whole-view path derives `snapshotWidth` from the view's bounds; a
    /// crop must derive it from the crop, or WebKit scales the region UP to
    /// the view-sized width. `min(1, …)` is what forbids the enlargement.
    static func snapshotWidthPoints(rect: CGRect, capPoints: CGFloat) -> CGFloat {
        let longEdge = max(rect.width, rect.height)
        guard longEdge > 0 else { return rect.width }
        let scale = min(1, capPoints / longEdge)
        return rect.width * scale
    }
```

在 `captureFrame` 里，解析出可选 rect 后：

```swift
            let displayScale = max(webView.traitCollection.displayScale, 1)
            let capPoints = 1_024 / displayScale
            let region = requestedRect.map { $0.intersection(bounds) } ?? bounds
            guard region.width > 0, region.height > 0 else {
                return .failure(String(localized: "local_apps_error_ui_capture_unavailable"))
            }
            configuration.rect = region
            configuration.snapshotWidth = NSNumber(
                value: Double(Self.snapshotWidthPoints(rect: region, capPoints: capPoints))
            )
```

`requestedRect` 从请求的 `value` JSON 解析（`{"rect":{"x","y","width","height"}}`，CSS 像素
= 点）。**钳位在客户端做**（`intersection(bounds)`），完全在视口外则回上面那个错误——
不静默改成整帧。

- [ ] **Step 4: 跑测试确认通过**

同 Step 2。Expected: PASS。

- [ ] **Step 5: 提交**

```bash
git add clients/ios/Sources/LocalApps/LocalAppWebView.swift clients/ios/Tests/LocalAppsStoreTests.swift
git commit -m "Cap a crop on its own long edge instead of the view's

The existing ladder derives snapshotWidth from the whole view, so setting
configuration.rect to a region made WebKit scale that region up to the
view-sized width -- a blurry enlargement that also spent the JPEG budget on
invented pixels."
```

---

### Task 9: Android 按 `rect` 裁剪（PixelCopy 源矩形）

⚠️ **不能「先 PixelCopy 整帧再裁」**——那是从一个已经被 1024 钳过的帧里取样，约 2.3×
分辨率损失。要把 CSS→window 的矩形作为 `PixelCopy` 的**源**矩形，目标位图尺寸按裁剪区
定，钳裁剪区，不放大。**`× density` 这一步必须显式写出来**：本仓库已经在
`LocalAppWebView.kt:344-353` 栽过一次密度混淆。

**Files:**
- Modify: `clients/android/app/src/main/java/com/lingxi/code/localapps/LocalAppWebView.kt`（`captureFrame`，`:185-260`）
- Test: `clients/android/app/src/test/java/com/lingxi/code/localapps/LocalAppWebViewTest.kt`

**Interfaces:**
- Consumes: Task 7 的 `value` JSON
- Produces: 纯函数 `internal fun cropSourceRect(cssRect: RectF, density: Float, viewWidthPx: Int, viewHeightPx: Int): Rect` 与
  `internal fun cropTargetSize(sourcePx: Rect, capPx: Int): Pair<Int, Int>`

- [ ] **Step 1: 写失败测试**

```kotlin
/**
 * Phase 1a twin of iOS `testCropIsCappedOnItsOwnLongEdgeAndNeverUpscaled`.
 * The CSS rect must become the PixelCopy SOURCE rect (times density), and the
 * destination is sized from the crop -- never sampled out of an already-capped
 * full frame, and never enlarged.
 */
@Test
fun `crop uses a density-scaled source rect and is never upscaled`() {
    val src = cropSourceRect(RectF(10f, 20f, 130f, 100f), density = 3f, viewWidthPx = 1179, viewHeightPx = 2556)
    assertEquals(30, src.left)
    assertEquals(60, src.top)
    assertEquals(360, src.width())   // (130-10) * 3
    assertEquals(240, src.height())  // (100-20) * 3

    val (w, h) = cropTargetSize(src, capPx = 1024)
    assertEquals(360, w)             // already under the cap -> untouched
    assertEquals(240, h)

    val (bigW, _) = cropTargetSize(Rect(0, 0, 600, 2400), capPx = 1024)
    assertEquals(256, bigW)          // 600 * (1024/2400)
}
```

- [ ] **Step 2: 跑测试确认它失败**

```bash
cd clients/android && ./gradlew :app:testPlayDebugUnitTest --rerun-tasks \
  --tests '*LocalAppWebViewTest*' 2>&1 | tee /tmp/t9.log
```
Expected: FAIL。

- [ ] **Step 3: 实现**

```kotlin
/**
 * CSS rect -> PixelCopy source rect, in real surface pixels.
 *
 * `× density` is the step this repo has already got wrong once
 * (see the note at LocalAppWebView.kt:344-353): a CSS pixel is not a surface
 * pixel, and PixelCopy samples the surface.
 */
internal fun cropSourceRect(cssRect: RectF, density: Float, viewWidthPx: Int, viewHeightPx: Int): Rect {
    val left = (cssRect.left * density).toInt().coerceIn(0, viewWidthPx)
    val top = (cssRect.top * density).toInt().coerceIn(0, viewHeightPx)
    val right = (cssRect.right * density).toInt().coerceIn(left, viewWidthPx)
    val bottom = (cssRect.bottom * density).toInt().coerceIn(top, viewHeightPx)
    return Rect(left, top, right, bottom)
}

/** Cap the CROP's own long edge; `coerceAtMost(1f)` forbids enlargement. */
internal fun cropTargetSize(sourcePx: Rect, capPx: Int): Pair<Int, Int> {
    val longEdge = maxOf(sourcePx.width(), sourcePx.height())
    if (longEdge <= 0) return 1 to 1
    val scale = (capPx.toFloat() / longEdge).coerceAtMost(1f)
    return maxOf(1, (sourcePx.width() * scale).toInt()) to maxOf(1, (sourcePx.height() * scale).toInt())
}
```

在 `captureFrame` 里，把 `PixelCopy.request` 的调用改成传入 `cropSourceRect(...)` 作为
**源**矩形，位图按 `cropTargetSize(...)` 创建。无 `rect` 时源矩形取整个 view，行为不变。

- [ ] **Step 4: 跑测试确认通过**

同 Step 2。Expected: PASS。

- [ ] **Step 5: 两个 flavor 全量 + 最终对账**

```bash
cd clients/android && ./gradlew :app:testPlayDebugUnitTest :app:testDirectDebugUnitTest --rerun-tasks 2>&1 | tee /tmp/t9-all.log
cd ../.. && cd lingxi-code && cargo test -p engine-mobile --all-features --no-fail-fast 2>&1 | tee /tmp/t9-rust.log
grep -A 20 "failures:" /tmp/t9-rust.log || echo "no failures block"
cd .. && git diff --name-only HEAD~9 | grep -E "client-protocol|protocol\.ts|blessed_major|contract_index" && echo "PROTOCOL TOUCHED — STOP" || echo "phase 1a touched no protocol, good"
```
Expected: 全绿，且最后一行是 `phase 1a touched no protocol, good`。

- [ ] **Step 6: 提交**

```bash
git add clients/android/app/src/main/java/com/lingxi/code/localapps/LocalAppWebView.kt clients/android/app/src/test/java/com/lingxi/code/localapps/LocalAppWebViewTest.kt
git commit -m "Crop from the PixelCopy source rect, not from a capped frame

Cropping the full frame afterwards samples out of something already capped
at 1024, losing roughly 2.3x. The CSS rect becomes the source rect times
density -- the density step spelled out, because this file has confused it
before."
```

---

## 真机验收（阶段 1a 的出口门）

单测全绿不构成交付。spec 明确记录过：重建视口/几何的判据历史上必然逃过单测。

- [ ] iOS 真机：DOM 应用与 canvas 应用各一个，`inspect_ui` 返回的 `rect` 与屏幕上肉眼位置一致；`capture_ui` 带 `rect` 取回的正是那一块且不模糊。
- [ ] iOS 真机：打开一个会在渲染时抛异常的模板应用（用出厂 `error-boundary.jsx`，**不要**手写 `throw` 页面），确认 `runtimeErrors` 里出现 `kind: "console"` 的条目。
- [ ] Android：同上三项，用设备或 instrumentation 覆盖。**若该轮上不了 Android 设备，必须记为阶段 1a 未完成**，不能用「Android UI 非目标」把 agent-facing contract 判绿。
- [ ] 引擎改动要上机必须先重建 xcframework（`xcodebuild` 不重编 Rust）：
      `LINGXI_REUSE_STAGED_LINUX_RUNTIME=1 clients/ios/scripts/build-xcframework.sh`，
      判据 `strings -a clients/ios/Frameworks/LingxiCodeFFI.xcframework/ios-arm64/libios_framework.a | grep capture_ui_value`；
      装机后验 `LingxiCode.app/LingxiCode.debug.dylib`（主二进制只有 91 KB，grep 它得 0 = 假阴性）。
