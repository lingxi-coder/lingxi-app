# 官方 Provider 与模型目录

Desktop、Android、iOS（以及 CLI）使用 Harness `provider-config::StartupCatalog` 的同一套目录服务。Firebase 是官方选项的发布入口；用户的凭据、自定义连接、默认模型和可见性设置独立保存。

## 数据与发布

只接受 `schemaVersion: 2`。不读取或转换旧版目录、缓存和云端草稿。Firebase 使用 `catalogV2State`、`catalogV2Payloads`、`catalogV2Parts`、`catalogV2Releases`；客户端缓存位于 `cache/provider-catalog-v2`。旧目录数据不删除。

`apps/firebase/catalog-policy.json` 明确列出初始精选模型，首个模型为 provider 默认推荐。`generate-seed.py` 从 SDK 提取这些型号的完整 TOML 执行定义，生成 Firebase seed 及 Harness 内置目录。当前为 11 个 provider、25 个模型；OpenAI 仅选 GPT 6/6.1，DeepSeek 仅选 V4 系列，Google 默认 Gemini 3.8 Flash。

后台保存、预览、发布互相独立。发布生成不可变快照并原子切换当前指针。历史恢复只恢复草稿，重新发布后才对客户端生效。新版命名空间第一次使用需先保存 seed 草稿再发布；没有已发布目录时客户端继续使用有效缓存或内置目录。

每个 provider 必须携带完整 `executionDefinition`、SHA-256、`requiredCapabilities` 和 `defaultModelId`；不会根据本地 SDK 猜测或补齐缺失定义。模型包含状态、平台、应用版本范围及运行能力要求。新协议、认证流程或新增执行能力需客户端实现并声明能力后才能开放。

## 获取与更新

启动立即使用有效缓存，没有缓存则使用内置精选目录；网络检查在后台执行。下载和校验成功后替换共享快照、通知客户端刷新，并保存缓存。失败保留上一份有效目录。

- 启动检查；持续前台每小时检查，调度 tick 带少量随机延迟。
- 回到前台：上次成功检查距今超过一小时，或上次失败且退避已到期时检查。
- 打开模型设置/选择器：成功检查距今超过 15 分钟时检查。
- 恢复联网：失败或过期时重试。离线、后台暂停自动网络请求。
- 用户手动刷新：跳过时间限制。
- 失败重试间隔依次为 1、5、15、60 分钟。

同进程 owner 共用快照、订阅及单次请求锁；Desktop 多个 bridge 进程通过缓存目录的文件锁及最近检查时间避免启动时重复下载。HTTP 使用 ETag/304；缓存按平台、应用版本、schema、运行能力和目录地址隔离。

## 运行与选择

`active` 正常展示；`deprecated` 保留调用能力，不推荐新选；`disabled` 保留停用声明并拒绝新调用。停用条目必须下发，避免用户保存的 profile 把旧模型重新带回。目录更新不自动改写用户默认选择。

官方连接继承云端模型清单，不注入 SDK 全量列表。自定义连接保留用户自己的地址和模型。供应商 `/models` 用于连接检查，不自动扩充官方选择器。

每个 turn 的模型/工具循环使用独立目录快照。更新后的目录供新 turn 使用；正在运行的 turn 继续使用已捕获的执行事实和请求级凭据。Desktop、Android、iOS 接收相同的 ProviderModelCatalog 事件及完整的非秘密连接元数据，新增采用现有协议的 provider 不需要维护三套型号白名单。

## 验证与上线边界

目录测试覆盖 schema/能力/平台/版本筛选、停用声明、ETag、离线缓存、刷新时机、退避、任务快照和自定义连接保留。Desktop 另做设置列表、凭据解析及命令校验测试。

初期源代码验证通过本地 Harness 覆盖进行；合并交付使用 llm-client `39f64b37e686a2eef4fc4bef2283836415d945a0`、Harness `d3b1431857e0b1f59b799577a5126a5569ebd934` 的不可变 Git pin，并使用锁定的正式依赖重新检查。Firebase Functions/管理页部署及保存、发布新版精选目录仍属于后续上线步骤。此文档不表示已经部署或完成真机验收。


### 本次本地验证（2026-10-29）

| 检查 | 结果 |
| --- | --- |
| Harness provider-config 全部单元测试 | 94 通过，含 11 项目录回归 |
| Harness desktop + mobile 特性编译检查 | 通过 |
| Product bridge-server 编译检查（正式 Git pin，--locked） | 通过 |
| Desktop 设置、凭据、协议命令等相关测试 | 103 通过 |
| Desktop 与管理 Web 的 TypeScript 检查 | 通过 |
| Firebase 测试 | 23 通过，8 项模拟器测试跳过 |
| 精选 seed / Harness 内置目录 / TypeScript 协议生成校验 | 通过 |
| iOS 修改文件与生成绑定的 Swift 语法检查 | 通过；未完成完整构建 |
| Android Kotlin 编译 | 未通过：当前 Local App、原生媒体 FFI 符号缺失；本次目录修改文件没有报告编译错误 |

未做 Firebase 部署、云端目录发布、签名打包或真机验收。本次没有添加旧 schema、旧目录缓存或旧草稿的数据转换。原有独立的用户设置处理不属于目录迁移。

合并交付额外检查：llm-client 全部 33 项 preset 测试通过，同时修正 GPT-6.1 Terra 的目录排序；精选 seed 的 provenance 指向已提交且无未提交数据修改的 llm-client 源。

正式 pin 的 Cargo 依赖来源检查通过，development=false；客户端协议检查覆盖 858 个原生源文件、3 份 host fixture 镜像和生成的 TypeScript 定义。
