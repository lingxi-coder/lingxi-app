# 会话历史缓存与增量续接接入

产品固定依赖：

- llm-client：`25d898973df7e4bbad9889d6805097bcb425b285`
- Harness：`a8dd167efd1b930c97aa609307e5a252b32792f2`

共享 Rust 运行时负责历史投影缓存、provider 增量续接与一次失败恢复。Desktop/CLI/Mobile 使用同一实现；本次无需前端回传完整历史，也不改变公开消息格式、持久 transcript 或 Remote attach 的 epoch/after_seq 协议。

主会话持有独立续接状态。side chat 沿用独立 ApiService，btw、compact 摘要及其他 stateless sidequery 只复用去除历史的空闲连接。辅助请求归属由 Host 显式标记，不能通过复制主会话的 query-source 标签取得父会话状态。compact 的提交边界清空主会话缓存、推进 generation，旧流不能写回。

HTTP 和首个 SSE/WS 事件中的明确续接拒绝最多恢复一次；原请求先结算，恢复重新签名、准入并扣减物理尝试预算。完整历史仍由 Host 保存，恢复不会重新执行工具。未知结果和已生成后的断流不因此增加重放。

存储策略保持现有边界：显式 store=false 继续有效，OpenAI/Qwen/GLM 持久 HTTP 续接需明确允许存储；Responses WS 可以使用连接内 store=false 续接。首次请求、进程重启、历史不匹配及不支持的 provider 使用完整历史。

本机 debug 探针：约 4 MiB JSON clone 64 次，正文复制从约 256 MiB 降为 0；128 条消息、2 MiB 工具结果，投影 8 次从约 8.10 秒降至 1.14 秒。该局部测量不等于整体首字节延迟；完整规范化、请求编码和安全前缀检查仍有历史大小相关的开销。

SDK 766 项回归、Host 60 项集成/流式/故障转移、sidequery 76 项、compact 相关 95 项、persistent_chat 相关 29 项通过。Host 完整串行单元回归 1451 通过，2 项 Fusion 模型目录断言失败；两项已在未修改 Harness e2051f1a + 旧 SDK a30a3a9 的基线复现。性能探针已另外运行通过。

Harness 的 `docs/performance/conversation-history-20261103.md` 记录缓存边界、测量方法和详细验证范围。发布时按 SDK → Harness → 产品的顺序使固定提交可获取；本次实现分支不包含向 main 的合并或推送。开发用绝对路径覆盖已移出仓库，固定版本编译不依赖它们。

真实 provider 账户的性能、跨端 UI/断线续传、移动设备及签名包仍需产品验收；上述离线测试不建立这些结果。

最终固定 Git 依赖组合：`cargo check --offline --locked -p bridge-server --lib` 通过，未使用路径覆盖。
