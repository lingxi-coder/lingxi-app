# MCP discovery cache production alignment（Claude Code 2.1.251）

日期：2026-08-31  
范围：MCP discovery cache Stage 3、resources/prompts 惰性拨号、MCP grant 分区、desktop production wiring  
基线提交：`48d6ceba0`（Stage 3 与惰性拨号 checkpoint）

## 1. 结论

本轮关闭了此前报告中明确 defer 的 production identity 与 composition-root wiring：

- desktop 现在把持久化 `DiscoveryCacheStore` 注入 production `McpRegistry`；
- 缓存分区只依赖固定兼容域和远程 MCP server 自己的 OAuth refresh grant；
- 不读取、保存或推导 Anthropic account UUID，也不读取任何 LLM provider credential；
- stale cache 命中会立即提供 catalog，并单飞执行后台重验证；
- 重验证失败只 strike 当初提供 stale catalog 的确切 partition；
- MCP refresh grant 在请求期间轮换时，不会把旧 catalog 或旧 strike 写入新 partition；
- tools、resources、resource directory 与 prompts 都能从 `Cached` 状态按需拨号；
- plugin unload/remove/disconnect 会 best-effort 清除对应 server cache family。

这表示本报告范围内的 discovery-cache 功能已经 production-reachable。它不等于声称所有 Claude Code MCP 内部概念均已移植；明确残余项见第 12 节。

## 2. 审查 oracle

本轮继续使用固定的 Claude Code 2.1.251 本地二进制作为 oracle：

```text
/Users/luolingfeng/.local/share/claude/versions/2.1.251
SHA256 625869b01e0050f260b2980fac248fd9cef9e462612bded4ec9d3d49ff8969a5
```

关键恢复点：

- eligibility/miss decision：Mach-O `cot` 附近 `@176266197`；
- gate `me`：`@176260900`；
- fingerprint/write refusal：`@176269458`、`@176271314`；
- logical-key canonicalization：`fM`/`ur`，二进制 `@157121288`；
- TTL、max-stale、strike defaults：`@176259300`；
- fresh/stale hit telemetry 与调用点：`@182536408`。

所有 `[GREP]`/literal 结论均以同批阳性对照为原则；没有把模块注释自述当作 oracle。

## 3. 认证边界：MCP OAuth，不是 LLM provider auth

LingXi 是多 provider 项目。MCP/plugin 不能依赖 Anthropic 登录态。

| 输入 | 是否进入 MCP cache partition | 说明 |
|---|---:|---|
| Anthropic API key | 否 | 只属于模型 provider 请求 |
| Anthropic OAuth/profile/account UUID | 否 | MCP 路径完全不读取 |
| OpenAI/Gemini/其他 provider credential | 否 | 同样不读取 |
| MCP server access token | 否 | 短期轮换，不作为稳定 grant identity |
| MCP server refresh token | 是，先哈希 | 代表远程 MCP grant；原文不落入文件名或日志 |
| 无 MCP token row | 是 | 映射为 `grant:none` |
| token row 存在但没有 refresh token | 禁止缓存 | 返回 `no-fingerprint`，避免错误共享 |
| MCP secure storage 无法读取/损坏 | 禁止缓存 | fail closed，不读、不写 |

代码中的固定字节 `acct:logged-out` 被命名为 `PROVIDER_NEUTRAL_IDENTITY_DOMAIN`。它只是复现 oracle hash material 的固定兼容域，不表示 LingXi 登录/登出状态，禁止替换为 provider profile、credential 或 account UUID。

## 4. 分区算法

### 4.1 MCP grant token

```text
no stored MCP token row:
    grant_token = "grant:none"

stored refresh token:
    grant_token = "grant:" + SHA256(refresh_token)[0..16]
```

固定向量：

```text
refresh-token -> grant:0eb17643d4e92611
```

access token 改变不会改变 grant token；refresh token 改变一定会改变 grant token。

### 4.2 Fingerprint

```text
fingerprint = SHA256(
    "acct:logged-out" + NUL + grant_token
)
```

固定向量：

```text
grant:none
-> 856f0d2375be22a510e79662f22d30c51c14dc3394b9d610af33a7116d81cda6
```

### 4.3 Logical cache key

logical key 使用恢复的 canonical-config 规则：

```text
logical_key = server_name + "-" + SHA256(canonical_config_json)[0..16]
```

会剔除 discovery 结果、cache state、scope、plugin path/source、config error 等非配置身份字段；对象 key 递归排序。`discoveryCache` 本身是 gate，不进入 logical key。

固定向量：

```text
server = srv
config = {"headers":{},"timeout":10,"type":"http","url":"https://a.example"}
logical_key = srv-3a9ea8118cd8b809

config.oauth = {}
logical_key = srv-3e065924e4160070

config.oauth = {"clientId":"client"}
logical_key = srv-95bfe547b37316e7
```

OAuth optional fields只有在 source config 中实际存在时才进入 canonical JSON；不存在的字段不会被 Rust `null` 扩大。

Agent scope 需要 oracle 的稳定 `agentSource`，当前 Rust config 没有这个字段，因此 Agent scope 明确 fail closed，而不是错误共享普通 server partition。

### 4.4 Partition filename

```text
partition_key = SHA256(
    logical_key + NUL +
    fingerprint + NUL +
    "era:legacy" + NUL +
    "2.1.251"
)[0..32]

filename = partition_key + ".json"
```

固定向量：

```text
logical-cache-key + grant:none fingerprint
-> a6fad12e13235da65ecc9b068d2c62b6.json
```

只接受 32 位小写十六进制 partition key；server name、URL、token 等均不会进入文件名。

## 5. Fresh / stale / miss 状态机

默认值：

| 参数 | 默认值 |
|---|---:|
| feature flag | off |
| TTL | 900 秒 |
| max stale | 14,400 秒 |
| max-stale hard cap | 604,800 秒 |
| strike threshold | 1 |

### Fresh

1. 校验 gate、fingerprint、partition、schema 和 logical key。
2. 将 catalog 安装为 `McpConnectionState::Cached`。
3. 不打开 transport。
4. tools/resources/prompts 可以立即参与 catalog 展示。
5. 第一次真实调用通过单飞 lazy upgrade 拨号。

### Stale

1. 与 Fresh 一样立即返回 cached catalog。
2. 创建 `LazyUpgradeSlot`，保存 cached generation、config snapshot 和命中 partition。
3. detached owner 在后台执行 live initialize/catalog discovery。
4. 只有 generation/config 仍匹配时才以 live catalog 原子替换 Cached。
5. CAS reject 的 live transport 会被断开，并进入有界清理重试。

### Miss

按 oracle vocabulary 发射 miss telemetry，然后 live dial。`discoveryCache:false` 与 `headersHelper` 会额外 purge 整个 server family；feature disabled、transport ineligible 等非动态 gate 不 purge。

## 6. Strike 精确语义

旧实现曾把“普通 connect 失败”作为保守 strike 信号，这会扩大 oracle `_6e` 的语义，并且在 grant 轮换时可能重新计算到另一个 partition。

最终实现：

- 只有 stale background revalidation failure 记录 strike；
- partition 在 stale hit consult 时计算并进入 `LazyUpgradeSlot`；
- failure path 不重新读取当前 grant，不重新计算 partition；
- strike 前再次验证 cached connection id 与 config snapshot 仍是当前 generation；
- replacement generation、disconnect/remove 或配置改变后不 strike；
- ordinary initial connect failure 保持原 entry 的 strike 为 0；
- grant A 命中、期间轮换到 grant B 时，只更新 A，B 保持不变。

## 7. Write-through 与 grant rotation

live discovery 的 write-through 使用两阶段 partition 检查：

1. authenticated transport 建立成功后、catalog RPC 前捕获 partition；
2. catalog RPC 完成后重新读取 MCP grant 并计算 partition；
3. 两者完全相等才写入；
4. 不相等说明 refresh grant 在请求期间轮换，跳过本次写入。

因此旧 grant 下获取的 tools/resources/prompts 不会被标记成新 grant 的 catalog。

## 8. Resources 与 prompts 惰性拨号

以下路径均接受 `Connected` 或 `Cached` server：

| 表面 | Cached 行为 |
|---|---|
| `ListMcpResourcesTool` 指定 server | `ensure_connected_client` 后执行 `resources/list` |
| `ListMcpResourcesTool` 全 server | 逐 server best-effort lazy dial，单个失败不阻塞其余 server |
| `ReadMcpResourceTool` | lazy dial 后执行 `resources/read` |
| `ReadMcpResourceDirTool` | lazy dial 后重新读取 live capabilities，再执行 directory read |
| `connected_prompts` | 从 cached catalog 暴露 prompt command |
| `get_prompt` | 以 cached connection generation 为 predecessor，lazy dial 后验证 live generation/config 再调用 |

这一设计避免了“普通 cached tools 可用，但 resources/prompts 被当作未连接跳过”的旧缺口。

## 9. Plugin 与生命周期失效

| 事件 | Cache 行为 | MCP OAuth token 行为 |
|---|---|---|
| disconnect | best-effort purge server family；I/O 失败只记录日志 | 按现有 disconnect 语义处理 |
| remove | best-effort purge server family；I/O 失败只记录日志 | 按 remove 语义撤销/删除 |
| plugin unload/remove | best-effort purge plugin server family；I/O 失败只记录日志 | 保留 OAuth grant，避免卸载插件导致用户被远程 server 登出 |
| `discoveryCache:false` | best-effort purge family | 不触碰 token |
| `headersHelper` 启用 | best-effort purge family | 不触碰 token |
| refresh grant replacement | 新 partition | 旧 partition 不再命中；生命周期 purge 可清整个 family |

plugin 中的 `anthropic/*` metadata 或官方 marketplace 标识只属于上游 wire/marketplace compatibility，不构成 Anthropic auth 依赖。

## 10. 持久化安全边界

production desktop root：

```text
<lingxi_home>/mcp-discovery-cache/
```

安全属性：

- 根目录 Unix mode `0700`；
- entry 文件 Unix mode `0600`；
- 单 entry 最大 8 MiB；
- read 使用 no-follow 打开并验证 regular file；
- symlink、非普通文件、超大文件、schema 错误、logical-key mismatch 均视为 corrupt miss；
- write 使用同目录 exclusive staging file + atomic rename；
- staging filename 只使用 partition digest 和内部 nonce；
- purge 不跟随 symlink；
- serialized catalog 写入前对 config header/env/URL credential 与 MCP stored token/client secret 做反射检查；
- 反射检查覆盖复合 Cookie/header 中以空白、`,`、`;`、`=` 分隔的 secret，以及 URI percent-encoded token（大小写 hex）；
- secure storage 无法读取时拒绝写入，而不是在不知道 secret 的情况下冒险持久化。

## 11. Production composition

Desktop：

```rust
.with_discovery_cache_store(DiscoveryCacheStore::new(
    cfg.lingxi_home.join("mcp-discovery-cache"),
))
```

store 总是可达，但 `MCP_DISCOVERY_CACHE` 默认关闭；未启用时不会读写缓存。

Mobile 当前 registry 只注册 `InProcess` transport，而 discovery cache gate 只允许 HTTP/SSE。为避免创建永远不会使用的持久目录，本轮明确不在 mobile composition root 接 store。这是经代码路径确认后的平台边界，不是遗漏。

## 12. 明确残余项

这些项目不阻塞本报告范围，但不能被描述为“Claude Code MCP 内部实现 100% 完全移植”：

1. **Agent `agentSource`**：缺少稳定 source identity，当前 Agent scope cache fail closed。
2. **Protocol era**：当前 transport 实际只执行 legacy，partition 固定 `era:legacy`；未来真正启用 modern transport 后必须纳入 era。
3. **`role` runtime**：JSON validation 已存在，但没有 LingXi runtime consumer。
4. **`cli-owned` / `env-placeholder` / `ambient-credential` gate**：LingXi config model 没有等价状态。
5. **`skills-capable` / `channel-capable` / `live-connection` post-hit miss**：依赖 Claude coordinator/plugin discovery 上下文，当前没有可靠映射。
6. **Mobile remote MCP**：若未来 mobile 支持 HTTP/SSE，需要再决定平台安全存储与 cache root。

Anthropic credential account UUID 持久化不是残余任务，也不应成为任务。

## 13. 修改文件职责

| 文件 | 职责 |
|---|---|
| `mcp/src/discovery_cache.rs` | policy、logical key、fingerprint、partition、store hardening、fixed vectors |
| `mcp/src/oauth.rs` | MCP refresh grant token 派生与 secure-storage fail-closed |
| `mcp/src/registry.rs` | consult/read/write、Stage 3 owner、exact strike、lazy dial、lifecycle purge |
| `mcp/src/connection.rs` | `discovery_cache: Option<bool>` runtime config |
| `mcp/src/json_config.rs` | `discoveryCache` parse/validation/threading |
| `tools/mcp/src/mcp_tool.rs` | resources list/read lazy dial |
| `tools/mcp/src/read_mcp_resource_dir.rs` | resource directory lazy dial 与 capability recheck |
| `plugin/src/manager.rs` | plugin-owned server unload/remove lifecycle |
| `apps/engine-desktop/src/lib.rs` | production store composition 与 wiring regression test |

## 14. 验证结果

已执行：

```text
cargo test -p mcp --quiet
612 passed; 0 failed; 3 ignored

cargo test -p tool-mcp --quiet
139 passed; 0 failed

cargo test -p plugin
137 unit + 9 discovery_bootstrap + 3 enabled_discovery + 11 materialize
160 passed; 0 failed

cargo test -p engine-desktop build_wires_mcp_discovery_cache_store
1 passed; 0 failed

cargo check --tests \
  -p cli -p engine-desktop -p engine-mobile \
  -p orchestrator -p test-harness -p tool-meta
passed

git diff --check
passed
```

重点回归包括：

- fixed fingerprint/partition/logical-key vectors；
- access-token 轮换不改变 grant identity；
- refresh-token 轮换改变 grant identity；
- stale background success/failure/panic/CAS reject；
- strike retained partition，不 strike replacement partition；
- ordinary failed connect 不 strike；
- `discoveryCache:false` purge + live dial + no rewrite；
- resources 三工具与 prompts cached-generation lazy dial；
- disconnect/remove/plugin unload retirement；
- desktop production store 可达；
- mobile 与下游 config literal 类型检查。

仓库仍有既有的 missing-docs、deprecated rand、unused test import 等 warnings；本轮没有扩大到无关清理。

## 15. 交付判定

最终 Sol xhigh 修复验证结论：`No findings`，`Verdict: SHIP`。

- [x] focused/full package tests 保持通过
- [x] scoped rustfmt 与 `git diff --check` 通过
- [x] 最终 Sol review 无 P0–P3 未解决 finding
- [x] 第一轮 review 的 composite/encoded secret、OAuth null canonicalization、文档 best-effort 三项已修复并复审
- [x] follow-up 使用 Lore commit protocol 提交

若后续修改 identity、grant、logical-key、protocol era 或 plugin unload 语义，必须重新运行 fixed vectors、grant rotation、stale strike 和 lifecycle purge 测试。
