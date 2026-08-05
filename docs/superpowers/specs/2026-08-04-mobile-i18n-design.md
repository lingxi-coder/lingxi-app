# 移动端多语言适配（iOS + Android）

日期：2026-08-04
状态：已批准，待实现

## 背景与目标

灵犀 Code 的 iOS（`clients/ios`，SwiftUI，iOS 17+）与 Android（`clients/android`，纯 Jetpack Compose，minSdk 26 / targetSdk 35）客户端目前**全部 UI 文案硬编码为简体中文**，无任何本地化设施。目标：为双端添加多国语言支持，覆盖 **简体中文（基准）、繁体中文、English、日本語、한국어**，架构可扩展更多语言。

### 已确认需求

| 维度 | 决定 |
|---|---|
| 语言 | zh-Hans（基准）、zh-Hant、en、ja、ko，可扩展 |
| 切换方式 | 跟随系统 + 设置内手动覆盖（双端一致） |
| 本地化范围 | 完整 UI 文案 + 系统权限文案 + App 名称 |
| 翻译来源 | AI 生成初稿，人工审校（只审一份 JSON） |
| App 名称 | zh-Hans / zh-Hant / ja / ko = `灵犀`；en = `Lingxi` |
| 方案 | 平台原生 + 单一翻译源 + 生成管线（方案 A） |

## 现状盘点（探查结论）

### iOS（clients/ios）

- ~286 处 `Text("中文")` 硬编码字面量，分布在 33 个 Swift 文件（Settings ≈81、Conversation ≈63、LocalApps ≈60、Cron ≈22、Drawer ≈19、Onboarding ≈16、Voice ≈7、Terminal ≈4、其余）
- **零**本地化设施：无 `.strings` / `.stringsdict` / `.xcstrings` / `.lproj`；无 `NSLocalizedString` / `String(localized:)` / `LocalizedStringKey`
- Info.plist：`CFBundleDisplayName = 灵犀` + 7 条权限文案（NSCameraUsageDescription 等）均为中文硬编码
- 项目由 xcodegen 从 `project.yml` 生成，pbxproj 不入库；`project.yml` 无 developmentRegion / knownRegions 设置
- 已有 `voiceLanguage` 偏好（UserDefaults，语音 STT/TTS 用）——与 UI 语言无关，不混用
- 主 UI 为 SwiftUI，少量 UIKit 片段（VoiceInteractionController、LocalAppWebView、Capabilities helpers）

### Android（clients/android）

- ~1300+ 处 UI 硬编码字符串（正则口径 ~2400 含日志/异常）；`stringResource()` 仅 LocalAppsScreen 使用 14 处
- 仅 2 个资源文件：`app/src/main/res/values/strings.xml`（含 app_name、local_apps_* 17 行）与 `app/src/direct/res/values/strings.xml`（direct 风味）；**无任何 locale 变体**
- MainActivity 继承 `ComponentActivity`（非 AppCompatActivity），无 appcompat 依赖
- 无应用内语言切换逻辑（无 setApplicationLocales / Locale 覆写）
- Cron 通知（CronNotifications.kt）、NotificationController.kt 的标题/正文为用户可见文案
- UI 测试 tag 常量在 `components/UiTags.kt`（内部标识，不翻译）

### 共享层

- Electron 无 i18n 库（硬编码英文）；`clients/shared` 为 bridge-client，无 UI 字符串
- 现有唯一 key 惯例：snake_case（`local_apps_title` 等）——双端统一沿用

## 设计

### 1. 翻译源与生成管线

**权威翻译源**：`clients/translations/` 目录

```
clients/translations/
├── zh-Hans.json    # 基准（抽取自现有硬编码），唯一权威
├── zh-Hant.json
├── en.json
├── ja.json
├── ko.json
└── generate.py     # 生成器 + 校验
```

- 扁平 snake_case key，功能前缀分组：`chat_`、`composer_`、`settings_`、`local_apps_`、`permission_`、`cron_`、`terminal_`、`voice_`、`drawer_`、`onboarding_`、`common_`、`app_`
- key 命名示例：`composer_placeholder`、`chat_open_new_conversation`、`settings_language_title`、`permission_camera_usage`、`app_name`、`common_cancel`、`common_confirm`
- **生成器** `generate.py`（python3，仓库已有 python 工具链）：
  - 输出 iOS：`clients/ios/Resources/Localizable.xcstrings`（String Catalog，含 InfoPlist 段）
  - 输出 Android：`clients/android/app/src/main/res/values/strings.xml`（基准中文，扩展现有文件）+ `values-zh-rTW/`、`values-en/`、`values-ja/`、`values-ko/`（`values-zh` 不建：基准即简体）
  - **幂等**：重复运行产物一致；**key 一致性校验**：各语言 key 集合必须与 zh-Hans 完全一致，缺失/多余即非零退出
  - `app_name` 等非翻译差异（同 key 不同 locale 值）按正常翻译处理
- 新增语言流程：加 `xx.json` → 跑 `generate.py` → 双端资源自动生成

### 2. iOS 侧

**String Catalog**（Xcode 15+ 原生，JSON 可脚本生成；部署目标 iOS 17 满足要求）：

- `project.yml` 增加项目级设置：
  - `developmentRegion: zh-Hans`
  - `knownRegions: [zh-Hans, zh-Hant, en, ja, ko]`
  - Info.plist `CFBundleLocalizations` 同集合
  - 资源目录 `Resources/` 加入 target sources（含 `Localizable.xcstrings`）
- **代码替换**：
  - `Text("字面量")` → `Text("snake_key")`（String Catalog 默认表；zh-Hans 为开发语言即回退语言）
  - 动态字符串 → `String(localized: "key")`
  - 带参数 → `String(localized: "...\(arg)")` + Catalog 占位符（`%lld` 等）
  - 需保证：所有 `Text()` 中出现的 key 均存在于 Catalog，缺失 key 的 UI 直接显示 key 名——用生成器校验兜底
- **语言切换（跟随系统 + 应用内覆盖）**：
  - `LocalizationManager`（`@MainActor`，ObservableObject，App 根注入）
  - 持久化：UserDefaults `appLanguage`（nil = 跟随系统；值如 `zh-Hans`、`en`、`ja`、`ko`、`zh-Hant`）
  - 机制：经典 `Bundle.main` swizzle（`localizedString(forKey:value:table:)`），使 `String(localized:)` 与 SwiftUI `Text` 自动读取所选语言 bundle
  - 切换后：保存 → 重建根视图（根 `id(locale)` 强制刷新）→ 设置页即时生效
  - 备选方案（不采用）：`AppleLanguages` + 强制重启——体验差
- **权限文案**：Info.plist 中 7 条 `NS*UsageDescription` 与 `CFBundleDisplayName` 移入 Catalog 的 InfoPlist 段（5 语言），Info.plist 本体仅保留占位（`$(APP_NAME)` 或留空由 Catalog 覆盖；实现时按 xcodegen 支持方式落地）
- App 名称：zh-Hans / zh-Hant / ja / ko = `灵犀`；en = `Lingxi`（CFBundleDisplayName 本地化）

### 3. Android 侧

**资源**：

- `res/values/strings.xml`：基准中文（现有文件扩展，生成器覆写）
- 新增 `res/values-zh-rTW/strings.xml`、`values-en/`、`values-ja/`、`values-ko/`
- 非翻译项（测试 tag、日志、内部标识）→ `translatable="false"`

**代码替换**：

- Compose：`Text("...")` → `Text(stringResource(R.string.x))`；`contentDescription` / `semantics` / `Modifier.semantics` 标签同样处理
- 参数化：`stringResource(R.string.x, arg)`
- **ViewModel / 非 Composable 上下文**（ChatViewModel.kt 等状态与错误文案）：
  - 优先 `AndroidViewModel.getString(R.string.x)`；若现有 VM 基类不便于持有 Context，则注入轻量 `ResourceProvider`（实现时按现有架构二选一，接口：`fun getString(@StringRes res: Int, vararg args: Any): String`）
- 通知文案（CronNotifications、NotificationController）→ `context.getString(...)`

**语言切换（minSdk 26、无 AppCompat、纯 Compose）**：

- `AppLanguageStore`（SharedPreferences，key `app_language`，nil = 跟随系统）
- `MainActivity.attachBaseContext` 覆写：用存储的语言构造 `Configuration.setLocale` 的 ContextWrapper
- 切换后 `recreate()`（Compose 全量重建，`LocalConfiguration` 随之更新）
- 无需 manifest `configChanges` 特判（全 Compose 单 Activity）

### 4. 语言切换 UX（双端一致）

- 入口：设置 → 「语言 / Language」页
- 选项：**跟随系统 / 简体中文 / 繁體中文 / English / 日本語 / 한국어**
- 切换即时生效（iOS 重建根视图；Android recreate）
- 语言名以各自母语展示（`简体中文`、`繁體中文`、`English`、`日本語`、`한국어`），不随当前语言翻译

### 5. 抽取与翻译

- **抽取顺序**：设置 → 对话（Conversation）→ LocalApps → Onboarding → Drawer/Cron/Terminal/Voice → 其余
- 抽取方式：按文件逐批替换（脚本辅助定位 `Text("...")`，人工/子代理确认语义并分配 key）；Android ViewModel 字符串单独处理
- key 总量预估：约 500–700（iOS ~286 处去重后 ~250，Android UI ~400–600 去重后 ~350）
- **翻译**：AI 生成 zh-Hant / en / ja / ko 初稿写入 JSON → 人工审校 JSON → 重跑生成器

### 6. 验证

- 生成器：幂等校验 + key 一致性校验（CI 可挂）
- 构建：`xcodebuild` 双端 Debug 构建通过；Android `./gradlew assembleDirectDebug assemblePlayDebug`（按现有风味）通过
- 运行：iOS 真机安装启动（沿用 `devicectl` 流程）；Android 模拟器/设备安装启动
- UI 测试：引用字面量的断言改为资源 key 或保持默认语言断言；测试 tag 不动
- 语言切换手测：5 语言 + 跟随系统共 6 态，双端逐一验证关键页（设置、对话、LocalApps）

### 7. 实施顺序

| 阶段 | 内容 | 出口标准 |
|---|---|---|
| P0 | 翻译源骨架 + generate.py + key 一致性校验 | 生成器跑通，双端资源文件可生成 |
| P1 | iOS：抽取全部 UI 字符串 → Catalog；LocalizationManager + 语言页 | iOS 构建通过，真机切换 5 语言生效 |
| P2 | Android：抽取全部 UI 字符串 → 资源变体；语言切换 + 语言页 | Android 构建通过，设备切换生效 |
| P3 | AI 生成 zh-Hant/en/ja/ko 初稿 | key 一致性校验通过，人工审校 |
| P4 | 验证：双端构建/运行/UI 测试更新 | 全部出口标准达成 |

## 边界与不做的事

- 不做 Electron 客户端 i18n（保持现状）
- 不引入第三方 i18n 库（双端均平台原生）
- 不翻译 AI 生成内容、终端输出、运行时内容
- 不动 `voiceLanguage`（语音偏好，与 UI 语言独立）
- 不改动 `clients/shared`（无 UI 字符串）
