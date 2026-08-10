# iOS：手写 Drawer → NavigationSplitView

日期：2026-08-09
决定：**方案 B** —— 放弃手写侧边浮层，改用官方 `NavigationSplitView`。
状态：未开工。本文是给新 session 的执行方案。

## 为什么这不是等价替换（先读这段）

iOS **没有**官方的「侧边抽屉」。左侧盖住内容的滑出层是 Material（Android）的
navigation drawer，Apple 从未提供，所以市面上的 iOS drawer 全是手写的。

`NavigationSplitView` 在 iPhone（compact）上**会塌缩成 `NavigationStack`**：
侧栏是**全屏一页、push/pop**，不是浮层。

| | 现状 Drawer | NavigationSplitView（iPhone） |
|---|---|---|
| 形态 | 左侧浮层，聊天仍可见 | 全屏一页 |
| 开关 | 自定义按钮 | 系统 toolbar 按钮，定制空间小 |
| 手势 | **没有** | 系统提供 |
| iPad | 无适配 | 自动多栏 |

⚠️ **交互会变**：现在「一边看聊天一边切会话」是一步，改完是两步。这是已经
被接受的产品取舍（用户选了 B），实现时不要试图"两全"再搭一层浮层回来——那
等于把手写 drawer 换个地方重写。

来源：
- <https://fatbobman.com/en/posts/new_navigator_of_swiftui_4/>
- <https://www.hackingwithswift.com/books/ios-swiftui/working-with-two-side-by-side-views-in-swiftui>
- <https://www.createwithswift.com/exploring-the-navigationsplitview/>
- <https://www.hackingwithswift.com/quick-start/swiftui/how-to-control-which-navigationsplitview-column-is-shown-in-compact-layouts>

## 现状（已核实的代码事实）

- `clients/ios/Sources/Drawer/Drawer.swift` — 587 行，**只是内容**
  （chats/projects/搜索/新建/导入 等 section）。可以基本原样搬进 sidebar 列。
- `clients/ios/Sources/App/RootView.swift:271-300` — 呈现层：
  `ZStack { ChatView; if navigation.drawerOpen { Drawer().transition(.move(edge:.leading)).zIndex(50) } }`
- `RootView.swift:316` — 外层 `.animation(.spring(...), value: navigation.drawerOpen)`
- `RootView.swift:322-333` — `closeDrawerThen`：先 `withAnimation` 关抽屉，再
  `Task.yield()` 延后一帧才改 presentation。注释原文：*iOS 26 上同一个动画
  transaction 里改 presentation 会让 SwiftUI 丢弃它*。
- `clients/ios/Sources/App/AppNavigation.swift:61-141` — `drawerOpen` 状态机，
  **9 处**在导航时把它置 false。
- `grep DragGesture Sources/` → **零命中**。当前没有任何拖拽开合。

## 已知缺陷（改造要顺带消掉的）

1. **没有手势**：不能边缘滑出、不能拖着关。
2. **条件插入 + transition + 外层 animation(value:)** 三者叠加，是 SwiftUI 里
   动画丢失/闪烁的经典组合。
3. `closeDrawerThen` 的 `Task.yield()` 是在和框架时序打架的补丁。

预期：1 和 2 由 `NavigationSplitView` 直接消除；3 大概率随之消失，但**必须实测
确认**，不要默认。

## 执行步骤

1. **`RootView` 改成 `NavigationSplitView(columnVisibility:)`**
   - sidebar 列 = 现有 `Drawer` 的内容（去掉 `onClose` 与自绘关闭按钮）。
   - detail 列 = 现有 `ChatView`。
   - 用 `@State var columnVisibility: NavigationSplitViewVisibility` 替代
     `navigation.drawerOpen`。
2. **`AppNavigation` 收敛**：`drawerOpen` / `showDrawer()` / `closeDrawer()`
   删除或改为驱动 `columnVisibility`。**9 个置 false 的点逐个改**，别漏——
   漏掉的表现是"导航后侧栏还开着"。
3. **删掉 `closeDrawerThen` 的 yield 补丁**，改成直接调用；然后**真机验证**
   settings / terminal / cron / apps 四条路由都能正常弹出。若仍被丢弃，说明
   补丁另有原因，再单独查。
4. **`preferredCompactColumn`**：控制 compact 下默认显示哪一列（应为 detail，
   即聊天），否则冷启动会停在侧栏页。
5. **不要 `.toolbar(.hidden, for: .navigationBar)`**：会连带隐藏系统的侧栏开关
   按钮，用户就再也打不开侧栏了（官方文档明确警告）。
6. `LocalAppsDrawerSection.swift`（61 行）跟着搬。

## 验收

1. iPhone 上能打开/关闭侧栏，用系统手势和系统按钮。
2. 从侧栏点 settings / terminal / cron / apps 四条路由都能正确弹出，且侧栏关闭。
3. 冷启动落在聊天页，不是侧栏页。
4. iPad 上是多栏，不再是浮层。
5. `xcodebuild test -only-testing:LingxiCodeTests` 全绿（当前基线 **321 passed**）。
6. 真机 build + 装机跑通。

## 陷阱备忘

- 本仓库 `.xcodeproj` 由 XcodeGen 从 `project.yml` 生成，**新增 Swift 文件后必须
  `xcodegen generate`**，否则报 "cannot find X in scope"，看着像代码错误。
- `@Environment(SomeType.self)` 在本 app 里只有 `AppState` / `LocalizationManager`
  被注入，其它类型会 force-unwrap 崩溃——sidebar 里要用 store 就当属性传。
  见 `ios-environment-object-traps-2026-08-08`。
