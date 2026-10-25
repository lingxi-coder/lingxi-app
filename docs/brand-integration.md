# 统一品牌接入

品牌源为 `assets/brand/identity-v1/`。本轮将设计源接入应用资源与官网，保持 bundle ID、URL scheme、包名、账号和签名配置。

## 产品仓库

- Desktop：ICNS、ICO、PNG、通知示意图、窗口图标及打包资源；Windows 打包写入 EXE 图标和产品 label，Linux 注册文件引用随包 PNG。
- iOS：AppIcon 全部尺寸；英文、日文、韩文系统显示名统一为 LingXi；简体为灵犀，繁体为靈犀。
- Android：五个 density 的启动、圆形与 adaptive foreground；背景色与系统显示名同步。
- 官网：共享转曲中英文字标、深色反白版本、favicon、Apple touch icon；SDK 卡片使用系列图标和 LingXi 名称；远控页面标题为 LingXi Remote / 灵犀远控。

执行 `python3 scripts/sync-brand-assets.py` 可从设计 SVG 重新导出平台资源，需要 Pillow 和 rsvg-convert。`--check` 校验源文件和导出文件摘要，无需图像依赖。语言资源由 `resources/translations/generate.py` 维护。

## 其他仓库

- `computer-use`：LingXi Control / 灵犀操控，接入窗口、导航、Dock、favicon 和 macOS 包图标。固定原有 Computer Use 数据目录，先创建目录再设置，保留显式诊断 profile；不改 Keychain service。
- `chat/ios`：LingXi Chat / 灵犀聊天，接入 Icon Composer、默认与备用图标、系统显示名称、英文旧 iGroup 显示文案和启动页色彩。备用图标资源 ID 与协议标识不变。
- `local-app-market`：LingXi Apps / 灵犀应用，接入导航字标、favicon、touch icon、页面标题与页脚。

其他仓库保留对应 SVG 副本与来源记录，独立构建不依赖相邻 checkout。

## 验证

- 主应用 Desktop：Node/Web 类型检查和 Electron 构建通过；40 项打包/协议注册/Windows PE 资源测试通过。Windows 测试验证写回后可解析、label 正确、原 manifest 与版权资源保留。
- 主应用 iOS 与 Chat：Xcode `actool` 实际编译图标成功，包括 Chat Icon Composer 和传统图标；Chat plist/localized system label 语法通过。
- Android：`:app:processPlayDebugResources --offline` 通过。需 Python 3.11+ 供现有 native preflight 使用；系统 Python 3.9 不满足其 tomllib 依赖。
- 官网：类型检查、生产构建、测试通过；桌面/390 px 手机预览及深色字标检查通过。
- 应用市场：类型检查、33 项测试及生产构建通过；4 项需 Firebase 环境的测试按原有条件跳过；桌面/390 px 手机品牌预览通过。
- Control：独立 worktree 的 Node/Web 类型检查、Electron 构建和定向测试通过；首次数据目录创建/既有数据保留/显式 profile 测试经独立复核通过。

本轮是源码及构建资源交付。未部署官网或市场到生产，未安装或替换真机应用，未生成新的签名发行包，也未执行真实 Windows/Linux 安装验收。Control 的历史固定哈希签名脚本仍用于旧包，不适用于新包。
