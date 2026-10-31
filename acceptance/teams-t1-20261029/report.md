# Teams T1 — 本地实现与验证

日期：2026-10-29。分支 `claude/teams`，基线 `5245cb47d`。

T1 的团队、成员和邀请功能已在四端接入，并完成以下本地验证。
**这不是完整发布验收：尚未部署 API、规则、Hosting 或 IAM，也未推送；
完整原生包、真机和部署后的双账号团队流程仍待验收。**

## 实现

- 可信 `teamsApi`：创建/删除团队、创建/撤销邀请、加入/退出、移除成员、
  设置管理员/成员，以及 Desktop custom-token 会话与可信会话 ACL 构造。
- 邀请使用次数、成员数、个人团队索引和持久 membership epoch 均通过事务维护。
  加入请求有独立的持久回执；被移除后重放旧请求无法恢复成员身份。
  新的明确加入操作递增 generation，旧移除/角色请求不能操作后来的身份。
- Desktop/Web 共用成员界面，Desktop 的 SDK/凭据保留在 main，IPC 绑定预期 UID。
  iOS/Android 使用现有账号和原生页面。四端均有邀请确认、成员管理、角色限制、
  退出/删除确认和平台/Bot 所有者可读提示。
- 深链只打开确认页，移动端无需本地引擎就绪。待处理邀请保留到登录之后。
  账号切换、监听拒绝及失效事件清空旧视图；旧 HTTP 响应和晚到的原生回调被丢弃。
- `(default)` 单独使用 Team 规则；其他数据库、owner-only RC 授权保持独立。
  Hosting 的 Teams API rewrite 排在 catalog 前，Firestore CSP 仅加入 Teams 页面。

实现契约及 API 字段见 [TEAMS.md](../../docs/TEAMS.md)。

## 验证结果

| 检查 | 结果 |
| --- | --- |
| Auth/Firestore 隔离模拟器 | **33 pass，0 skip**：26 个规则/成员/API 测试，7 个真实 SDK 测试 |
| Web 完整单元套件 | **236 pass，2 skip，0 fail**；两个 SDK skip 均由上面的模拟器命令覆盖 |
| Firebase 常规单元套件 | **23 pass，33 环境 skip，0 fail**；Team 集成由模拟器命令覆盖 |
| Desktop/共享客户端专项 | **19 pass**，含账号切换期间的令牌请求和旧响应隔离 |
| Desktop main/renderer、Web 类型检查 | 通过 |
| Desktop/Web 构建 | 通过；Web 仍有既有 Docs 大 chunk 提示 |
| 实际 Electron 43.1.1 | 最终源码的真实沙箱 preload、IPC、custom-token SDK、监听及创建/邀请/删除通过 |
| 实际浏览器 | React StrictMode 下创建→邀请→登录保留邀请→确认加入→远端移除→重入 generation 2→角色更新→管理员邀请通过 |
| 浏览器收尾回归 | 最终源码删除确认后无遗留权限错误；390px 窄屏无水平溢出 |
| Android | 使用产品版本目录编译实际 Teams 页面/模型/SDK；**13 个邀请及现有深链/inbox JVM 测试通过** |
| iOS | 固定 Firebase 12.19.2，实际 Teams SwiftUI 页面/模型/SDK 的无签名 arm64 构建通过；实际 Foundation 解析器 **10 个场景通过** |
| 新增 Team 本地化资源 | 五个语言目录均有 41 个键；6 份原生输出中的 Team 项与生成结果一致 |
| 独立复审 | 后端与客户端均通过；审查发现的重放、账号边界、监听恢复和登录邀请问题已修复 |

模拟器覆盖：邀请最后一次使用的并发竞争、过期/撤销、移除后下一次读取被拒绝、
旧加入请求在移除和重入之后均被拒绝、新请求正常重入、角色越权、索引一致性、
删除与加入竞争，以及同一 Web SDK 会话在重入后继续收到其他成员变化。

Desktop 静态文案测试原先遗漏已有的 `updateCatalog`，本次补入该测试范围。

## 真实测试账号与免登录复用

用户已明确授权创建两个 `lingxi-5e4b2` 邮箱测试账号，Firebase 连接器已查询确认。
随后用户明确授权登录 `lingxios.net` 并保存隔离浏览器会话。
两个账号均已登录，并通过保存的 IndexedDB storage state 在**新的独立浏览器
上下文中免登录恢复**。此项验证的是现有账号/登录页面，不是尚未部署的线上 Team API。

凭据、刷新令牌和浏览器状态只保存在本机 Git 忽略目录
`output/teams-t1-private/`，目录权限 0700、三个文件均为 0600。
Web 开发服务器对该目录返回 HTTP 403；核验时未读取响应正文。
这些文件及真实测试账号标识未纳入此报告或 Git。

复用账号：`node apps/firebase/scripts/teams-test-accounts.mjs`。
已有账号可直接刷新令牌；不要再次运行 `--create` 或更改既有账号密码。
移动/移除工作树前，需单独保留这个私有目录。

## 仍待完成的门槛

1. 部署及 IAM 预检：`push-api` 现有 Firestore 权限限于 `push-notifications`，
   需确认 `(default)` 数据权限和 custom-token 自签名权限。按交接约定另行批准部署。
2. 完整原生产品/真机：Android 全产品 Kotlin 构建在本工作树缺少固定版本的
   MobileLinux Maven 包和 sherpa AAR；隔离目标已编译实际新源码。
   iOS 完整产品框架、签名、产品 XCTest 与真机流程尚未验证。
   本次没有安装应用、改变设备上的既有账号或重建 Rust/FFI。
3. 全仓本地化 freshness gate：当前有 6 份原生输出与既有 canonical 源不一致，
   在未修改的基线 `5245cb47d` 上也已复现。涉及其他功能的遗留条目，未在 T1 中
   全量覆盖/删除；仅生成并核验新增 Team 项。可选 generator pytest 套件因环境缺少
   pytest 未运行。详见两份 resource 日志。
4. 使用已保存的真实测试账号，在部署后完成四端 Team 创建、邀请、移除/重入和角色验收。
   T2 消息、T3 Bot、T4 委托、T5 Local Apps 仍未实施。

## 证据与复现

- [results.json](results.json)：检查结果、源码 SHA-256、日志 SHA-256。
- [browser-result.json](browser-result.json)：已执行的浏览器流程与范围。
- [成员界面](browser-member.png)、[窄屏界面](browser-narrow.png)：均为模拟器虚构数据。
- `logs/*.log.gz`：16 份压缩日志，包含最终通过结果和明确的既有阻挡项。
- 规则/API/SDK：`npm --prefix apps/firebase run test:teams`。
- 实际 Electron/Web：通过同一 runner 将 `TEAM_SPIKE_COMMAND` 指向
  `apps/firebase/scripts/teams-members-runtime.mjs`，打开打印的回环 URL。
  完成浏览器检查后在打印的 scratch 目录创建 `stop` 文件，服务和模拟器会正常退出。
- 原生复现：[probes/README.md](probes/README.md)。

所有测试服务均已停止。未清理任何缓存，保留 2 GiB 磁盘余量门槛。
