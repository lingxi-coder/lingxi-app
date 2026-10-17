# Firebase Cloud Messaging

项目为 `lingxi-5e4b2`。Android Play/Direct 和 iOS Store/Full 使用同一 Firebase Auth
账号体系，通过专用 `pushApi` 注册推送。使用 SDK 当前推荐的 FID 注册及 Admin SDK `fid`
发送接口，不添加旧 token API 兼容分支。Desktop/CLI 没有原生 FCM 接收 SDK，本次不注册虚假设备。

## 设备与账号绑定

- 每个应用安装生成独立 UUID 与 256 位随机安装密钥，保存在应用私有、排除备份的文件中。
  不采集硬件 ID、广告 ID、IMEI、用户自定义设备名称或 APNs 原始 token。
- 请求先通过认证初始化安装记录，再绑定 FCM 注册标识（FID）。初始化不保存可推送 FID，
  客户端在初始化返回后重新检查当前账号和版本，避免退出期间首次注册的竞态。
- 服务端用 `verifyIdToken(token, true)` 校验会话，UID 只从认证结果获取。
  记录包含 UID、本应用安装 ID、FCM FID、FID 哈希、平台、应用 ID/版本、系统版本、
  设备型号、语言、通知开关、递增 revision，以及服务端生成的时间戳。
- 同一个 FID 同时只属于一个安装；换账号更新同一安装的 UID，FID 变化时原子移除旧索引。
  安装密钥仅保存 SHA-256 哈希，API 不返回 FID 或密钥，不记录原始请求和 provider 错误对象。
- revision 在发送前持久化。旧注册、旧解绑和旧 FID 发送失败均不能覆盖较新的绑定。
  同版本的相同请求可重试；同版本的不同账号/载荷被拒绝。
- 退出后保留待解绑标记，使用安装密钥重试解绑，无须保留用户的 Firebase ID token。
  未知安装的匿名解绑不创建文档；已有安装解绑后保留墓碑，阻止旧请求恢复绑定。
  Android 用 WorkManager 网络约束/退避恢复；iOS 在网络恢复、启动和前台重试。
- Android 接收 data 消息，在显示前检查 UID、开关和消息有效期；退出或切换账号清除已显示推送。
  iOS 使用 APNs alert，前台检查 UID/开关/有效期；退出注销 APNs 并清除已显示推送。
  iOS 后台 alert 由系统显示，服务端解绑需要网络，已经提交给 APNs 的通知无法保证撤回；
  不应在锁屏推送中放置敏感会话内容。点击远程通知仅打开应用，不接受远程 cron/会话操作。

## 有效期

| 对象 | 策略 |
| --- | --- |
| 设备绑定 | 服务端最近成功注册后 30 天过期 |
| 续期 | 登录、FID 更新、进入前台、通知设置变化；Android 另有每 7 天网络约束维护任务 |
| 退出墓碑 | 保留 30 天，阻止乱序请求恢复绑定 |
| 推送消息 | 默认 3600 秒，允许 1–86400 秒，同时不超过设备剩余有效期 |
| 物理清理 | Firestore TTL 加每天一次的有界清理任务；过期记录即使尚未物理删除也不参与发送 |
| 无效注册 | 明确 `registration-token-not-registered` / `invalid-registration-token` 时撤销对应旧快照；参数错误、凭据错误和临时故障不删除 |

30 天是应用的绑定租约，不是对 FCM 注册本身寿命的假设。Android / APNs 均设置消息过期时间。
官方参考：[注册管理](https://firebase.google.com/docs/cloud-messaging/manage-tokens)、
[消息有效期](https://firebase.google.com/docs/cloud-messaging/customize-messages/setting-message-lifespan)。

## API

基础地址：`https://us-central1-lingxi-5e4b2.cloudfunctions.net/pushApi`。

- `POST /v1/push/installations/{uuid}/initialize`：Firebase Bearer 认证，body 为 `installationSecret`。
- `PUT /v1/push/installations/{uuid}`：Firebase Bearer 认证，body 为上述设备字段、`installationSecret`、`revision`。
- `DELETE /v1/push/installations/{uuid}`：安装密钥授权，body 为 `installationSecret`、`revision`。
- `POST /v1/admin/push/messages`：认证、已验证邮箱及独立 `push_admin: true` claim；body 为
  `{ "uid": "目标用户UID", "title": "标题", "body": "内容", "ttlSeconds": 3600 }`。
  返回发送成功/失败/明确无效注册数量；不返回 FID。发送权限不继承模型目录管理员权限。

## 部署及验证

```sh
bash apps/firebase/scripts/setup-push.sh
firebase deploy --only functions:backend:pushApi,functions:backend:prunePushRegistrations,firestore:push-notifications --project lingxi-5e4b2 --non-interactive
```

独立数据库 `push-notifications` 只允许 Admin SDK 访问。`push-api` 服务账号的 Firestore
权限限制到该数据库，另外具有 Firebase Auth 只读和 FCM 消息发送权限，不生成服务账号私钥。
安装两个移动 SDK 后重新生成 iOS 工程。现有 XCTest 夹具脚本需要 Python 3.11+，
XcodeGen 会在生成时展开脚本中的 PATH，应先选择正确的 Python：

```sh
cd apps/ios/native
PATH="/opt/homebrew/opt/python@3.13/libexec/bin:$PATH" xcodegen generate
```

iOS 的 `aps-environment` 对 Debug 为 development、Release 为 production；Firebase Console
需为两个 bundle ID 配置 Flare 团队 APNs authentication key / certificate。
私钥、签名配置和设备 token 均不得提交到仓库。

本地：`npm --prefix apps/firebase test`；设置 `FIRESTORE_EMULATOR_HOST` 可运行真实事务回归。
移动端分别运行 `PushMessagePolicyTest` 和 `PushRegistrationTests`。
真机验收需要：登录并授权通知，确认绑定记录与当前 UID/应用 ID 一致，发送测试消息，
FID 更新、换账号、离线退出后恢复网络、过期消息均按上表处理；iOS 需检查后台/锁屏收取。
编译和 Emulator 测试不能替代这些真机及 APNs 验收。

当前本地证据：后端 14 项测试全部通过（含真实 Firestore Emulator 事务）；
Android Play/Direct Kotlin 编译及 3 项推送策略测试通过；iOS arm64 模拟器应用编译、
链接及 `PushRegistrationTests` 的 2 项 XCTest 通过。

2026-10-16 用户明确授权后，已创建并部署生产数据库、运行服务账号、限定数据库的 IAM、
两项 Functions、拒绝客户端访问的 Rules 及两项 `expiresAt` TTL（状态 ACTIVE）。
`pushApi` 健康检查成功，未登录注册请求返回 401；每日清理任务已手动触发并执行成功。

Samsung SM-S9310 的 Play Debug 更新已保留数据安装。真实登录会话自动注册成功，
云端记录的 UID、安装 UUID、应用 ID、设备型号、通知开关和 30 天租约已核对。
向该单一安装发送 60 秒测试消息后，Android 系统确实创建 `lingxi_push` 通知，
剩余超时约 58 秒，到期后从活动通知列表移除。此处发送使用操作员的短期 Google 凭据，
未给现有用户授予 `push_admin`，管理发送 API 的真实账号调用尚未验收。
2026-10-16（America/Los_Angeles）在同一 Samsung 真机补充完成账号与离线回归：

- 通过真实 Firebase Auth SDK，从原账号 A 切换到本轮临时邮箱账号 B；安装 UUID 和 FCM
  注册保持一致，云端 UID 与 revision 更新为 B，唯一注册索引保持正确。
- 发给 A 的消息实际到达手机后被接收策略拒绝；发给 B 的消息正常进入系统通知列表。
- 关闭手机 Wi-Fi / 移动数据，以设备实际 HTTPS 请求失败确认离线。VPN 会保留
  `NET_CAPABILITY_VALIDATED`，因此没有仅凭系统网络标志判断断网。
- 离线退出后 Auth 会话为空，已显示推送被清除，待解绑 revision 和标记已持久化；
  此时云端仍保留 B 的旧绑定，证明退出操作确实发生在离线状态。
- 恢复原网络设置后，WorkManager 自动使用安装密钥完成解绑；云端保留无 UID / FID 的墓碑，
  并删除旧 FCM 唯一索引。离线期间发送的消息在重连后实际到达手机并被拒绝。
- 删除临时账号，成对恢复原 SDK 加密 Auth 缓存与 crypto 状态，重启产品应用；
  原账号 A 自动重新绑定并实际收到推送。原账号、业务数据、安装身份和网络设置均已恢复。

最终仪器测试和全部 11 项场景检查通过。SDK 注册刷新仅用于此前的基线恢复，
最终断网重连验收没有手动重启 Google 服务或刷新 FCM 注册。
本轮新增调试事件仅在 Debug 且带合法随机验证 UUID 时记录接受/拒绝结果，不记录 UID、
FID、安装密钥、凭据或消息内容。Play 的 3 项推送策略测试和 Direct Kotlin 编译再次通过。
FID 轮换仍未做真机验收，已有 Emulator 事务测试覆盖。

iOS Full 真机签名构建及保留数据安装成功；APNs 密钥配置由用户自行处理，之后继续后台/锁屏收取验收。
本次未创建、修改或删除 Flare 团队现有 APNs 密钥。

操作员可在 `apps/firebase` 目录运行以下脚本复查绑定元数据和唯一注册索引：

```sh
node scripts/verify-push-live.mjs
node scripts/verify-push-live.mjs <安装UUID> --send
```

脚本固定目标项目，使用当前 `gcloud` 账号的短期登录；多账号时可设置 `PUSH_VERIFY_ACCOUNT`。
不会输出或保存 FID、
安装密钥或访问令牌。`--send` 强制指定一个安装，只发送 60 秒有效的验证通知。

账号与离线场景可用 `PushAccountDeviceTest` 和 `verify-push-accounts.mjs` 重跑。先构建并保留数据
安装 Play Debug 与对应 AndroidTest APK，再在 `apps/firebase` 运行：

```sh
ADB=/path/to/Android/sdk/platform-tools/adb \
PUSH_VERIFY_ACCOUNT=operator@example.com \
node scripts/verify-push-accounts.mjs <USB设备序列号> ../../backups/fcm-device/report.json
```

输出目录需预先创建。测试必须显式选择生产项目，默认测试套件会跳过；它创建并清理本轮
临时账号，暂时关闭并恢复手机网络，最后停止测试进程、恢复原加密会话并重启产品应用。
在报告旁生成的 `.auth-cache-backup.json` 以 0600 权限保存 SDK 不透明加密缓存和关联 crypto
状态，作为本机恢复点，不解析或输出凭据，必须放在忽略目录中且不得提交或分享。
测试不会回退推送安装密钥或 revision，避免恢复会话时覆盖较新的解绑状态。
