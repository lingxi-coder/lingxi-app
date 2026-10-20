# LingXi 遗留任务接手清单

2026-10-19，America/Los_Angeles。用户指定接手 LingXi 遗留工作；**chat 项目与 Computer Use 暂不接手**。Claude Code 运行时目标保持 2.1.295，不增加旧协议兼容分支。Remote 保持 LingXi 自建服务和通信所有权。

## 基线与核查

- SDK：`e986d0841e5cc6b5151bdaeabca1c6dd156771df`。
- Harness：`f928124df1035eb861c1d5f056d977cef8756369`。
- Product：`bf24677c3e470b3acb06157495b54cdd9e288ae0`。
- 在两个隔离的 `codex/remaining-tasks-20261019` 分支推进，不重置原工作区。
- 已读取相关 session 的最近报告并核对 main 祖先。旧报告中的“未提交”不能直接视为当前未合并；Activity、Side Chat、模型路由、FCM、Crashlytics、Fusion、slash 提示等已有实现进入 main，不重新重放历史补丁。
- 本轮前的 iOS 设置构建与 5 项定向测试已通过。它们不替代真机、签名 Universal Link 或系统广播验收。

## 当前仍开放的交付

| 工作 | 当前证据与缺口 | 下一步 |
| --- | --- | --- |
| WebRTC iOS 系统广播 | 提前帧的无符号时间差下溢已修复，回归先失败后通过；当前优化静态库、Flare 签名 probe/extension、严格签名检查及真机安装/启动通过 | 等待手机系统选择器确认，然后验证实际跨 App 帧、停止与恢复 |
| WebRTC 产品接入与 Remote | 独立 transport/media/FFI 与 localhost TURN 已验证；生产产品没有接入 | 在媒体边界验证后接入现有 host/control-plane；认证、设备与权限仍归项目自有服务，不另造执行所有者 |
| WebRTC 其他媒体边界 | 限制性 NAT、TCP/TLS、手机互通、长期刷新、热状态与能耗尚无完整证据 | 逐项实施和验证，区分共享测试与真实设备结果 |
| 版本化模型目录四端刷新 | Firebase API、已发布目录及 Web 管理页已存在；当前文档明确尚未接入四端启动同步 | 闭合 SDK 官方目录定义、Harness 校验/缓存/刷新、Desktop/CLI/mobile 启动和当前选择器消费者；保留用户模型、连接和凭据 |
| iOS 真机界面与 Side Chat | 源码与模拟器回归已存在，旧 session 的真机检查因镜像/锁屏未完成 | 用匹配绑定和库的当前签名 App 验证键盘、独立草稿、隐藏重开与模型设置 |
| 深链接发行验收 | 本地实现、网页、AASA/direct 关联已发布；Play 真实 App Signing 指纹、签名 iOS Universal Link、Windows/Linux 实机仍开放 | 复用当前实现做平台验收；不能猜证书或把模拟器/脚本结果标为发行通过 |
| 移动 Crashlytics | Android JVM 测试云端入库已有记录；iOS 真机上报及 NDK 云端符号化未完成 | 准备匹配产物和隔离测试；云端上传遵守既有授权范围，不对正在使用的会话故意触发崩溃 |
| Desktop production 凭据稳定性 | 旧任务等待 production 通道保存凭据，未完成约定的 15 次刷新、3 次启动 | 先确认当前签名通道及凭据状态；不跨通道复制密钥，不记录明文凭据 |
| 其他验收边界 | macOS 公证/跨机器 Gatekeeper、长期稳定性等没有因 main 合并自动完成 | 只在对应环境与实际操作完成后关闭 |

Local App 原产品代码不是 Claude 对齐项。独立市场的旧“后端 503 未配置”报告已被后续 Firebase 部署替代；本轮实际读取当前市场 `/api/v1/health` 与 `/api/v1/capabilities`，均为 JSON / HTTP 200，可关闭那条旧阻塞。模型公共 API 实际返回 46 个提供商、1327 个模型，未登录管理接口返回 401；这证明后台可用，不代表四端启动刷新已经实现。生产服务变更和证书发布须保留具体可审内容与相应授权。

## 调度与证据

首个修复阶段的 Harness 已推送 main：`b054a3bfe094d8a588d76706ca90212d62381111`，远端一致、分歧 `0 0`。Product 的全部 Harness manifest/lock 来源同步固定到该提交；SDK 仍为 `e986d0841e5cc6b5151bdaeabca1c6dd156771df`。新旧 Harness 的生产 `crates` tree 均为 `5407db7a705a9a1305e4f0a6bac8710ab6e0b76c`，本批改动仅在独立 WebRTC 源码与文档；因此不将旧包重标为新构建，也不重复未受影响的 tokenizer/生产 FFI 全量构建。

源码与最小可复现优先，采集仅用于剩余语义疑点和关键路径。一次只运行必要的重量构建，避免磁盘再次耗尽。每批验证通过后合并推送 main；只清理已交付且无占用的本任务分支、工作树与可重建缓存，保留仍作为 Mods 注册源的目录。

接手证据在工作区 `.buildcache/remaining-task-takeover-20261019/`：`task-inventory.json` 包含原 session 标题、报告及已核实的 main 祖先；每项新验证保留独立日志。该清单是继续推进的入口，不是“所有遗留任务已完成”的声明。
