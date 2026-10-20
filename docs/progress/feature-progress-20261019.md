# LingXi 功能进度与工作检查点报告

日期：2026-10-19（America/Los_Angeles）。范围：三仓主工作区、旧内嵌 SDK 副本、此前列出的两个对齐集成工作区。

## 当前结论

Claude Code 对齐目标固定为 2.1.295。最终 R413 矩阵为 42 项、0 项开放；证据包含字节、约束、源码语义和批准差异，不能理解为所有输出全部逐字节相同。后续 Desktop 修复已通过各自验证并推送远端 main，最近一次为 Product `7ee78d4583d8d4afce336d2b39c31c260dc21be2`。

本次保存全部现有源码、测试、文档和未跟踪验收资料，形成 6 个检查点提交，涉及 751 个路径。没有重写实现，也没有合并或推送这些检查点。旧工作区基线落后于远端 main，一些脏文件与 main 内容相同，另一些还缺少 main 的后续改动；不能将检查点整树覆盖 main，也不能把保存完成记为新功能验收完成。

## 功能状态

| 功能 | 已有实现与证据 | 状态及剩余边界 |
| --- | --- | --- |
| 固定 2.1.295 运行时与 Mods 对齐 | R413 42 行矩阵已关闭；保留多 provider、品牌、Fusion 等既定差异 | 已交付；不追更 Claude Code；Remote 定制与其他明确排除项独立处理 |
| DeepSeek 独立 Search / Fetch、Responses 搜索 | SDK、Harness adapter、工具结果与错误分类已接通，已有聚焦测试记录，相关成果部分已在远端 main | 本次保存旧工作区残留；Harness 检查点仍含 `../llm-client` 本地 patch，发布前需基于最新 main 重基并恢复确定 SHA；本次未重新进行真实付费 API 验收 |
| 模型路由及音频元数据探测 | provider 限定模型引用；可选音频绑定缺失不再阻断文本会话 | 已交付；Product 本次 router 相对 main 的差异仅剩注释 |
| Desktop Crashlytics | 桌面接入、打包、符号与隔离测试报告验证已有记录 | 已交付；历史 V8 IPC 反序列化 OOM 没有 channel/body，无法断言唯一历史根因 |
| Mod UI IPC 内存积压 | 有界并发、队列、请求体预算与失效消息处理 | 已交付；长时间稳定性与高负载观察仍可继续，当前验证有界 |
| Weather 与 Replay 生命周期 | 独立站点共存、连接门控、刷新、翻页与关闭；维护版插件 0.1.1 进入版本控制 | 已交付并有真实签名包 GUI 证据；关闭 Replay 保留 Weather |
| 长会话 Mod UI 超限 | 在 IPC 前按 Host 限制检查；超限站点保留完整原生内容，释放旧 revision，停止无效重试 | 已交付 main `7ee78d458`；29 项相关测试、1 项 mounted Electron 回归、5 项维护 Mod 测试、8 项 smoke 合约及 TS 通过；签名包静态、标准冒烟和真实长会话验收通过 |
| iOS 模型设置简化 | 主页面保留默认模型与 provider，把路由、重试和服务配置移入 Advanced；翻译与 UI 测试同步 | 已保存；本次未重新编译或执行设备/UI 测试，不记为本次验收通过 |
| 文件读取状态恢复 | 新增冷/热恢复、Write/Edit 历史重建、共享 map 接入及外部内容变化校验，含 4 项新增测试定义 | 已保存；需与最新 main 三方整合后执行相关测试，不因存在测试源码就记为通过 |
| WebRTC 与移动端采集 | ICE 配置、UDP TURN relay、RTP/媒体通道、iOS native/Broadcast、Android thermal policy 和 probe；含 vendored 源码 | 已保存；文档记录 host forced-TURN 双向数据/RTP、策略测试和部分设备实验。它们不是本次检查点全量验收；NAT/手机互通、TURN TCP/TLS、relay 长期刷新、真机热状态与能耗仍有待验收范围 |
| 历史验收资料和 Local App 草稿 | 候选补丁、pre/postimage、日志、metadata、验收 JSON 和 `local_app.rs` | 已独立归档；归档不表示旧候选重新生效或 Local Apps 已新增验收 |

## 本次提交清单

| 工作区 | 提交 | 路径数 |
| --- | --- | --- |
| `llm-client` | `ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b` | 15 |
| `harness-runtime/deps/llm-client` | `34b64ad36d1933b35b97e5e13f419d743d030c85` | 126 |
| `harness-runtime` | `19a467080aa757bff58b0b18d49706688f5bdf15` | 215 |
| `lingxi-app` | `89107706cbd1a0dbf2336168b2fd7f76a21d3925` | 33 |
| `.worktrees/claude-alignment-integration/harness-runtime` | `8f63b88a3b06b9c8a818908c9973980319b2be4f` | 252 |
| `.worktrees/claude-alignment-integration/lingxi-app` | `059f09dea52fbf535e5453e74055e1bf6ed35e86` | 110 |

报告本身以单独的 Product 文档提交保存，提交 SHA 见 Git 历史及本地交付回执。

## 本次执行的检查

- 记录提交前 HEAD、分支、status、候选路径与 SHA-256，逐文件复核后暂存，未修改原有实现内容。
- 三仓主工作区原有 diff 的空白检查通过。没有执行新的全量编译、测试、打包或设备验收，不能继承所有旧结果作为这些检查点已通过的证明。
- 候选文件凭据模式检查仅命中已交付的 Firebase 客户端公开配置。没有发现该检查覆盖的私钥或 token 模式；这不是全面安全审计。
- 仅保存当前文件并移动相应本地分支 HEAD；保留提交前恢复引用 `refs/checkpoints/all-changes-20261019-before`。

## 保留但不提交的内容

- `harness-runtime/target-scratch-rootedfs/`：约 152 MiB 可重建 Cargo 输出，保留磁盘内容，不纳入 Git。
- `harness-runtime/deps/llm-client/`：独立 Git 仓库，内部 126 个路径已单独提交。父仓不新增不完整的 gitlink 或 `.gitmodules`，因此父仓仍可能显示 `deps/` 未跟踪。
- Git 已忽略的构建产物、运行数据及依赖缓存不纳入本次源码提交。

## 下一步

1. 在最新 main 上按差异整合 iOS 设置、文件恢复和 WebRTC 的真实新增部分，排除已交付重复内容与旧基线缺失，禁止整树覆盖。
2. 关闭本地 SDK patch，形成固定 SDK → Harness → Product 提交关系；执行对应生产者/消费者门禁后再决定发布。
3. 对本次保存的 iOS UI、文件恢复和 WebRTC 新差异执行聚焦验证并修复失败；本轮不执行 main 推送。
4. 按需要补充桌面长时间稳定性、移动端实机安装启动、macOS 公证/跨机器 Gatekeeper、指针拖拽排序等未实测边界。

本地检查点证据：`.buildcache/all-changes-checkpoint-20261019/` 下的 `inventory.json`、`commits.json`、`sensitive-scan.json`、`verification.json`。最近 Desktop 验收：`.buildcache/mod-ui-input-bounds-20261019/`；完整交付记录位于 Product 已交付提交 `7ee78d458` 的 `docs/mods/desktop-delivery-20261019.md`。

Raw HTTP 验收记录使用局部 `.gitattributes` 禁用换行转换，保留 CRLF 原始字节；该归档策略文件不计入原有 751 个路径。
