# Firebase 后台资源与部署清单

目标项目：`lingxi-5e4b2`。现有客户端账号继续使用同一个 Firebase Authentication。

Team 的首个客户端 Firestore 访问例外仅限 `(default)` 的 `teams` 数据，契约见
[Teams](TEAMS.md)。本地配置为它单独使用 `firestore.default.rules`；
`model-catalog` 和 `push-notifications` 仍为 deny-all。该配置尚未部署，
T1 的 Team API、四端成员管理和邀请功能已在本地实现，验证记录见
[T1 acceptance](../acceptance/teams-t1-20261029/report.md)。生产部署与完整原生包、
真机及线上团队双账号验收仍待后续。T2 已补充四端成员消息、持久变更补齐、
未读、编辑/撤回和本地定义的 `onTeamMessage`，见 [T2 acceptance](../acceptance/teams-t2-20261029/report.md)。
消息 API 和 create trigger 均未部署；Eventarc/default 数据库与发送身份的权限仍需发布前核对。

`teamsApi` 计划复用 `push-api` 身份。现有该身份的 Firestore 授权限定
`push-notifications`，部署前必须另外确认/授予它对 `(default)` 的必要数据权限，
以及为自身签发 custom token 所需的 `iam.serviceAccounts.signBlob` 权限。
本次没有变更 IAM 或部署规则/函数/Hosting。测试账号已按用户明确授权创建，
独立浏览器会话经授权保存；凭据仅保存在本机私有、Git 忽略目录，不写入报告。

FCM 的 Android/iOS 客户端、独立注册 API、30 天绑定租约和清理代码见
[Firebase Cloud Messaging](firebase-fcm.md)。用户已明确授权并完成该部分生产部署，
Android 真机已验证注册、后台通知、消息到期移除、换账号隔离及离线退出后自动解绑；
iOS APNs 配置由用户处理后继续验收。

Android / iOS 的崩溃采集与符号配置见 [Firebase Crashlytics](firebase-crashlytics.md)。
已启用 `firebasecrashlytics.googleapis.com`；沿用现有 Firebase apps，不新增服务账号私钥。

## 已授权并部署的资源

| 资源 | 名称 / 限制 | 用途 |
| --- | --- | --- |
| Firestore Native 数据库 | `local-app-market`，`us-central1`，删除保护 | 市场、用户角色、上传及任务状态 |
| Firestore Native 数据库 | `model-catalog`，`us-central1`，删除保护 | 模型草稿、分片快照、发布指针及历史 |
| Firestore Native 数据库 | `push-notifications`，`us-central1`，删除保护 | 私有设备绑定、FCM 注册唯一索引及 30 天 TTL |
| Firebase Hosting | `lingxi-5e4b2`、`lingxi-5e4b2-market` | 官网与市场前端，按用户后续要求替换 Cloudflare 静态托管 |
| Cloud Functions 第 2 代 | `marketGateway`，Node 22，512 MiB，并发 4，最小 0 / 最大 2 | Hosting 同域 API 入口，保留后端代理密钥及 Firebase token 校验 |
| Cloud Functions 第 2 代 | `catalogApi`，Node 22，512 MiB，最小 0 / 最大 2 实例 | 目录公共 API 与管理 API |
| Cloud Functions 第 2 代 | `pushApi`，Node 22，256 MiB，最小 0 / 最大 2 实例 | 设备绑定、解绑及有独立权限的推送 API |
| Cloud Scheduler / Functions | `prunePushRegistrations`，每天，最大 1 实例 | 清理过期设备及相应注册索引，已手动触发验证成功 |
| Cloud Run | `market-api`、`market-jobs`，最小 0 / 最大 2 实例 | 市场业务及私有任务处理 |
| Cloud Tasks / Scheduler | 队列 `market`；每分钟调度 | 上传处理、清理、状态修复 |
| IAM 服务账号 | `catalog-api`、`market-api`、`market-jobs`、`market-tasks`、`market-scheduler` | 按服务分配数据库和调用权限 |
| IAM 服务账号 | `push-api`，Firestore 权限限定 `push-notifications`，Auth 只读及 FCM 发送权限 | 推送函数运行身份，无长期私钥 |
| Secret Manager | 市场代理、内容网关和游标密钥 | 服务端密钥，值不进入 Git / Terraform state |
| Artifact Registry / Cloud Build | 构建、保存后台镜像与 Functions 制品 | 服务发布 |

需要启用的 API：`run.googleapis.com`、`cloudbuild.googleapis.com`、
`artifactregistry.googleapis.com`、`cloudtasks.googleapis.com`、
`secretmanager.googleapis.com`、`iam.googleapis.com`、
`iamcredentials.googleapis.com`、`eventarc.googleapis.com`。

这些服务按实际用量计费。最小实例数为 0 不代表全部资源零费用；数据库、存储、
构建制品、队列、调度和密钥也可能产生费用。部署不会配置固定月度承诺。
用户已明确授权按此清单创建和部署。数据库、服务账号、Functions、两个 Cloud Run 服务、任务队列、每分钟调度、密钥及构建资源已创建。

## 现有资源配置

- 独立 Web 应用：市场 `1:55470483519:web:d57888154ffe7dfa57708a`；
  官网管理页 `1:55470483519:web:0100e420a56a3ab957708a`。
- 管理页路径 `/admin/models`。Google / 邮箱密码登录后，服务端验证 ID token、
  已验证邮箱及 `model_catalog_admin: true` 权限；普通账号无法编辑或发布。
- 公共 API：`GET /v1/model-catalog?platform=desktop|android|ios|cli&appVersion=0.1.0&schemaVersion=1`。
  只返回已发布目录；版本上下界均包含，支持预发布版本比较与 ETag / HTTP 304。
- 初始草稿来源为当前 `llm-client/data/providers`：46 个提供商、1327 个模型。
  已发布初始 SDK 目录（revision 1）；没有接入四端启动同步。
- 每份目录上限 8 MiB / 5000 个模型。Firestore JSON 分片每 100000 个 Unicode 码点，
  单片最多约 400 KiB，避开单文档 1 MiB 上限。草稿通过 revision 检查并发冲突；
  发布使用不可变内容与原子指针，历史版本恢复到草稿后必须重新发布。
- Firestore / Storage / Realtime Database 客户端直接读写默认拒绝，业务经后端 IAM 访问。
  已按用户授权部署拒绝客户端直接读写的规则；后台通过 IAM 访问。
- 文件存储已按用户选择改为当前项目 Firebase Storage；统一桶级访问已启用，市场服务账号限制于 `market/` 前缀。

## 本地验证

```sh
cd apps/firebase && npm ci && npm test
cd ../web && npm ci && npm run typecheck && npm test && npm run build
```

Firestore 事务集成测试需要 emulator。设置 `FIRESTORE_EMULATOR_HOST` 后运行
`apps/firebase` 的测试；它使用 `demo-lingxi-backend` 的随机测试数据库，不触碰云端数据。
数据库、角色、Rules、索引、Functions 和两个市场服务已部署；市场详细验收状态见相邻仓库 `local-app-market/docs/firebase-deployment.md`。

## 实际访问与验证

- Firebase Hosting 模型管理：https://lingxi-5e4b2.web.app/admin/models 。官网同域 `/v1/**` 转发 `catalogApi`；静态资源使用带内容哈希的长期缓存，管理页不缓存。
- 市场 Firebase Hosting：https://lingxi-5e4b2-market.web.app ，同域 `/api/**` 转发 `marketGateway`。网关覆盖写入服务端代理凭据，保留 Firebase token 和强 ETag，不转发 Cookie 或调用方的代理密钥。
- `lingxios.net` 主域名已切换为 DNS-only A `199.36.158.100`，旧 apex AAAA 已替换；公开 Google DNS 返回该 A、无 AAAA。Firebase 为 `HOST_ACTIVE` / `OWNERSHIP_ACTIVE`。证书 API 仍标记 `CERT_PROPAGATING`，实际主域名 HTTPS 已使用 Google Trust Services WR3 证书并正常验证 SAN / 信任链；响应来自 Firebase `x-served-by`，页面资源哈希与默认 Hosting 站点一致。
- 旧官网 `lingxios-site` 的 `lingxios.net/*` 路由、workers.dev 和预览入口已停用。临时 HTTP ACME 处理代码、环境变量及路由已移除；所有权 TXT 和证书验证 TXT 保留。内容 Worker 版本为 `2a1e72da-6ee1-4139-a53f-929d30bf7b43`，下载/独立试玩路由保持。
- 主域名验收：`/`、`/docs`、`/admin/models`、带哈希静态资源均为 200；公共目录 200（46 / 1327）、ETag 条件请求 304、未登录管理接口 401。市场健康 200，未知内容/试玩版本 410。浏览器主页与文档导航正常。
- 模型 API：`https://us-central1-lingxi-5e4b2.cloudfunctions.net/catalogApi`。
- 线上健康 200，公共目录 200（46 个提供商 / 1327 个模型），未登录管理接口 401。
- 管理员 `luolingfeng.flare@gmail.com` 和用户后续明确授权的 `luolingfeng.lingxi@gmail.com` 均已授予 `model_catalog_admin: true`。后者 Firebase UID 为 `e63PxUPxxih9Bt2MhRjdJvrfoJC3`，同时配置市场数据库管理员角色。客户端需刷新 token/重新登录获取权限。
- 函数使用专用 `catalog-api` 身份，只能读写 `model-catalog` 数据库，最小 0 / 最大 2，Node 22 / 512 MiB。
- 后端 10 项测试（包含真实 Firestore Emulator 并发事务和市场网关）通过；官网 35 项测试、TypeScript 和构建通过。
- 浏览器通过真实 Auth/Firestore Emulator 验证登录、编辑保存、版本过滤、发布不可变、恢复后再发布、ETag、非法 JSON、刷新会话、移动布局和普通账号拒绝。
- 用户在 Chrome 完成 Google 登录；此前 `ADMIN_REQUIRED` 是因为实际登录账号为 `luolingfeng.lingxi@gmail.com`，与最初授权账号不同。经用户明确授权补充管理员权限并刷新 token 后，生产管理页成功加载草稿 #1、46 个提供商及 1327 个模型；再次刷新页面后仍保持登录并成功加载相同目录。
- 两个 Firebase Hosting 站点已部署并验证 SPA 深链接及同域 API。旧市场 Cloudflare Worker 公开入口已停用；独立下载/试玩内容 Worker 继续保留。Functions 制品保留 14 天。
- 官网原有用量、API Key、账单页面仍是界面原型，不代表已实现新的计费或模型调用代理服务。
