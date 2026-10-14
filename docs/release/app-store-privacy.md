# Apple App Store / Google Play 隐私上架准备

本次新增内容已实现，但不代表发布包已经满足提交条件。正式主体使用 LingXi；Flare 测试签名不能用作正式运营主体。联系邮箱：support.lingxi@gmail.com。

## 发布后填写的 URL

- Privacy Policy URL：https://lingxios.net/privacy（中英文，可公开访问，无需登录）
- Google Play 删除申请 URL：https://lingxios.net/data-deletion（通过邮箱申请，支持团队需实际受理）

目前仅修改源码，必须发布网站并实测以上公网 URL，再填写市场表单。按正式商店开发者资料核对 LingXi 的主体名称；如登记的是法人实体，应将法定名称补充到政策。

## 当前实现与待完成项

- 已增加网站页脚、Desktop 关于页、Android/iOS 数据与隐私页及注册/登录区域的政策入口。
- 删除说明区分 Firebase 账号、设备本地数据、工作区/导出/备份及第三方提供商数据。
- 已移除 iOS 隐私页中未接入持久化/后端的训练与崩溃报告开关，以及无行为的导出和删除按钮。
- **提交阻塞：真实的应用内账号删除流程尚未实现。** Apple 要求支持创建账号的应用提供应用内删除；普通应用不能仅用联系支持替代。Google Play 要求应用内和应用外都能发起删除。邮件说明页只提供应用外申请路径。
- 删除实现需覆盖近期登录/重新认证、不可逆操作确认、Firebase 账号删除、关联数据清理、成功/失败状态及本地退出。Apple 登录还需完成 Apple 授权令牌撤销；不能将退出登录当作账号删除。
- 支持邮箱必须可收件并实际处理所有权验证、删除请求、处理时间通知和依法保留例外。不要只发布无法受理的申请地址。

## 隐私表单核对底稿

以下是源码数据流底稿，不应直接勾选“未收集数据”。最终按正式 StoreRelease / playRelease 产物、服务配置和供应商实际行为填写；应用内共享及提供商保留也需按商店定义判断。

| 数据类别 | 用途及接收方 | 核对重点 |
| --- | --- | --- |
| 邮箱、姓名、用户标识、认证信息 | Firebase Authentication；Google/Apple 登录 | 身份关联、账号管理、认证服务网络/设备元数据 |
| 对话、代码、文件内容、工具结果 | 用户选择的 AI 端点、远程桥接、MCP/插件 | 端点/代理/多模型实际路由，保留与训练规则 |
| 图片、语音、转写文本 | 模型、系统语音服务或配置的音频服务 | 本地与云端区分；系统识别也可能联网 |
| 联系人、日历、位置、屏幕内容 | 授权的工具/本地应用及后续任务上下文 | 正式发行渠道可用能力、权限与显著告知 |
| 诊断、支持邮件 | 本地诊断导出、用户发送的支持信息 | 是否新增自动遥测，导出内容是否敏感 |
| 网站请求信息 | 网站托管服务 | IP/浏览器标识、托管日志与保留配置 |

## 提交前验证

1. 确认正式开发者主体、签名、包标识以及收件邮箱；政策版本日期对应实际发布版本。
2. 部署网站，实测匿名访问、直接打开路由、刷新、中英文和移动端排版；检查邮件链接。
3. 在真实 iOS/Android 发布包中检查政策入口和登录前可访问性，权限拒绝/撤回以及实际数据接收方。
4. 完成并端到端验证账号删除，包括 Apple 撤销、重新认证、离线/失败恢复及支持邮箱申请。
5. 根据正式二进制与 SDK 隐私清单核对 Apple App Privacy / Google Data safety；审计 Apple required-reason API 的实际使用并生成相符 PrivacyInfo.xcprivacy，不能凭空声明未使用。
6. 核对第三方 AI 数据共享的告知与同意、语音及高风险权限的显著披露；仅有政策链接不能代替需要的同意流程。

## 官方依据

- Apple App Review Guidelines 5.1：https://developer.apple.com/app-store/review/guidelines/#privacy
- Apple 账号删除：https://developer.apple.com/support/offering-account-deletion-in-your-app/
- Google Play User Data：https://support.google.com/googleplay/android-developer/answer/10144311
- Google Play 账号删除：https://support.google.com/googleplay/android-developer/answer/13327111

核对日期：2026-10-14。此文件是提交准备清单，最终表单需与发布包保持一致。

## 本次验证记录

- 官网 `npm run typecheck`、`npm run build` 及 35 项现有测试通过。
- Playwright 检查中英文政策/删除说明、桌面及 390px 手机宽度，无横向溢出；邮件申请链接目标正确。
- Electron renderer TypeScript 检查通过。
- Android `:app:compilePlayDebugKotlin --offline --no-build-cache` 通过；未进行真机发布包验收。
- iOS 修改文件的 Swift 语法检查通过；未构建或验证签名发布包。
- 翻译生成一致性检查通过（1898 keys，5 locales）。同步已有语音文案到源文件，避免生成时丢失；问答字符串改由同一源文件生成，保留 Android 独立复数资源。
- 未部署网站，未执行市场提交。翻译生成器的额外 Python 测试因本机未安装 pytest 未运行；生成器一致性检查已完成。
