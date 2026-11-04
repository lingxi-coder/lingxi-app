# 统一品牌接入

品牌源为 `assets/brand/identity-v1/`。本轮将设计源接入应用资源与官网，保持 bundle ID、URL scheme、包名、账号和签名配置。

## 2026-11-01 星芒图标统一

母品牌与 Code、Chat、Control、Apps、Remote、Harness、SDK 全部使用同一 ✨ 四角星芒，产品之间只保留既有配色差异。所有尺寸使用同一几何，移除双环及各产品叠加的功能符号；字标中的品牌图形一并更新。

已重新生成主应用 Desktop / iOS / Android / Web 的 35 个平台资源，更新 Chat 默认、深浅备用与 Icon Composer 的 178 个资源、Control 的 SVG / PNG / ICNS，以及应用市场字标、favicon、touch icon。各仓来源摘要已同步；品牌预览截图与 ZIP 已刷新。执行 `python3 assets/brand/identity-v1/build.py --png --archive` 可重新生成设计资源与下载包。

资源校验通过：7 产品的 21 个图标 SVG 共享同一星芒路径；56 个产品 PNG 尺寸正确，8 个 1024 px 母版不透明；各仓资源摘要、Chat 深浅备用图标、Android adaptive 安全区与 Control ICNS 均通过独立复核。浏览器验证预览页 28 种产品 / 主题 / 语言组合无图片加载错误，390 px 手机页面无横向溢出。历史双环路径在本轮品牌源及接入副本中无遗留。

本轮只更新源码资源与本地预览，未部署网站、生成签名发行包或安装真机客户端。以下记录保留原验收日期与范围。

## 2026-11-01 星芒前端发布

用户随后授权提交、推送与上线。四个品牌提交均已推送到对应仓库的 `main`：主应用 `25feabc80900c513e1ec2c093c0edc9aacf7f8df`、Chat `9dfa3d2e24f04cdb5b5fdc05fd7c1f285bfd0093`、Control `f18f271e6bac1d31e4c7e6eb64fceead0eaab1df`、应用市场 `57fc633c0f9fc2743831729f9b232716a5f86a78`。主应用与市场从隔离的已提交工作区构建；原工作区其他改动保留。

| 入口 | Hosting site | 发布版本 |
|---|---|---|
| [官网](https://lingxios.net) | `lingxi-5e4b2` | `a0df56c549ba5794` |
| [应用市场](https://lingxi-5e4b2-market.web.app) | `lingxi-5e4b2-market` | `c8ef79bff9257b01` |

发布范围为两个 Hosting 站点的静态内容及既有 Hosting 配置。官网类型检查、生产构建、263 项测试通过，3 项模拟器测试按原条件跳过；市场类型检查、生产构建、64 项测试通过，4 项 Firebase 环境测试按原条件跳过。

发布后，两个生产域名的首页与品牌资源共 11 项均返回 HTTP 200，SHA-256 与对应提交的构建文件逐项一致。浏览器验证官网深浅主题、两个站点的桌面与 390 px 手机页面，新星芒字标正常加载且无横向溢出，控制台无错误。客户端签名发行包与真机安装仍属于独立验收范围。

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

客户端验证范围为源码及构建资源：未安装或替换真机应用，未生成新的签名发行包，也未执行真实 Windows/Linux 安装验收。Control 的历史固定哈希签名脚本仍用于旧包，不适用于新包。

## 2026-10-25 生产前端发布

用户授权后，官网与应用市场均已部署到 Firebase 项目 `lingxi-5e4b2`。仅发布 Hosting 静态内容与现有 Hosting 配置，没有部署 Functions、数据库规则或其他后端服务。

| 入口 | 构建源提交 | Hosting site | 发布版本 |
|---|---|---|---|
| [官网](https://lingxios.net) | `68366a39c6adf42876312b5f134a6676c08885df` | `lingxi-5e4b2` | `46bba03772d8f1cd` |
| [应用市场](https://lingxi-5e4b2-market.web.app) | `98f96db24660f549d1b6c56d5d4d6a3e02991cab` | `lingxi-5e4b2-market` | `28e1d18deebb8d13` |

两个部署任务均返回成功。生产域名的首页、favicon、Apple touch icon 和品牌 SVG 共 11 项资源均返回 HTTP 200，SHA-256 与本次构建文件逐项一致。浏览器验证新字标正常加载，桌面及 390 px 手机页面正常，手机页面无横向溢出。市场 `/api/v1/health`、`/api/v1/capabilities`、`/api/v1/apps` 均返回 HTTP 200。

本次生产验证为公开页面与接口的只读检查，没有登录、发布应用或修改业务数据。客户端安装与签名验收范围不因前端发布而改变。
