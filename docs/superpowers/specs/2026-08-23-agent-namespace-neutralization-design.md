# 命名空间中性化（LINGXI / CLAUDE → AGENT）

> 本文的事实基础来自一轮 12 agent 的只读勘察（7 个 finder + 3 个对抗性校验 + 1 个完整性评审 + 1 个汇总，2.4M token / 658 次工具调用）。带 ✅ 的结论是在本会话中由代码复核过的；带 ⚠️ 的是单一来源、未经复核的**假设**，实施第一步必须重新导出。
>
> 一个 finder（byte-locks）中途因 API 错误终止，其覆盖面由本会话手工补测。

## Context

这个仓库的引擎能力将被用在多个 app 上，而当前这个 app 叫 LingXi。因此配置面、环境变量、文件名等一切**产品无关**的命名要中性化为 `agent` / `AGENT`。同时，引擎是 Claude Code 的 parity 移植，上一次 `.claude` → `.lingxi` 的重命名（2026-06-27）留下了大量未转换的 CLAUDE 命名。

**关键发现改变了问题的性质。** 上一次重命名是一次性的，结果是今天：

- **60 个 `CLAUDE_*` 环境变量仍在运行时被读取** ✅
- **77 处 live `.claude` 字符串字面量**在非注释代码行上 ✅
- 一条**自我修改安全护栏在盯着一个本产品不使用的目录**，而它的反向守卫恰好漏掉了唯一出问题的文件，所以一直是绿的 ✅

原因不是上次没做完。原因是**每一轮 parity 对齐都会重新带进 CLAUDE 的名字**。这不是一次性污渍，是持续渗漏。

因此本方案的产出**不是一次重命名**，而是：一次重命名 **加上一个不会退化的防漏门**。前者是一次性状态，后者是唯一能防止第三次的东西。

第二个改变问题性质的发现：这次改动的主要风险不是失败，是**伪成功**。20 组 lockstep 里大多数的失败模式是「编译干净、测试全绿、只有真机/真用户能发现」。两个已验证的例子：

- `tools/scripts/check_version.sh` **今天是红的**（exit 1，88 个文件不匹配，VERSION 钉在 `0.5.0` 而树已到 `0.12.0`）。它的循环是 `done < <(find lingxi-code -name Cargo.toml)`；目录改名后 `find` 找不到东西，循环体一次都不执行，脚本打印 `OK: all Cargo.toml files at 0.5.0` 并 **exit 0**。`set -euo pipefail` 不传播进程替换的失败。**一个红门会被目录改名这一个动作单独变绿。** ✅
- `git grep -cE '\bCLAUDE_CODE_ENTRYPOINT'` 返回**零行**、退出码 0；换成 `git grep -cP '(?<![A-Za-z0-9_])...'` 才有真实命中。git grep 的 ERE 不实现 `\b`，**静默返回空**。任何用 `\b` 得出的计数都是无效的。 ✅

---

## 1. 已锁定的决策

| # | 决策 | 含义 |
|---|---|---|
| **D1** | 命名空间中性化，产品身份做成**可注入** | 引擎/配置面全中性；`PRODUCT_NAME` 与 system-prompt 身份行由宿主注入，中性默认 `Agent` |
| **D2** | 混合绑定：**命名空间编译期固定，只有显示名运行期注入** | 命名空间是跨 app 的统一标准，不该每个 app 一套；`branding` 的命名空间常量保持 `const &str` |
| **D3** | **Clean break** | 不迁移、不双读、不设废弃窗口。`~/.lingxi`（30M / 236 session / 凭证）直接作废，重新认证 |
| **D4** | 源码树**全部中性化** | `lingxi-code/` → `agent-code/`、crate `lingxi-cli` → `agent-cli`、`memory/src/lingxi_md/` → `agent_md/`、CI `lingxi-release.yml` → `agent-release.yml`、CLI 二进制默认 `agent`、npm 包 `agent` |
| **D5** | `CLAUDE_*` **只留「外部写入」的入口契约** | 保留 `CLAUDE_AGENT_SDK_*`、`CLAUDE_CODE_ENTRYPOINT`、`CLAUDE_CODE_EXTRA_BODY`、`CLAUDE_CODE_EXTRA_METADATA`、`CLAUDE_CODE_OAUTH_TOKEN`（O1）。其余全改 `AGENT_*`。`ANTHROPIC_*` 不动 |
| **D6** | `AGENT_` 前缀保留，但**合成路径加白名单** | `env_parser` 不再把任意 `AGENT_*` 当设置；子进程转发从前缀匹配改成显式 key 列表 |
| **D7** | 设备端「名字即句柄」的标识符**全改**，能迁移的写迁移，不能的接受丢失 | 详见 §6 |

### 1.1 范围边界

**改：**
- `lingxi-code/` 全部
- `clients/` 的**命名空间面**：`~/.lingxi/bridge`、`X-LingXi-Ide-Authorization`、`LINGXI_*`（113 处 / 33 文件 ⚠️）
- `.github/workflows/`、`scripts/`、`tools/`、`skills/`、`npm/`
- 设备端「名字即句柄」标识符（§6）

**不改（用户明确划线）：**
- Android package `com.lingxi.code`、iOS bundle id `com.lingxi.code`（1335 处）
- `LingxiCode*.swift`、`LingXiAccessibilityService.kt` 等客户端类名与文件名（125 处）
- `docs/` 下 410 个历史 spec / plan / audit（16933 处）—— 它们是已发生事实的记录，重写会把历史改成假的，且夹着 666 处 oracle 出处引用
- **`灵犀`（483 处，全部在 `clients/`，引擎里 0 处 ✅）** —— 它是 LingXi 的中文名，属产品身份，按 D1 保留。它是 iOS 桌面图标名（`Info.plist:13`）、Android `app_name`、以及 4 个 iOS 系统权限弹窗文案；真源在 `clients/translations/*.json`

**不能改（协议层）：**
`anthropic` host/provider · 模型 ID（`claude-sonnet-*`、`us.anthropic.claude-*`）· `tengu_*` 遥测事件名 · beta 头 · `claude-cli` User-Agent · `ANTHROPIC_*` · `platform.claude.com` OAuth 端点 · `BedrockClaude*` / `VertexClaude*` / `FoundryClaude*` / `ClaudeAiOAuth*` 类型名 · 一切 `claude-code/src/...:line` 形式的 oracle 出处引用

---

## 2. 架构：三层切分

整个设计的骨架是把今天混在一起的「品牌」拆成三层，**每层的绑定时机和可变性都不同**。

| 层 | 内容 | 绑定时机 | 谁能改 |
|---|---|---|---|
| **L1 命名空间** | `.agent`、`AGENT.md`、`AGENT.local.md`、`~/.agent.json`、`AGENT_CONFIG_DIR`、`.agent-plugin`、`/etc/agent`、`Application Support/Agent`、`AGENT_` 前缀 | 编译期 `const &str` | **没人**。这是跨 app 的中性标准，下游 app 改它就是制造碎片 |
| **L2 产品身份** | `PRODUCT_NAME`、system-prompt 身份行、对外 UA / OTel service name / 帮助文档 URL base | 运行期宿主注入，默认 `Agent` | 宿主 app |
| **L3 冻结身份** | Anthropic 协议面 + 「名字即句柄」中不可迁移的那些 | 永不改 | 显式豁免清单（带理由），**防漏门读它** |

**L3 是这次新增的概念。** 上一次重命名没有它，所以「不能改」和「还没改」混在一起——这正是今天 60 个 `CLAUDE_*` 说不清死活的根本原因。

### 2.1 branding crate 的切分

`lingxi-code/branding` 保持**零依赖叶 crate**，只留 L1 编译期常量：

```rust
pub const DOT_DIR: &str = ".agent";
pub const GLOBAL_CONFIG_FILE: &str = ".agent.json";
pub const LEGACY_GLOBAL_CONFIG_FILE: &str = ".config.json";   // 已中性，不动
pub const CONFIG_DIR_ENV: &str = "AGENT_CONFIG_DIR";
pub const MEMORY_FILE: &str = "AGENT.md";
pub const MEMORY_LOCAL_FILE: &str = "AGENT.local.md";
pub const PLUGIN_MANIFEST_DIR: &str = ".agent-plugin";
pub const ENV_PREFIX: &str = "AGENT_";
pub const MANAGED_DIR_MACOS: &str = "/Library/Application Support/Agent";
pub const MANAGED_DIR_WINDOWS: &str = r"C:\Program Files\Agent";
pub const MANAGED_DIR_UNIX: &str = "/etc/agent";
pub fn config_home(home: &Path, config_dir_env: Option<OsString>) -> PathBuf;
```

翻转这 11 个值会**自动传播到 366 处调用点 / 88 个文件**（`DOT_DIR` 194/60、`PLUGIN_MANIFEST_DIR` 75/13、`CONFIG_DIR_ENV` 50/24、`MEMORY_FILE` 8/7、`GLOBAL_CONFIG_FILE` 5/4、`MANAGED_DIR_*` 各 5/5、`config_home` 3/3、`LEGACY_GLOBAL_CONFIG_FILE` 1/1、`MEMORY_LOCAL_FILE` 1/1）✅，这些调用点**零编辑**。

`PRODUCT_NAME` 从 `branding` **移出**到 `traits`，变成 `product_name()`。理由：注入需要 `tokio::task_local!`，不该把 tokio 拉进 88 个文件依赖的叶子 crate；而 `PRODUCT_NAME` 只有 14 个调用点 / 7 个文件 ✅，搬家成本远低于污染层级。

### 2.2 注入点：照抄既有形状

`traits/src/session_flags.rs:69-98` 已经是这个形状，且其注释明说它就是为了解决「一个进程里多个嵌入式运行时共存」：

```
进程级默认值  +  tokio::task_local! 覆盖  +  effective_*() 读取
```

`product_name()` 照抄。**不用 `OnceLock`** —— 它不能重设，会破坏多引擎同进程的场景，而 `session_flags` 已经用 task_local 解决过这个问题。

### 2.3 把 L1 的静默失败转成编译错误

`llm-client/src/prompt_format.rs:137` 今天做：

```rust
let Some(rest) = s.strip_prefix(&prefix_boundary) else {
    return vec![SystemBlock { text: s.to_string(), cache_control: org_cc() }];  // 单块
};
```

`prefix_boundary` 由该模块**私有的** `const HEADER`（`:22`）拼成，而装配方用的是 `orchestrator/src/prompt/locked_templates.rs:10` 的另一份同值常量。两者一旦漂移，`strip_prefix` 失配 → 走 else 分支 → 返回**单块** → **prompt cache 的前缀断点在每一个请求上静默消失**。不 panic、不报错、无测试。✅

**改造：**

```rust
pub fn split_system_blocks(s: &str, header: &str, enable_caching: bool) -> Vec<SystemBlock>
pub fn split_system_blocks_with(s: &str, header: &str, enable_caching: bool, opts: SplitOptions) -> Vec<SystemBlock>
```

splitter 不再持有 `const HEADER`，由装配方传入。今天的漂移是零信号，改完之后是**编译错误**。`orchestrator/src/prompt/mod.rs:244` 的 re-export 随之更新。

这是整个设计里性价比最高的一处改动。

### 2.4 D6 的两个口子

**(a) 设置合成路径。** `engine/src/settings/env_parser.rs:64` 用 `format!("{prefix}{suffix}")` **合成**设置名，`FIELD_MAP[0]` 是 `("MODEL","model",Scalar)`，而 `settings.model` 是活的模型选择输入（`apps/cli/src/init.rs:465,:480`）✅。翻成 `AGENT_` 后，**用户 shell 里任何别的 agent CLI 导出的 `AGENT_MODEL` 会静默接管本产品的模型选择**。唯一的守卫 `unrelated_env_vars_are_ignored`（`env_parser.rs:176-181`）只喂了 `PATH`/`HOME`，永远绿。

→ 改成**显式后缀白名单**：未知 `AGENT_*` 忽略，不进入设置。

**(b) 子进程转发。** `apps/cli/src/background_dispatch.rs:931-950` 的 `launch_env_key_allowed` 是**前缀**匹配（`key.starts_with(prefix)`），而 `HOOK_ENV_DENYLIST`（`platforms/posix/src/process/runner.rs:100-117`）是**精确 key** 清洗。翻转后第三方的 `AGENT_*` 密钥会被转发进后台子进程而不会被洗掉。

→ 只把数组里 `"LINGXI_"` 这**一个元素**换成显式 key 列表。**其余 18 个前缀元素冻结。**

> ⛔ **陷阱**：数组里 `"CLAUDE_CODE_"` 和 `"CLAUDE_AGENT_"` 两条看起来冗余。删掉 `CLAUDE_AGENT_` 正好切断 D5 保留的 SDK 契约入口（`"CLAUDE_AGENT_SDK_VERSION".starts_with("AGENT_")` 为 false）✅。这两条必须保留。

---

## 3. 执行策略：六段，顺序不可换

| # | 内容 | 为什么是这个位置 |
|---|---|---|
| **S0** | 建防漏门（CI + 单测）。此刻它是**红的** | 它吐出的违规清单**就是**穷尽账本——不是人 grep 出来的。后面每消一项门绿一格，进度可度量 |
| **S1** | 修 19 个既有 bug（§5）+ 补反向守卫的 needle + 新造 §4.2 的三处接缝 | 这些**与改名无关**，现在就是错的。放在翻转前修，才能区分「改名引入的红」和「本来就红」 |
| **S2** | 纯重构：散落字面量路由进 `branding`，**值仍是 lingxi** | 字节零变化 ⇒ 测试应全绿。**这个绿是「我没改行为」的证据** |
| **S3** | 翻转常量 + 重算 41 个含品牌字面量的字节锁（共 58 个字节锁文件 ✅） | 此时唯一的失败源是翻转本身 |
| **S4** | L2 注入 + 20 组 lockstep 逐组落地 + 设备端迁移（§6） | lockstep 必须**整组一个 commit**，拆开就是静默故障 |
| **S5** | `git mv`：`lingxi-code/` → `agent-code/`。**被移动的文件内容一个字节不改**；同一 commit 里**必须**同步更新树外指向旧路径的字面量 | 改动被移动文件的内容会让 git rename 检测失效，review 变成 2198 个「新增文件」。放最后，把与其他分支的路径冲突窗口压到最短 |

> ⛔ **S5 的「纯」是指被移动文件的内容不变，不是指 commit 里没有别的改动。** 树外有指着旧路径的字面量必须在**同一个 commit** 里更新，否则 CI 全红：`.github/workflows/` 里 30 行 `working-directory: lingxi-code` ✅、`tools/scripts/check_version.sh` 的 `find lingxi-code`、以及 §4.1 M4 列出的构建脚本。rename 检测按单文件内容相似度工作，同一 commit 里编辑**未被移动**的文件不影响它。
>
> `check_version.sh` 尤其要小心：它今天是红的，而目录改名会让它的 `find` 空转从而**翻绿**（见 Context）。它必须在 **S1** 就被修成能真正失败的形状，S5 只更新路径。

S2/S3 的两段式是上一次重命名验证过的手法：上次 173 个 fixture 失败正是靠 A/B 拆分才收敛的。

### 3.1 顺序硬约束（违反即静默错）

1. `LINGXI.local.md` **早于** `LINGXI.md`，否则 `.local` 变体先被吃掉再被后续 pass 二次改坏
2. `lingxi-cli` **早于**任何泛化 `lingxi` 替换，否则得到 `agent-cli` 满地跑，与 D4 的「二进制叫 `agent`」矛盾
3. `CLAUDE.md` **早于** `.claude`，否则 `.claude.json` 与 `CLAUDE.md` 分叉
4. `<MEMORY_DIR>` **早于** `<CWD>`（`parity_claude_2_1_220.rs:268` 归一化器自身的顺序约束，其注释 `:266-267` 有警告）
5. **全程禁用大小写不敏感替换。** `-i` 会打中 `.LingXiTheme`×36、`.LingXiCode`×4、`.LingXiNativeV1`×1，以及 `AndroidManifest.xml:21` 的 `.computeruse.LingXiAccessibilityService` —— 后者是 OS 注册组件，Android 按 `package/component` 记授权，改名会**静默吊销所有已安装机器上的 computer-use 授权**
6. 场景 5 的另一面：5 处**故意的大小写乱序**必须手工改，不能靠 sed —— `permission/src/filesystem.rs:1096 ".LiNgXi"`、`permission/src/auto_edit_safety.rs:660 ".lInGxI"`、`sandbox-runtime/src/path_utils.rs:588 ".lInGxi"`，以及 `filesystem.rs:1036` / `auto_edit_safety.rs:964` 的 `normalize_case_for_comparison(".LINGXI") == ".lingxi"`。大小写敏感的 sed 跳过它们 ⇒ 大小写敏感性测试不再测 dot-dir；大小写不敏感的 sed 把乱序压平 ⇒ 断言变成同义反复

### 3.2 这份 spec 对应几个实施计划

六个阶段不适合塞进一个计划文档。建议拆成两个：

- **计划 A（S0–S1）**：防漏门 + 19 个既有 bug + 反向守卫 needle + §4.2 的三处新接缝。**这一半完全不含改名**，可以独立评审、独立合并，且合并后仓库立刻比今天更安全（B3 那条安全护栏、B16 的 bridge 发现、B2 的重复读都是当下就存在的缺陷）
- **计划 B（S2–S5）**：重构 → 翻转 → lockstep 与设备迁移 → 树重命名

拆开的另一个好处：计划 A 落地后，**防漏门产出的违规清单才是计划 B 的真实任务列表**，而不是 §10 里那些待重新导出的数字。

---

## 4. 把静默耦合转成响亮耦合

逐组打补丁会漏。20 组 lockstep 按**为什么静默**归类，每类一种治法。

### 4.1 五种机制

#### M1 —— 同一个值被两处各持一份字面量（7 组）

`L1` HEADER（orchestrator ↔ llm-client）· `L7` bridge 路径（TS ↔ Rust）· `L8` auth 头（TS ↔ 4 处 Rust）· `L9` auto_mode 规则散文 ↔ 标签 · `L16` telemetry 值 ↔ 自身守卫 · `L18` `HOOK_ENV_DENYLIST` ↔ 写入方 · `L19` kill-switch 写端 ↔ 读端（跨 crate）

**治法：一处持有，其余接收。** 能变参数的变参数（L1，见 §2.3），能变导入的变导入。

- `L7` ✅：`clients/shared/src/lockfile.ts:35` 硬编码 `join(homedir(),'.lingxi','bridge')`，而 Rust 侧 `bridge/src/lockfile.rs:160-167` 走 `branding::CONFIG_DIR_ENV`/`DOT_DIR`。**翻转 branding 常量会零源码编辑地把服务端移到 `~/.agent/bridge`，而 TS 客户端继续扫 `~/.lingxi/bridge`，什么也发现不了。** TS 编译干净，无 Rust 测试覆盖。（附带：这一对**今天就已经坏了** —— 任何设置了 `LINGXI_CONFIG_DIR` 的用户，bridge 发现已经失效，见 B16）
- `L8` ✅：`clients/shared/src/client.ts:52 AUTH_HEADER_NAME` ↔ `bridge/src/mcp_endpoint.rs:25`、`platforms/common/src/mcp_sse.rs:24`、`mcp_ws.rs:31,:158-159`。头名在 proper-case 与 lowercase 两种拼法都存在（`from_static` 要求小写）
- `L16`/`L18` 是特例：**守卫和被守卫的值在同一个文件里，一次 sed 同时改掉两者，守卫就变成同义反复**。这两处的守卫 needle 必须从独立来源取。`L18` 的现有测试（`platforms/posix/tests/process_spawn_env_test.rs:100-141`）覆盖 17 个 key 里的 6 个，但它**从同一批字面量播种**，所以在任何方向上都不可能发现漂移；11 个 key 零覆盖 ✅

#### M2 —— 跨语言 / 跨构建的字符串契约，编译器管不到（8 组）

`L4` build.rs ↔ `option_env!` · `L5` `project.yml` ↔ `#if LINGXI_FULL` · `L6` 19 个 FFI 符号 · `L12` BGTask id · `L13` App Group · `L14` translations ↔ 生成物 · `L15` URL scheme · `L20` iOS bridge handler

**治法：单一真源 + 生成**（`L14` 已经是这个形状，照抄）；做不到的加**契约测试**。

- `L5` ✅ 最阴：Swift 把 `#if <未定义>` 当合法的 false。`clients/ios/project.yml:101-110` 定义 `LINGXI_FULL`，`Sources/LocalApps/LocalAppsRuntimeDistribution.swift:5` 消费它。改一边不改另一边 ⇒ `usesFullRuntime` 静默变 `false` ⇒ 引擎被告知设备没有 Node 工具链 ⇒ **每次本地应用构建都静默走 Store 路径**。无警告、无 Rust 影响、无测试。（`LINGXI_STORE` 有零个 `#if` 消费者，是死的）
- `L4` ✅ 三对 build.rs ↔ `option_env!`：`LINGXI_GIT_SHA_SHORT`、`LINGXI_COST_BUILD_EPOCH_SECS`、`LINGXI_MOBILE_LINUX_AUTHORIZATION_SHA256`。前两对失配时 `option_env!` 产出 `None`（空 git sha / 无 cost epoch）。第三对是**失败关闭**的（`apps/ios-framework/src/lib.rs:578` 用 `let Some(expected) = … else { return false; }`），这是好的设计——但 CI 里的 `|| ''` 默认值（`clients.yml:99-100,238-239`）把它变成一个**报绿的空操作**。那个 `|| ''` 要去掉。全仓库 `rerun-if-env-changed` 计数为 **0** ✅

#### M3 —— 主构建门看不见的目标（与 M2 重叠）

`cargo build --workspace` 不启用 `uniffi`，不编 iOS/Android。`L2`/`L6`/`L12`/`L13` 全在这个盲区里。

**治法：门必须跨语言扫源码**，不能只依赖编译。CI 增加 `--all-features` 与客户端目标。但这只是必要条件：`L12`（BGTask id 不匹配 → `register(forTaskWithIdentifier:)` 在启动时抛异常）只有**真机启动**才暴露。

#### M4 —— 文件路径 ↔ 代码里的路径字面量（3 组）

**治法：同一 commit `git mv` + 编译期存在性断言。**

- `L2`：`.lingxi/` 模板目录 + `lib/lingxi-bridge.js` + `lib/lingxi-provider.jsx` ↔ `apps/engine-mobile/src/local_apps_build.rs` 的 `embedded_template_file!` / `include_bytes!(concat!(…))` 字面量 ↔ `VITE_LOCKED_FILES` 路径清单。失配是编译错误——**但只在 `--all-features` 下**
- `L3` ✅ 是**静默数据丢失**：根 `.gitignore` ↔ `lingxi-code/.gitignore` ↔ `local-apps/tests/fixtures/.gitignore:8` 的 `!**/.lingxi/` 反选 ↔ `vite-react-static-v1/.gitignore:4`。反选停止匹配后 `git add -A` **暂存 4 个 fixture 删除并跳过新路径**，本地此刻一切正常；下一个 fresh clone 上 serde 兼容测试才炸。→ 需要一条「fresh clone 后 `git status` 必须干净」的测试
- `L17` ⚠️：`platforms/android-shellbin/build.rs:260` 引用 `"README.lingxi.md"`，而该文件在 `third_party/toybox/generated/`（**出范围**）。改字面量不改文件 ⇒ 构建脚本坏；改文件 ⇒ 违反范围边界。→ **需要写进 L3 显式豁免清单**

#### M5 —— wire key ↔ Rust 字段名（1 组）

`L10` ⚠️：`memory/src/lingxi_md/` 改名波及 `memory/src/lib.rs:12` 的 `pub mod`、163 处限定路径、5 个自由函数、`LingxiMd*` 类型、`lingxi_md_excludes` 字段，以及 serde `rename = "lingxiMdExcludes"`。**字段改了 wire key 没改 ⇒ 结构体永远读不到那个键，设置加载恒为 None，零编译错误。**

**治法：改名同时看 serde attr；加一条「用真实磁盘配置反序列化」的测试。**

> ⛔ `s/LingxiMd/AgentMd/` 会**过度匹配** ✅：同时命中 camelCase JSON 键 `hasLingxiMdExternalIncludes{Approved,WarningShown}`（14 处，是磁盘配置键）和 oracle 移植的文档注释符号 `getLingxiMds`（10）/ `isLingxiMdExcluded`（5）/ `getAdditionalDirectoriesForLingxiMd`（2）/ `omitLingxiMd`（1）。必须用 `-P` 配 `\bLingxiMd(Tier|Excluder|HierarchyWalkedPayload)\b` 隔离。

#### M6 —— 树外状态，仓库里防不住（2 组）

`L4` 第三项 GitHub repo variables（`vars.LINGXI_MOBILE_LINUX_ENABLED`、`vars.LINGXI_MOBILE_LINUX_AUTHORIZATION_SHA256`）· `L13` Apple provisioning profile（App Group `group.com.lingxi.code`）

**治法：部署前检查清单 + fail-closed。** 只改 YAML 不改仓库 Settings → Variables，会让 mobile-linux 授权检查变成一个**仍然报绿的空操作**。

### 4.2 三处必须新造的接缝（S1 阶段，改名之前）

| 接缝 | 现状 | 为什么必须**先**做 |
|---|---|---|
| **splitter 传参**（L1） | `prompt_format.rs:137` 自持私有 `const HEADER` | 改完之后 header 漂移是编译错误；不改则每请求静默丢 cache 断点 |
| **iOS bridge 显式映射**（L20 / H9） | `LocalAppWebView.swift:166` 用 `message.name.replacingOccurrences(of:"lingxi", with:"").lowercased()` **派生**命名空间；`:1046-1048` 注册 12 个 handler | 改名后自然的去重叠（`agentAgent` → `agent`）会让 strip 返回空串，**`agent` 这一个命名空间停止路由，其余 11 个照常**。编译干净，一个能力死掉。→ 先换成显式 `[handlerName: namespace]` 字典 |
| **iOS 数据根单一常量** | `ConversationSource.swift:1105` 与 `ProviderRepository.swift:2370` **两处独立字面量，无共享常量** ✅ | 改一处不改另一处 ⇒ 引擎读 `<AppSupport>/AgentCode`、`provider-settings.json`（API key）留在 `LingxiCode`。**只有真机能发现** |

> 补充：`apps/ios-framework/src/lib.rs:3569` 看起来像在守卫这个分叉，但它在 `#[cfg(all(test, feature="uniffi"))] mod tests` 里（`:3546` 开始），且把**同一个局部变量**同时喂给 `app_sandbox_root` 和 `workspace_host_path` ✅。它检测不出它看起来在守卫的东西。该字段自己的文档（`:131-135`）写着这个值「must be supplied, never derived」。

---

## 5. 已确认的既有 bug（与改名无关，S1 阶段修）

| ID | Bug | 位置 |
|---|---|---|
| **B1** ✅ | `branding::ENV_PREFIX` 是**零调用点的死接缝**；真正的前缀硬编码在另外两处 | `branding/src/lib.rs:49`；实际消费者 `engine/src/settings/env_parser.rs:16`、`apps/cli/src/background_dispatch.rs:932` |
| **B2** ✅ | `resolve_tmpdir()` 把 `LINGXI_TMPDIR` **读了两遍**；第二个 arm 本该是其文档注释描述的 CLAUDE 别名 | `sandbox-runtime/src/manager.rs:91-95`（文档在 `:88`） |
| **B3** ✅ | auto-mode 自我修改 BLOCK 规则列举了 **14 个本 build 不使用的配置面**。分类器读的是这段散文，所以这条安全护栏对真实配置目录是盲的 | `apps/cli/src/commands/auto_mode.rs:78,:129,:131,:133,:153` |
| **B4** ✅ | B3 的反向守卫存在，**且恰好差一个文件**：`SOURCES: [(&str,&str);7]` 只列了 7 个 `permission/src/auto_mode_*.rs`，`apps/cli/src/commands/auto_mode.rs` 不在其中。它一直是绿的 | `permission/src/auto_mode_pregather.rs:840-854` |
| **B5** ✅ | `.claude/routines/` 是**幽灵路径** —— `git grep '\.lingxi/routines'` 零命中 | `auto_mode.rs:78,:129` |
| **B6** ✅ | `.claude/loop.md` **低估了面**：加载器同时读 `<cwd>/.lingxi/loop.md` 和裸 `<cwd>/loop.md` | `cron/src/autonomous_loop.rs:347-350` |
| **B7** ✅ | 一对成对的 system-prompt 段落只改了一半：兄弟段落仍把 `CLAUDE.md` 和 "another Claude session" 送进**活的 prompt**。无 parity / 字节锁覆盖 | `traits/src/live_sessions.rs:39-46` vs `agent/src/handle.rs:1122` |
| **B8** ✅ | `CLAUDECODE=1` 仍在写入 shell-snapshot 子进程，而读端早已改成裸 `LINGXI` 标记。全树无人读 `CLAUDECODE` | 写 `tools/skill/src/prompt_shell.rs:191`；读 `permission/src/cli_mode.rs:198` |
| **B9** ✅ | 同一概念的写/读端不匹配：MCP headersHelper 子进程拿到 `CLAUDE_PLUGIN_ROOT`，hooks 与 plugins 拿到 `LINGXI_PLUGIN_ROOT`。一个 fixture 把这个泄漏钉住了 | `mcp/src/headers_helper.rs:99-108,:240` vs `hooks/src/executor.rs:2217`、`plugin/src/manager.rs:1320` |
| **B10** ✅ | `/cd` 确认提示对用户说 `CLAUDE.md`，且被 142 字节锁 + 逐字断言钉住 | `command-api/src/cd.rs:24`（断言 `:63-72`） |
| **B11** ⚠️ | 报错文案指向 `~/.claude.json`，而本 build 存在 `~/.lingxi.json` | `llm-client/src/aws_auth.rs:59,:70` |
| **B12** ⚠️ | 运行时能力原因写着 "claude-code does not support sandbox on Windows" | `platforms/windows/src/sandbox.rs:61` |
| **B13** ⚠️ | 沙箱默认写白名单授予 `/tmp/claude` 和 `/private/tmp/claude` —— 一个中性产品给另一个产品命名的目录发写权限 | `sandbox-runtime/src/path_utils.rs:401-402`（`.lingxi/debug` 在 `:404`） |
| **B14** ✅ | `tools/scripts/check_version.sh` **当前是红的**，且**仅凭目录改名就会翻绿** | 见 Context |
| **B15** ✅ | `apps/ios-framework/src/lib.rs:3569` 是一个检测不出它看起来在守卫之物的测试 | 见 §4.2 |
| **B16** ✅ | TS lockfile 解析器硬编码 `homedir()/.lingxi`，**无 `LINGXI_CONFIG_DIR` 覆盖**；Rust 写端走 `branding::CONFIG_DIR_ENV`。**bridge 发现对设了该变量的用户今天就是坏的** | `clients/shared/src/lockfile.ts:35` vs `bridge/src/lockfile.rs:160-167` |
| **B17** ✅ | branding 自测只断言 `!contains("claude")` —— 改名后它静默停止证明任何东西 | `branding/src/lib.rs:96-118` |
| **B18** ⚠️ | **上一次重命名把 oracle 引用也改了**：21 行 / 15 文件的文档注释以逐字 claude-code 源码的形式呈现，却拼作 `LINGXI_*`。`platforms/posix/src/secure_storage/helpers.rs:63` 字面写着 "Claude-code's source uses `process.env.LINGXI_CONFIG_DIR`" —— 是假的 | |
| **B19** ⚠️ | `DENIAL_MARKER`（"denied by the Claude Code auto mode classifier"）与其正则孪生是只读的一对 —— 本 build 无人发出这句话（唯一 producer 是测试）。它挖的是历史转录 | `permission/src/auto_mode_producers.rs:2420,:2439` |

---

## 6. Clean break 的波及面（D7）

D3 授权丢弃 `~/.lingxi`。下列状态**都不在它下面**，且大多编译干净、测试全绿：

| 状态 | 位置 | 处置 |
|---|---|---|
| **本地应用 SQLite 表** | `local-apps/src/data.rs:278-296` 的 `_lingxi_schema` / `_lingxi_records` + 索引 `_lingxi_records_updated`，约 14 条语句引用；**全文件无 `ALTER TABLE` / `RENAME TO`** ✅ | **写迁移**（`ALTER TABLE … RENAME TO` + 索引重建）。不写则设备上每个已存在的本地应用静默丢光记录，而**仓库里没有任何东西能抓到**（测试用 `tempfile::tempdir()` 建全新库 ✅） |
| **iOS 引擎数据根** | `<AppSupport>/LingxiCode`，两处独立字面量 ✅ | **先合并成单一常量**（§4.2），再首启 move |
| **Electron userData 根** | `~/Library/Application Support/LingXi Code`，源自 `package-support.mjs:29 APP_NAME`，`packaged-app-smoke.mjs:13` 有第二份断言 | 首启 move |
| **用户仓库里的 git ref** | `local-apps/src/checkpoints.rs:30 refs/lingxi/checkpoints`；`session/src/rate_limit_checkpoint.rs:102 refs/lingxi/checkpoint-` ✅ | 写 ref 迁移 |
| **git commit trailer** | `checkpoints.rs:31-33` 的 `Lingxi-Checkpoint:` / `Lingxi-Created-At-Ms:` / `Lingxi-Dependency-Lock-Digest:` ✅ | **不可迁移**（已写入的 commit 无法重写）。接受：旧 checkpoint 停止解析 |
| **Android Keystore alias** | `AndroidSecureStorageAdapter.kt:106 ALIAS = "lingxi_secure_store_key"`（AES/GCM `:107`） | **不可迁移**。旧密钥仍在，新 alias 产生新密钥 ⇒ 已有密文**永久不可解**。接受：重新认证 |
| **Android 通知渠道 ID** | `NotificationController.kt:16 "lingxi_agent"`、`CronNotifications.kt:30 "lingxi_cron_result"` | **不可迁移**（渠道 ID 每次安装内不可变）。接受：per-channel 声音/重要性/屏蔽设置重置。同时改 `ClipboardController.kt:11 CLIP_LABEL`（用户可见） |
| **Android 已排 PendingIntent 的 action** | `CronAlarmScheduler.kt:29`、`CronNotifications.kt:32`、`LocalAppWidgetPinRequester.kt:13-14` | **可迁移**：升级后首启用旧 action 构造 PendingIntent 并 cancel，再用新 action 重排 |
| **Android 持久化 store 名** | `ModelRecentsStore.kt:50`、`LinuxRuntimeBridge.kt:49`、`OnDemandBash.kt:197` | 写迁移（文件 rename） |
| **macOS Keychain / Linux Secret Service** | 基名 `LingXi`、`LingXi-credentials*`、`--label=LingXi`（`platforms/posix/src/secure_storage/macos.rs:126,607,609,616,617`、`linux.rs:106,193,350,351,358,359`） | **不可迁移**。接受：重新认证 |
| **GitHub repo variables** | `vars.LINGXI_MOBILE_LINUX_*`（树外，Settings → Variables） | 部署前检查清单；同时去掉 `clients.yml` 里的 `\|\| ''` 默认值 |
| **用户自己写的 skill / plugin `.mcp.json` / hook 命令** | 项目仓库与 plugin marketplace 里的 `${LINGXI_PLUGIN_ROOT}` / `${LINGXI_SESSION_ID}` / `${LINGXI_SKILL_DIR}` / `${LINGXI_PROJECT_DIR}`（112 处 / 20 文件 ⚠️） | **不可迁移**（在别人的仓库里）。接受：静默停止替换。→ 需要在 CHANGELOG 里显式说明 |
| **iOS 项目树内 scratch 目录** | `ProjectSynchronizer.swift:4-5` 的 `".__lingxi_temp__"` / `".__lingxi_backup__"` | 改名；残留物由用户手工清理 |

---

## 7. 防漏门

### 7.1 判据

门要回答的**不是**「有没有 lingxi 字样」，而是「**有没有出现在不该出现的位置的品牌 token**」。三份显式清单驱动：

1. **L1 命名空间清单** —— 这些值在 `branding` crate **之外**必须零出现
2. **L3 冻结清单** —— 每条**带理由**（例：`lingxi_secure_store_key` — Android Keystore alias，改名使已有密文永久不可解）
3. **oracle 引用白名单** —— `claude-code/src/....ts:123` 形式的注释是溯源证据，不是品牌

| # | 失败条件 |
|---|---|
| **G1** | 任何**代码行**的字符串字面量里出现品牌 token，且不在 L3 豁免清单。**needle 集合按区域不同**，见 §7.2 |
| **G2** | 任何 `CLAUDE_*` **环境变量读取点**，名字不在 D5 保留清单 |
| **G3** | **反向**：L1 的**路径与文件名**常量值（`.agent`、`.agent.json`、`AGENT.md`、`AGENT.local.md`、`.agent-plugin`、`AGENT_CONFIG_DIR`、三个 `MANAGED_DIR_*`）出现在 `branding` 之外 |
| **G4** | 注释里出现 `LINGXI_*` 却声称是 claude-code 源码引用（B18 那 21 行假引用的形状） |
| **G5** | L3 豁免清单里存在**目标已消失**的条目（死豁免 = 清单在腐烂的信号） |

> **G3 不含 `ENV_PREFIX`。** `"AGENT_"` 这个前缀值按设计会出现在 §2.4(b) 枚举的消费点上。作为补偿，B1 的修法是**把那两个硬编码消费者（`engine/src/settings/env_parser.rs:16`、`apps/cli/src/background_dispatch.rs:932`）接到 `branding::ENV_PREFIX`**，而不是删掉这个死常量——接上之后消费点收敛到可枚举的少数几处，G3 才有可能在未来收紧到覆盖它。

### 7.2 G1 的 needle 集合按区域不同

一张全局 needle 表会立刻淹没在 §1.1 里被判定为**保留**的东西上（`clients/` 的 336 处 `LingXi`、1335 处 `com.lingxi`、483 处 `灵犀`）。把它们全塞进 L3 豁免清单会让清单失去信号价值——**一份两千条的豁免清单等于没有清单**。所以门按区域用不同 needle：

| 区域 | needle 集合 | 理由 |
|---|---|---|
| `lingxi-code/`（引擎） | `.lingxi` · `LINGXI` · `LingXi` · `Lingxi` · **`灵犀`** | 引擎里今天 `灵犀` 是 **0** 处 ✅ —— 把它列进来是**回归守卫**，成本为零 |
| `clients/` | `.lingxi`（dot-dir 形态）· `LINGXI_`（env 形态）· `X-LingXi-Ide-Authorization` | 只查命名空间面。`com.lingxi.*`、`LingxiCode*`、`LingXiAccessibilityService`、`灵犀` 按 §1.1 保留，**不进 needle 表也就不需要豁免条目** |
| `.github/` · `scripts/` · `tools/` · `skills/` · `npm/` | 全集 | 无保留项 |
| `third_party/` · `docs/` | 不扫 | 冻结 |

`L17`（`platforms/android-shellbin/build.rs:260` 引用 `third_party/` 里的 `README.lingxi.md`）是**唯一**需要跨界豁免的条目。

### 7.3 实现约束（全部来自实测）

- **一律 `git grep -P` + lookaround，禁止 `\b`。** ERE 不实现 `\b` 且静默返回空（见 Context）
- 扫描面必须含 Kotlin / Swift / TypeScript / gradle / plist / xcconfig / Python / shell —— Rust 编译器覆盖不到（M3）
- 区分**代码行**与注释行（Rust `//` `///` `//!`；Kotlin/Swift/TS `//`），因为 oracle 出处引用只合法地存在于注释里

### 7.4 门的自测

`check_version.sh` 已经证明这个仓库里的门会因为一个目录改名而从红变绿。所以门本身必须被验证：

- **埋雷测试（正向）**：仓库里放一个已知违规样本，断言门**失败**。门抓不到就是门坏了
- **清空测试（反向）**：样本移除后断言门通过
- **计数基线**：门报告的违规数必须等于一个显式记录的 baseline。**只断言 exit 0 是不够的** —— 正则退化成零匹配时它也 exit 0

第三条是从 `check_version.sh` 的失效形状直接反推的。

---

## 8. 测试与验证策略

### 8.1 字节锁：先分类，再重算，最后破坏验证

58 个字节锁文件，**41 个含品牌字面量** ✅。

**① 分类依据是 fixture 自己的出处字段（`_meta.source` / `capture:`），不是它在哪个块里。**

不是形式主义：一个来自 CHANGELOG 而非真实抓取的条目，混在 sha256 钉住的 oracle 块里，会毒掉基于它的测试且**永不失败**，因为它只断言它自己。

明确**不动**的：`parity_claude_2_1_220.rs:159`（oracle 二进制自身摘要）· `claude_2_1_220_gap_oracle.json:88 "CLAUDE_CODE_WALNUT_SPIRE"`（oracle 观测到的上游名）· `parity_init_template.json` 的 `_meta.source` 与 `line_count: 21`（改名长度中性）。

**② 重算由代码算并写回，禁止手改。**

长度算术：`.lingxi`(7)→`.agent`(6)、`LINGXI.md`(9)→`AGENT.md`(8)、`LINGXI.local.md`(15)→`AGENT.local.md`(14)、`LingXi`(6)→`Agent`(5)、`LINGXI_`(7)→`AGENT_`(6)，全部 −1。已知锁：`HEADER.len()` 57 → 56（`orchestrator/tests/prompt_header_test.rs:14-23` ✅）· `OLD_INIT_PROMPT` 1582 → 1576（`commands/core/src/templates.rs:94`，4×`LINGXI.md` + 2×`LingXi` ✅）+ 其 sha `:102`。**这类算术正是手算出错的地方。**

带方框字符的 insta `.snap` 必须 `cargo insta review` 重生成 —— sed 会留下算错的 padding。

**③ 每个重算完的锁，改一个字节，断言测试红。** 重算之后没验过 = 不知道它还绑不绑。

**④ Q9 的代价必须记录在案。** 把 PRODUCT_NAME 从 parity manifest 归一化掉（像 `<CWD>`/`<MEMORY_DIR>` 那样，`parity_claude_2_1_220.rs:265`），会**失去**今天唯一能抓 orchestrator↔llm-client header 漂移的那把锁（`:586-591`，其归一化器只重写 CWD/Platform/Shell/OS/date，所以 57 字节 HEADER 在哈希里 ✅）。

因此顺序是硬的：**§2.3 的 splitter 传参改造必须先落地**，并补一条「装配后的 prompt 过 splitter 必须 ≥2 块」的测试。**覆盖是转移，不是消失。**

需重算的其余锁（部分 ⚠️）：`parity_claude_2_1_220.rs:407-452` 的 10 个模型 prompt sha 元组 ✅ · `:586-591` live-prompt 锁 ✅ · `:595-612` 两个 output-style 清单 ✅ · `command-api/src/cd.rs:66,:69`（若修 B10）✅ · `local-apps/templates/vite-react-static-v1/.gitignore` + `local_apps_build.rs:2407-2411` 的 pinned 断言 ✅ · `full_v0_4_0_smoke.json:7 env_prefix_priority` + `parity_full_v0_4_0_smoke.rs:120`（必须同时翻）· keychain service-name fixture 及其镜像测试 · `tools/web/src/blocklist.rs:79,:336` + `web_fetch_test.rs:891` 的 3 个 `assert_eq!`（在 L2 注入下全破）· `telemetry/src/otel/logs.rs:70-71` · `telemetry/src/tengu/queue.rs:47-51`

**已验证不受影响**：`prompt_assemble_test.rs:139 FOOTER.len()==900`、`compaction/src/prompt.rs:339 BASE_COMPACT_PROMPT.len()==5814`。

### 8.2 反向守卫：一次 sed 之后会「绿着，而且盲着」

| 形状 | 例子 | 为什么假绿 | 治法 |
|---|---|---|---|
| **只找上一代品牌** | ~12 处 `assert!(!body.contains(".claude/"))`（本会话正则命中 10 处 ✅，清单报 12 —— **这个数字应由防漏门导出，不由手工 grep 定**） | 单向 sed 后照常通过，而它要防的是 `lingxi` 残留 | needle 集合改成 **{claude} ∪ {lingxi} ∪ {灵犀}** |
| **断言含新品牌字面量** | `agent/src/builtins.rs:935 assert!(prompt.contains("LingXi"), "{ty} must identify LingXi")` ✅ | 翻成 `contains("Agent")` 会绿，但**证明不了 prompt 跟随注入** —— 一个硬编码的 "Agent" 同样能过 | **注入一个非默认名字**，断言 prompt 含那个名字 |
| **来源清单不全** | `auto_mode_pregather.rs:857` 的 `SOURCES` ✅ | 恰好漏掉唯一出问题的文件（B4） | 清单来源改为「所有含规则散文的文件」，由防漏门校验完整性 |
| **守卫与被守卫值同文件** | `telemetry/src/tengu/queue.rs:18-24` + `:37-43` + `:47-51` | 一次 sed 同时改掉两者 ⇒ 同义反复 | 守卫 needle 从独立来源取 |
| **负例断言的是函数名不是字面量** | `migrations/src/global_config.rs:1746,:1760,:1773,:1782,:1787` ⚠️ | 只改字面量的 sed 两个都不改，函数在它们脚下被重命名而测试照常通过 | 纳入 M5 的处理 |

### 8.3 分层验证矩阵 —— 含**明说抓不到的**

| 缺陷类 | 单测 | `--workspace` | `--all-features` | 客户端编译 | 防漏门 | 真机 |
|---|---|---|---|---|---|---|
| 常量翻转遗漏 | ✅ | | | | ✅ | |
| header 双副本漂移（L1） | ❌今天 → ✅改造后编译错误 | ✅ | | | | |
| 字节锁失配 | ✅ | | | | | |
| FFI 符号单边改（L6） | ❌ | ❌ | ❌ | ✅ 链接错误 | ✅ | |
| `.gitignore` 四处联动（L3） | ❌ | ❌ | ❌ | ❌ | ❌ | **fresh clone 测试** |
| `project.yml`↔`#if`（L5） | ❌ | ❌ | ❌ | ❌ Swift 当 false | ✅ | ✅ |
| BGTask id（L12） | ❌ | ❌ | ❌ | ❌ | ✅ | ✅ 启动抛异常 |
| 反向守卫变盲 | ❌ **它自己是绿的** | | | | ✅ 计数基线 | |
| **SQLite 表名 → 数据丢失** | ❌ **测试建全新库** | ❌ | ❌ | ❌ | ❌ | ✅ 仅带旧数据的设备 |
| Keystore / 通知渠道 / 已排 PendingIntent | ❌ | ❌ | ❌ | ❌ | ❌ | ✅ 仅**升级安装** |

最后三行是重点：**仓库里没有任何东西能抓到它们**。SQLite 那一行唯一的防线是迁移本身**加一条从旧 schema 播种的测试** —— 不是建新库，是**先建旧表名再跑迁移**。

### 8.4 只有真机能给的结论

L12 启动抛异常 · L5 静默降级到 Store 路径 · iOS 数据根分叉（API key 孤儿）· Keystore 密文不可解 · 通知渠道孤儿 · 已排闹钟无 receiver · computer-use 授权吊销。

> ⛔ **子代理看不见工具返回的图像。** 一个由 Task 或 workflow 跑的「真机验证过」的门，实际只是元数据门。真机取证必须在主会话里看。

### 8.5 跑测试本身的纪律

- `--no-fail-fast`。cargo 在第一个失败的**二进制**处停 —— **测试总数下降本身就是红旗，哪怕失败数是 0**
- 永远不要只 grep `FAILED`，那会丢掉点名测试的 `failures:` 块。全量输出落文件再 grep 文件
- 等**进程退出**，不要等第一行 `test result`（那会在半程触发）
- `--all-features`：engine-mobile 的 local-apps 模块是 `#[cfg(feature = "uniffi")]`，`--workspace` 对它是盲的

---

## 9. 命名裁定

### 9.1 已定（可推翻）

| 项 | 裁定 | 理由 / 代价 |
|---|---|---|
| `CLAUDE_CODE_X` → | **`AGENT_X`**（丢掉 `CODE_`） | 留着 `CODE_` 就是把无意义的历史段永久化。代价：88 组撞名 ⚠️，其中 **31 组同文件共存**必须逐处读 |
| 31 组同文件共存的合并 | **逐处读，禁止套用统一优先级规则** | `traits/src/uds_inbox.rs:123` 的数组是 `["XDG_RUNTIME_DIR","CLAUDE_CODE_TMPDIR","LINGXI_TMPDIR"]`，**`CLAUDE_CODE_TMPDIR` 排在前面并胜出** ✅ —— 它是唯一一处 LINGXI_ 不在前的。套用「LINGXI_ 总是赢」会合并到错的那个 arm，静默改变 UDS socket 目录 |
| parity 的 13 个 sha 清单 | **把注入名归一化掉**，而非按 "Agent" 重算 | 与 `<CWD>`/`<MEMORY_DIR>` 一致；下次产品改名不用再算。代价见 §8.1 ④ |
| `claude_code.*` OTel 命名空间（22 个值 / 56 处：19 个 instrument 名 + `METER_NAME = "com.anthropic.claude_code"` + 2 个 log signal + `DEFAULT_SERVICE_NAME`） | **跟随 L2 注入**，默认 `agent.*` | 这些发往**用户自己的** Grafana/Datadog，不是 Anthropic 的 tengu，`tengu_*` 豁免不覆盖它们。代价：破坏所有已有用户看板 |
| `window.lingxi.v2`（本地应用 JS bridge 全局） | **改**，走 client-protocol **major bump** 与既有 re-bless 流程 | 不改则每个生成的本地应用源码里都留着 `lingxi` |
| Electron `contextBridge.exposeInMainWorld('lingxi', …)` | **改**（与上一项是两个不相关的命名空间，只是同名） | |
| `CLAUDE_CODE_USE_BEDROCK` 等云后端家族 | **改成 `AGENT_*`** | D5 明确选了窄保留清单而非宽清单。代价：使用这些变量的企业 CI 需更新。`CLAUDE_CODE_USE_ANTHROPIC_AWS` → `AGENT_USE_ANTHROPIC_AWS` 半中性，但 D5 的规则如此 |
| `CLAUDE_CODE_MCP_SERVER_NAME` / `_URL` / `CLAUDE_PLUGIN_ROOT`（写给用户脚本的**出站**变量） | **改** | Clean break 决定。B9 必须先修 |
| 帮助文档 URL（`code.claude.com/docs/…` 15 处 / 10 文件，含两处启动对话框的安全指南链接与 `/agents` 帮助文案；`github.com/anthropics/claude-code/issues` 在 system prompt 的反馈行里） | **做成 L2 可注入的 doc base URL，默认空 ⇒ 省略该行** | 没有替代文档站；把用户送到另一个产品的 issue tracker 更糟 |
| 对外身份值（MCP UA `claude-code/{version}`、WebFetch UA `claude-code-tool/` 与 `Claude-User (claude-code/{})`、进程包装名 `claude-code_{}_agent`） | **跟随 L2 注入** | 中性化的目的即此 |
| `.lingxi` 派生的兄弟名（`.lingxi-build-state`、`.lingxi-scratch`、`.lingxi-dependency-ready`、`.lingxi-worktree-owner.json`、`.lingxi_writable_probe`、`.lingxi_disk_probe`） | **跟随 dot-dir** | `.lingxi-build-state` 与 `skills/create-local-app/SKILL.md:279,306` 及 `local_apps_host.rs:3146,:3154` 是 lockstep |
| `${LINGXI_*}` / `__LINGXI_*__` 模板 token（112 处 / 20 文件 ⚠️） | **改** | Clean break 决定。`__LINGXI_CSP__` 是**不存在的哨兵**（唯二命中是两处注释 ✅），活的占位符是 `__LINGXI_NATIVE_FORM_FACTOR__` |
| `LINGXI_AGENT_LIST_IN_MESSAGES` | **`AGENT_LIST_IN_MESSAGES`**（去重叠） | ⛔ 必须在 §4.2 的 iOS bridge 显式映射落地**之后**才应用去重叠模式（H9） |
| `LINGXI_DISABLE_LINGXI_MDS` | **`AGENT_DISABLE_MEMORY_FILES`** | 唯一的双品牌名；`MDS` 只在文件叫 `CLAUDE.md` 时才讲得通。Clean break 下改名零成本 |

### 9.2 曾经开放，现已收敛（2026-08-23）

| # | 问题 | 裁定 |
|---|---|---|
| **O1** | `CLAUDE_CODE_OAUTH_TOKEN` 保留还是改？ | **保留**，加入 D5 保留清单。它命名的是 Anthropic 签发的凭证，与 `CLAUDE_AGENT_SDK_*` 同类 |
| **O2** | `CLAUDE_CODE_API_KEY_HELPER_TTL_MS` | **改成 `AGENT_API_KEY_HELPER_TTL_MS`** —— 我方自己的旋钮 |
| **O3** | §3.3 的 34 个硬编码 `"LingXi"` const 项如何分配 | **规则采纳**：第三方看得见或据以索引的值（Copilot/OpenAI/WebSearch UA `LingXi-Code`、IDE lockfile 名、MCP clientInfo、keychain service 基名、git author 名、PR attribution）→ **L3 冻结**，除非配迁移；只有用户读的 → **L2 注入**。**逐项列表在实施计划里产出，不在本文** |
| **O4** | coordinator 的身份行（3 处） | **跟随 L2 注入**（与 CLI 身份行一致）。⚠️ 这是**未经用户逐项确认的假设**，若 coordinator 的身份需要与主 agent 区分则应推翻 |
| **O5** | npm 包名 `agent` 是否可用 | **降级为实施任务，不是设计问题**：计划 B 里加一步查 npm registry（连带 6 个平台包 `agent-{linux,darwin,win32}-{x64,arm64}`）。⚠️ 若 `agent` 已被占用，回退命名需要用户拍板，届时阻塞 S5 而非更早 |

---

## 10. 需要重新导出的数字

以下数字来自单一 finder、未经复核，**实施第一步必须由防漏门重新导出**，不得直接采信：

- `LINGXI_*` 不同名的总数：本会话 `git grep -hI -oE 'LINGXI_[A-Z0-9_]+' | sort -u | wc -l` 得 **259**；finder 的枚举是 250 名 + 10 个前缀片段 = 260。**有一个 token 对不上。**
- §4.1 M1 里 `.lingxi` 生产 Rust 字面量的 124/49 拆分
- `LINGXI.md` / `LINGXI.local.md` 字面量的 44/22 拆分
- SET A 的 88 组撞名与 SET B 的 31 组同文件共存的完整枚举
- 反向守卫的准确条数（本会话得 10，finder 报 12）

---

## 附：勘察方法

12 个 agent：7 个 finder（环境变量 / branding 接缝 / live 字面量 / 字节锁 / 树与构建 / clients 命名空间 / 用户可见文案）→ 3 个对抗性校验（「改了会不会断线」/「sed 撞名」/「改错了还全绿吗」）→ 1 个完整性评审（「谁都没看的面是什么」）→ 1 个汇总。校验推翻了 finder 的若干分类，本文采用校验的裁定。完整性评审补上了 7 个 finder 全体漏掉的 8 个面，其中最大的是 **483 处 `灵犀`**（所有 finder 都只用了拉丁字母 needle）。

byte-locks finder 中途因 API 错误终止，其覆盖面由本会话手工补测（58 个字节锁文件 / 41 个含品牌字面量）。
