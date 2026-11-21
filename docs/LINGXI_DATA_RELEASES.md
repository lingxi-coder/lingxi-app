# LingXi Data 的版本化报表

用户在个人 Chat、Team 或 Desktop 中选择数据源、权限分区、分组和指标，预览实际会发布的分区汇总与读者范围，再确认发布。Chat 传递 `lingxi-data:release/<dataset>/<release>`。每次打开由服务端用当前身份读取该版本。“查看最新”读取同一目标数据集最后一次成功发布的版本。

## 输入与冻结

- Desktop 私有源由数据源所有者在本机采集，每次采集只做一次一致读取。MySQL 对指定视图执行一条有界的只读查询。PostgreSQL 在只读的 REPEATABLE READ 事务中执行一条查询，SQLite 执行一条 SELECT。MongoDB 在一个快照会话中读取，需要 MongoDB 5.0 以上的副本集或分片集群，单机部署只能预览。Redis 在一次 MULTI/EXEC 事务中读取前缀下的键，最多 20,000 个，扫描之后新建的键不计入。CSV 和 JSON 从一次文件读取取得数据。普通 WebRTC 和 HTTPS 预览许可继续只允许预览。发布还需目标 Team 会话的数据集管理权。
- Desktop 导入宿主配置时，MySQL、PostgreSQL、MongoDB 和 Redis 的连接信息只能引用钥匙串项 `com.lingxi.data.<kind>`，账户为源 ID，且不接受回环测试模式。SQLite、CSV 和 JSON 使用绝对路径。
- 宿主按权限分区和图表分组执行计数或定点十进制求和。它用本机密钥签署获批指标定义、目标及受众绑定、分区汇总和输入摘要。连接信息与原始行留在宿主。相同查询涉及多个独立数据源时，应分别发布和引用各自的采集结果。
- 个人或 Team 协作记录由 Data 服务从同一次读取到的数据集状态汇总。请求携带预期 epoch，发生并发变更时重新预览。受保护的源导入须从源所有者的发布流程取得新授权。
- 分区和分组使用文本字段。金额使用十进制文本或整数，保留最多 12 位小数。精度不足、溢出或超过捕获限额会中止发布。单次源采集最多 100,000 行、8 MiB 投影，文件源当前上限为 1 MiB，每个版本最多有 4,096 个分区与分组组合。

## 发布与读取

| 产品命令 | 约定 |
| --- | --- |
| `reportContext` | 返回当前发布版本以及经数据集和会话共同授权的成员与代次。 |
| `prepareReport` | 将目标、预期父版本、源版本、指标、每位读者的分区和获准处理报表的 Bot 绑定在一起。协作记录返回待确认汇总。Desktop 取得绑定后在本机预览签名结果。 |
| `publishReport` | 核对确认摘要与当前授权，在一次 Firestore 事务中写入不可变分块、受众和版本头。两个不同候选争用同一父版本时只有一个成功。 |
| `getReport` | 用固定版本或当前版本读取。先按原始批准的成员代次、当前会话与数据集权限、当前分区限制过滤，再计算总数。返回值保持十进制精度。 |
| `limitReportReader` | 管理员通过预期权限版本修改当前读者的分区限制。历史版本的原批准范围仍是上限。 |
| `openReportReview` | 为一个确定版本、分区和分组创建复核单。重复请求得到原复核单，人和 Bot 共用业务去重键。 |

发布版本的身份由批准的计划和确认摘要决定。历史读取使用已发布的事实，模型重新生成的指标或源数据后续更正需另行发布。取消源接入影响后续采集。取消报表读取权限影响后续受控读取。用户已经看到或主动导出的信息由接收端持有。

## Bot、四端与验收

共享 Bot 同时受任务许可、部署版本、数据集授权和发布者的报表 Bot 名单限制。计算输入取会话成员当前均可接收的分区交集。创建复核单另外要求显式授予该动作。私有源预览任务仍受独立执行边界约束。Local App 通过已绑定的数据集别名调用同一服务，受应用声明的读写级别和当前账号约束。

验收使用隔离的 Firebase 与 MySQL。`apps/firebase/scripts/test-lingxi-data.py` 加 `--private-preview --postgres-bin` 时，另用真实 PostgreSQL 源依次走通注册、本机宿主、预览和签名报表发布，并确认每种新数据库类型都能注册。LingXi Data 自身的 `tests/acceptance.py` 对 PostgreSQL、SQLite、Redis 和 MongoDB 各启动一个真实服务。检查超过一个记录页的输入、超出 JavaScript 安全整数的金额、并发发布、读者权限变化、原始快照和最新版本、源离线、Bot 任务与独立业务回执。Web、Desktop 和原生移动端均从消息中的引用打开报表。移动端 Local App 通过系统默认浏览器进入。

签名 Desktop 联验使用 Flare 的 development 通道。构建时将 `LINGXI_SIGNED_REPORT_STAGING` 指向 staging 输出的端点日志。运行验收时设置 `LINGXI_DATA_SIGNED_REPORT_QA=1`、`LINGXI_DATA_STAGING_ENDPOINTS`、已发到 Chat 的 `LINGXI_TEST_REPORT_LINK` 和数据源启动参数。验收包只在独立诊断账号槽位中连接本机 demo 项目。测试先以读者和所有者各打开一次 Chat 报表，再用系统钥匙串里的临时签名密钥生成一个新版本，并独立检查持久复核单。

USB iPhone 使用独立的 `com.lingxi.data.reportqa` 开发包和 `LINGXI_DEVICE_DATA_STAGING` 编译条件。将实机 `.xctestrun`、设备 UDID、staging 日志传给 `apps/ios/native/scripts/test-data-staging-usb.py`。提供一次性 Local App 启动器日志后，也可验证默认浏览器。测试进程在手机回环地址监听，通过配对 USB 转发到预先选定的本机服务。
