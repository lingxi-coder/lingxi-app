# 功能进度与 main 整合报告

日期：2026-10-19。Claude Code 运行时对齐目标保持 **2.1.295**。

## 整合结果

本轮以远端最新 main 为起点，逐项比较此前保存的检查点，只移入实际新增实现。原工作区和旧检查点均保留，没有将旧工作区整树覆盖 main。

| 仓库 | 处理结果 |
| --- | --- |
| llm-client | 主 SDK 的 15 个修改文件均与 main 相同，无需重复提交。继续固定 `e986d0841e5cc6b5151bdaeabca1c6dd156771df` |
| 旧内嵌 SDK | 126 个文件中 37 个已完全相同；其余包含被新版源码替代的旧 API、字符串载体与错误投影。三方预检后的 6 处残留已逐项核对，均为 main 已覆盖的测试/逻辑、重复 import 或旧 API，保留当前 main 的 JsonPayload、source diagnostic、模型路由及后续修复；旧检查点仅作恢复用途 |
| Harness | 文件读取恢复、Bash 读取状态和 DeepSeek Web 等成果已在 main，不重复导入。新增独立 WebRTC workspace；已合并推送 `f928124df1035eb861c1d5f056d977cef8756369` |
| Product | 保留现有 Desktop Crashlytics、模型路由、IPC、Weather/Replay 和超限修复；新增 iOS 模型设置简化。所有 Harness 依赖统一固定到 `f928124df1035eb861c1d5f056d977cef8756369` |
| 历史验收目录及 Local App 草稿 | 保留归档检查点；不把历史候选、源码副本或未接入的 `local_app.rs` 作为生产实现重放 |

Product 的 router 残留差异仅为已等价实现的注释，不另行导入。Harness 原工作区的 `../llm-client` 路径覆盖没有带入本轮交付；manifest、lockfile 和 Cargo metadata 保持 canonical Git URL 与完整 SHA。

## 功能进度

- **2.1.295 对齐**：此前 R413 的约定矩阵为 42 项、0 项开放；证据分为字节、约束、源码语义和批准差异，不代表全部输出无条件逐字节相同。本轮不追更、不重新进行大规模 Native 采集。
- **Desktop 稳定性与 Mods**：Crashlytics、有界 IPC、Weather/Replay 共存和刷新、长会话超限预检均已在 main。本轮完整保留这些实现；最近一次签名包和真实长会话验收见 `docs/mods/desktop-delivery-20261019.md`，不将旧包验收冒充本轮新包验证。
- **iOS 设置**：默认模型和 provider 留在主页面；路由、重试与服务配置移至 Advanced。同步翻译与 UI 回归，不增加旧 UI 或旧协议兼容分支。
- **iOS 会话恢复**：修复深链接工具身份映射中残留的旧八字段模式，直接使用当前五字段 `toolResult`。回归覆盖工具调用/结果的 transcript UUID 映射、重复恢复及普通文本不误映射。
- **WebRTC**：独立 workspace 包含真实 Rust DataChannel/RTP、音视频处理、UDP TURN、移动 FFI、Android/iOS probe 与 Broadcast 源码及必要的 vendor 补丁。它尚未接入 Product Remote 服务，也不构成 Claude Remote 对齐交付。

## 本轮验证

| 检查 | 结果 |
| --- | --- |
| WebRTC core / test-server / media / mobile Rust 回归 | 15 + 4 + 50 + 3 项通过 |
| 独立 localhost Coturn 两项真实回归 | 2 项通过：relay-only 二进制/音视频传输、20 次固定源端口重连释放 |
| Android 热策略回归 | 通过 |
| Android Java 与 arm64 JNI | Java 编译、NDK 语法检查通过；不等于 APK 或真机验收 |
| iOS 采集、probe 与 Broadcast | 当前 SDK 语法检查通过；不等于 probe app 链接或真机验收 |
| Product canonical 来源、锁文件及协议镜像 | 通过；所有生产者/消费者使用同一 Git 来源，28 项来源解析测试通过 |
| 当前 Product 的 iOS 绑定、原生库及 app | 当前源码绑定生成、固定 Harness SHA 的 arm64 simulator 原生库和 `build-for-testing` 全部通过 |
| iOS 恢复与设置界面 | iOS 26.3.1 / iPhone 17 Pro 专用模拟器：2 项恢复单测、3 项真实 UI 测试通过；覆盖浅色、深色、大字体、Advanced 路由和 provider 编辑往返，零失败；结果见 `ios-focused.xcresult` |

Harness 本轮的生产 `crates` Git tree 与原基线 `b9c2860e` 完全相同，tree SHA 均为 `5407db7a705a9a1305e4f0a6bac8710ab6e0b76c`。因此没有为未改动的 SDK、tokenizer 或 Desktop UI 重跑无关测试；Product 仍执行当前固定 SHA 的实际原生构建及来源校验。

初次 iOS 构建暴露复用的本地生成缓存属于旧 DTO；没有修改当前 Swift 消费者去兼容旧接口，改为从当前源码生成匹配绑定。当前绑定随后暴露并修复工具结果身份映射的一处旧模式；测试脚本通过仓库提供的 `LINGXI_PYTHON3` 设置选择 Python 3.11+。磁盘耗尽和 Docker EOF 导致的失败日志保留；清理本任务中间缓存并获用户授权正常重启 Docker 后，两项 TURN 回归完成。第三方 vendor 原始格式保留，格式和空白门禁针对自有源码。

## 剩余边界

- WebRTC 的生产 Remote 接入、手机互通、真实限制性 NAT、TURN TCP/TLS、长期 relay 刷新、设备热状态及能耗，仍是独立工作；本轮没有声称通过。
- 既有移动端构建、设备记录与本轮检查分别报告。当前改动不意味着所有 Android/iOS 实机流程已经重新验收。
- 桌面长时间稳定性、macOS 公证/跨机器 Gatekeeper 和指针拖拽排序等未实测边界，不因本轮合并而自动关闭。

## 保存与清理

提交前后的原工作区 HEAD、index、status 和 diff 持续核对，其他 session 的修改未被重置或吸收。已完成且合并推送的本轮 Harness 工作树和两条临时分支已删除。另已清理三个此前交付、干净且无占用的 Desktop 临时工作树及分支。

Product 本轮工作树、分支和可重建缓存按“验证通过、合并推送、确认无占用”的顺序清理；实际执行结果另存清理回执。原始检查点、源码、验收证据和需要保留的产物不删除。维护版 Mods 的注册源仍位于 `.worktrees/mod-band-coexist-delivery/plugins/desktop-mods`，该运行依赖尚在使用，不能当成闲置目录删除；其他 session 的工作树同样保留。

详细证据和交付/清理回执位于多仓工作区 `.buildcache/checkpoint-main-integration-20261019/`。WebRTC 当前范围的完整验证说明在 Harness `packages/webrtc/INTEGRATION_ACCEPTANCE_20261019.md`。
