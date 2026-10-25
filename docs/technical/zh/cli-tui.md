# CLI、TUI 与无界面宿主

使用产品命令行、区分终端与 print 模式，并在保留运行时准入的前提下集成确定输出。

<a id="cli-assembly"></a>

## 宿主与运行时边界

cli 包构建 lingxi-cli。apps/cli/host 负责 argv／环境解析、启动、stdout／stderr、信号及终端切换；apps/cli/tui 渲染交互终端，tui-core 共享 UI 状态。初始化产生 Harness DesktopConfig，并将 Agent 装配交给 desktop facade。CLI 与 Electron 是同一运行时分层上的不同宿主。

可复用应用应依赖 Harness 公共 facade，不应导入产品 CLI crate。产品 headless 执行仍接收 DesktopHostServices 实现，提供 OAuth 描述、环境更新、启动身份及 runtime-ready hooks。选择输出格式不能替代 provider 凭证、项目校验、权限策略或持久会话建立。

<a id="cli-mode-selection"></a>

## 模式选择与输出契约

decide_mode 对非空 prompt 选择 Print；否则 --no-tui 或 stdin／stdout 非终端时选择 StdioRepl，完整终端选择 Tui。启动与 argv 门控进一步限定有效组合。自动单次运行应明确使用 -p。Print 专用控制包括输出／输入格式、预算／回合限制、结构化 schema 与 fallback model，不能据此推断交互行为。

text 输出面向人的结果，json 返回单个结构化结果，stream-json 为程序消费者提供换行分隔的实时输入／输出。保留记录分帧，将诊断 stderr 与协议 stdout 分离。stream-json 形状属于 Harness headless 协议；Bridge 使用 WebSocket 上的 Frame，原生客户端使用 UniFFI，消费者须选择一个传输契约。

| 模式 | 用途 | 输出 |
| --- | --- | --- |
| Tui | stdin／stdout 均为 TTY 的交互终端。 | 终端 UI 与直接交互。 |
| StdioRepl | --no-tui 或非 TTY 回退。 | 按行会话。 |
| Print | 单次提示、自动化与脚本。 | text / json / stream-json。 |

<a id="cli-permission-and-session"></a>

## 权限、会话与取消行为

运行时初始化先加载允许的配置来源，并在执行前解析模型／provider 状态。明确 --model 优先于环境与持久选择。Print 模式开启 deny_unresolved_ask，避免无人值守执行停在用户权限提示；--permission-prompts none 采用相同无弹窗策略。预批准规则仍通过运行时权限机制处理。

Resume 通过共享会话生命周期恢复持久状态，仅有 UI 行状态并不足够。Ctrl+C 取消归属回合工作，并使用产品信号／清理路径。TUI 排队提示与 slash 操作携带提交顺序和 source generation，避免取消或会话重置后旧结果发布到替换会话。后台会话 attach 是独立于 shell 进程输出的产品生命周期。

### 从产品根目录执行 CLI 示例

```bash
cargo build --locked -p cli --bin lingxi-cli
./target/debug/lingxi-cli --help
./target/debug/lingxi-cli -p "Explain the project structure" --output-format json
./target/debug/lingxi-cli -p "Summarize the selected files" --output-format stream-json --verbose
./target/debug/lingxi-cli --no-tui
```

验证方式: 需环境验证

提示运行需要已配置 provider，可能调用真实模型。--help 不调用模型即可验证 CLI 接口。这些示例不启用权限绕过。

<a id="cli-diagnostics"></a>

## 退出状态与验证

产品退出常量为成功 0、运行时／argv 错误 1、SIGINT 130；保留的未实现路径使用 64。自动化须同时检查进程状态与最终结构化记录，因为参数校验失败可能不产生回合输出。不能把非零 provider 结果重新解析成成功 JSON 响应来消除故障。

运行 CLI mode／argv 与宿主测试，验证分派及 headless 行为，再使用确定的 fake-provider 检查序列化。真实 provider 取消、不同 shell 的终端恢复及系统进程树清理需要各自运行验收，不能把 parser／单元套件称为完整终端或模型对齐证据。

### CLI 专项检查

```bash
cargo test --locked -p cli --lib
cargo check --locked -p tui -p tui-core
```

验证方式: 需环境验证

- [Harness 接入](harness.md)
- [桌面宿主](desktop-host.md)

## 源码与文档基线

[源码仓库](https://github.com/lingxi-coder/lingxi-app/tree/main/apps/cli/host/src/entrypoint.rs)

- [lingxi-app/apps/cli/host/Cargo.toml](https://github.com/lingxi-coder/lingxi-app/blob/abad4d9603ce6a0c032ddf089f69f88e927b76c4/apps/cli/host/Cargo.toml) — 已提交源码 (abad4d9603ce6a0c032ddf089f69f88e927b76c4)
- [lingxi-app/apps/cli/host/src/entrypoint.rs](https://github.com/lingxi-coder/lingxi-app/blob/abad4d9603ce6a0c032ddf089f69f88e927b76c4/apps/cli/host/src/entrypoint.rs) — 已提交源码 (abad4d9603ce6a0c032ddf089f69f88e927b76c4)
- [lingxi-app/apps/cli/host/src/argv.rs](https://github.com/lingxi-coder/lingxi-app/blob/abad4d9603ce6a0c032ddf089f69f88e927b76c4/apps/cli/host/src/argv.rs) — 已提交源码 (abad4d9603ce6a0c032ddf089f69f88e927b76c4)
- [lingxi-app/apps/cli/host/src/init.rs](https://github.com/lingxi-coder/lingxi-app/blob/abad4d9603ce6a0c032ddf089f69f88e927b76c4/apps/cli/host/src/init.rs) — 已提交源码 (abad4d9603ce6a0c032ddf089f69f88e927b76c4)
- [lingxi-app/apps/cli/host/src/mode.rs](https://github.com/lingxi-coder/lingxi-app/blob/abad4d9603ce6a0c032ddf089f69f88e927b76c4/apps/cli/host/src/mode.rs) — 已提交源码 (abad4d9603ce6a0c032ddf089f69f88e927b76c4)
- [lingxi-app/apps/cli/host/src/headless_host.rs](https://github.com/lingxi-coder/lingxi-app/blob/abad4d9603ce6a0c032ddf089f69f88e927b76c4/apps/cli/host/src/headless_host.rs) — 已提交源码 (abad4d9603ce6a0c032ddf089f69f88e927b76c4)
- [lingxi-app/apps/cli/host/src/sigint.rs](https://github.com/lingxi-coder/lingxi-app/blob/abad4d9603ce6a0c032ddf089f69f88e927b76c4/apps/cli/host/src/sigint.rs) — 已提交源码 (abad4d9603ce6a0c032ddf089f69f88e927b76c4)
- [lingxi-app/apps/cli/host/src/exit_codes.rs](https://github.com/lingxi-coder/lingxi-app/blob/abad4d9603ce6a0c032ddf089f69f88e927b76c4/apps/cli/host/src/exit_codes.rs) — 已提交源码 (abad4d9603ce6a0c032ddf089f69f88e927b76c4)
- [lingxi-app/apps/cli/tui/src/lib.rs](https://github.com/lingxi-coder/lingxi-app/blob/abad4d9603ce6a0c032ddf089f69f88e927b76c4/apps/cli/tui/src/lib.rs) — 已提交源码 (abad4d9603ce6a0c032ddf089f69f88e927b76c4)
- [harness-runtime/crates/runtime/src/headless/mod.rs](https://github.com/lingxi-coder/harness-runtime/blob/a39da75f0985d61d90fffa255f7b4d90901b0e27/crates/runtime/src/headless/mod.rs) — 已提交源码 (a39da75f0985d61d90fffa255f7b4d90901b0e27)

- lingxi-app: HEAD abad4d9603ce6a0c032ddf089f69f88e927b76c4; 所据文件指纹 sha256:bbb3c48b6b5761f33d3d66eb68ccaf089f9fd5ffc104138cbe084fb24e3b4f5b
- harness-runtime: HEAD a39da75f0985d61d90fffa255f7b4d90901b0e27; 所据文件指纹 sha256:92c2108dc63215bc69ea9c11ea081795da883fdcb1c9c28b060aa9221a1b95a2; 产品固定版本 a39da75f0985d61d90fffa255f7b4d90901b0e27

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
