# 桌面 · Electron 宿主

归属于会话的 Bridge sidecar、renderer IPC、事件重放、原生服务与签名打包。

<a id="desktop-ownership"></a>

## 装配与归属

Electron main 是桌面装配宿主。SessionRuntimeManager 拥有各会话的 SessionRuntime，每个对象携带项目上下文、Bridge 连接、可选子进程归属、生命周期 generation 及待处理交互。Renderer 组件消费 preload API 与共享 DTO，不直接启动 Rust sidecar 或读取其 lockfile token。

Rust bridge-server 解析产品启动状态，并将 ClientCommand 路由到 Harness desktop 装配。Provider 通信仍由 LLM Client 负责；原生设备音频、账号／凭证服务、文件系统选择及窗口由宿主负责。浏览器文档页面是独立应用，不共享此 Electron 进程内传输。

![Renderer IPC 进入 Electron main，再认证其 Bridge sidecar；移动端使用 UniFFI。](../assets/platform-host-transports.svg)

<a id="desktop-flow"></a>

## 启动与 renderer 挂接

打包构建仅从 process.resourcesPath/bin 解析 bridge-server。开发环境可使用明确的 serverBin 或 LINGXI_BRIDGE_SERVER_BIN，再搜索工作区 target/debug 与 target/release，避免打包应用静默采用开发可执行文件。复用进程前检查存活进程身份、私有且归属正确的 lockfile，以及兼容的启动身份。

Preload 为 renderer 挂接保留有序号的运行时重放缓冲。会话 hydration 与实时交付须共享 source generation 及会话身份，避免旧恢复结果覆盖当前对话。断开／重启会拒绝待关联操作并清除旧交互；销毁时宿主区分自有进程与采用的外部进程。

1. 解析公开设置、凭证引用、活动项目与会话。
2. 根据宿主启动身份及明确发现文件解析或启动 sidecar。
3. 连接 Bridge、校验 hello，安装权限、问卷、事件与音频处理器。
4. 恢复持久 transcript，挂接有序重放，再渲染后续实时事件。

<a id="desktop-ipc"></a>

## IPC、原生服务与凭证

BrowserWindow 开启 sandbox 与 contextIsolation。Preload 暴露明确的命令及订阅方法；main 在转发前校验发送方、会话引用、命令变体与载荷。新增运行时命令需要匹配的 Rust／TypeScript DTO、main 准入、preload 类型及 renderer 归属；仅有 renderer 类型值不能证明准入。

Main 在启动时通过安全宿主服务解析 provider 凭证，仅发送选定 engine 所需的值。诊断使用 sanitizeDiagnostic 与有上限缓冲。openai_oauth_updated 作为凭证事件处理，并排除在 Bridge 迭代器重放之外。macOS 的已签名 Credential Broker 校验打包身份，因此未打包 npm dev 不适合证明安全凭证持久化。

| 边界 | 所负责行为 |
| --- | --- |
| Renderer → preload/main | 类型化 IPC、可信发送方与会话校验。 |
| Main → Bridge | Lockfile 认证、hello、生命周期及 broker 响应。 |
| Main → 原生服务 | 音频租约、系统弹窗、凭证存储与桌面窗口。 |
| Bridge → Harness | 会话／回合执行、权限门控、工具及持久历史。 |

<a id="desktop-development"></a>

## 开发、打包与故障诊断

在 Electron 前构建私有 Bridge 客户端，确保 file 依赖含有匹配的 dist 导出。源码／类型检查成功验证 JavaScript 集成；sidecar 与包验证还检查资源布局、签名及实际进程启动。macOS 发布路径使用 package:mac:flare，保留源码路径映射、由内到外签名，并使用已配置 Flare team 验证打包。

按边界排查：打包二进制缺失对应资源；HTTP 401 对应发现 token；hello 拒绝对应协议／构建配对；等待问卷／权限对应 broker 状态；旧 UI 对应重放 generation；macOS 凭证拒绝对应已签名应用／broker 身份。Renderer 专项测试不能验证系统授权、音频设备或 Windows／Linux 打包。

### 桌面源码检查

```bash
cargo build --locked -p bridge-server --bin bridge-server
npm --prefix packages/bridge-client ci
npm --prefix packages/bridge-client run build
npm --prefix apps/electron ci
npm --prefix apps/electron run typecheck
npm --prefix apps/electron test
npm --prefix apps/electron run build
```

验证方式: 需环境验证

### macOS 签名打包流程

```bash
cd apps/electron
npm run package:mac:flare -- --check
npm run package:mac:flare
npm run verify:package
```

验证方式: 需环境验证

- [Bridge 传输详情](bridge-client.md)
- [构建矩阵](platform-builds.md)

## 源码与文档基线

[源码仓库](https://github.com/lingxi-coder/lingxi-app/tree/main/apps/electron/src/main/bridge.ts)

- [lingxi-app/apps/electron/src/main/index.ts](https://github.com/lingxi-coder/lingxi-app/blob/ed180d70d6dc36d7bed12c654620d0bccb231883/apps/electron/src/main/index.ts) — 已提交源码 (ed180d70d6dc36d7bed12c654620d0bccb231883)
- [lingxi-app/apps/electron/src/main/bridge.ts](https://github.com/lingxi-coder/lingxi-app/blob/ed180d70d6dc36d7bed12c654620d0bccb231883/apps/electron/src/main/bridge.ts) — 已提交源码 (ed180d70d6dc36d7bed12c654620d0bccb231883)
- [lingxi-app/apps/electron/src/main/bridgeDiscovery.ts](https://github.com/lingxi-coder/lingxi-app/blob/ed180d70d6dc36d7bed12c654620d0bccb231883/apps/electron/src/main/bridgeDiscovery.ts) — 已提交源码 (ed180d70d6dc36d7bed12c654620d0bccb231883)
- [lingxi-app/apps/electron/src/main/validation.ts](https://github.com/lingxi-coder/lingxi-app/blob/ed180d70d6dc36d7bed12c654620d0bccb231883/apps/electron/src/main/validation.ts) — 已提交源码 (ed180d70d6dc36d7bed12c654620d0bccb231883)
- [lingxi-app/apps/electron/src/main/host-utils.ts](https://github.com/lingxi-coder/lingxi-app/blob/ed180d70d6dc36d7bed12c654620d0bccb231883/apps/electron/src/main/host-utils.ts) — 已提交源码 (ed180d70d6dc36d7bed12c654620d0bccb231883)
- [lingxi-app/apps/electron/src/preload/index.ts](https://github.com/lingxi-coder/lingxi-app/blob/ed180d70d6dc36d7bed12c654620d0bccb231883/apps/electron/src/preload/index.ts) — 已提交源码 (ed180d70d6dc36d7bed12c654620d0bccb231883)
- [lingxi-app/apps/electron/src/preload/event-replay.ts](https://github.com/lingxi-coder/lingxi-app/blob/ed180d70d6dc36d7bed12c654620d0bccb231883/apps/electron/src/preload/event-replay.ts) — 已提交源码 (ed180d70d6dc36d7bed12c654620d0bccb231883)
- [lingxi-app/apps/electron/src/renderer/bridge/useBridge.ts](https://github.com/lingxi-coder/lingxi-app/blob/ed180d70d6dc36d7bed12c654620d0bccb231883/apps/electron/src/renderer/bridge/useBridge.ts) — 已提交源码 (ed180d70d6dc36d7bed12c654620d0bccb231883)
- [lingxi-app/apps/electron/scripts/package-macos-flare.sh](https://github.com/lingxi-coder/lingxi-app/blob/ed180d70d6dc36d7bed12c654620d0bccb231883/apps/electron/scripts/package-macos-flare.sh) — 已提交源码 (ed180d70d6dc36d7bed12c654620d0bccb231883)
- [lingxi-app/apps/electron/package.json](https://github.com/lingxi-coder/lingxi-app/blob/ed180d70d6dc36d7bed12c654620d0bccb231883/apps/electron/package.json) — 已提交源码 (ed180d70d6dc36d7bed12c654620d0bccb231883)

- lingxi-app: HEAD ed180d70d6dc36d7bed12c654620d0bccb231883; 所据文件指纹 sha256:ef51a25cc818084d97b831957cb840fd84e1a4666089524025c7544ee886acda

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
