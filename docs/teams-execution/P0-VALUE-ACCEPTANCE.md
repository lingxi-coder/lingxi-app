# P0 W02/W03/W04 验收驱动与研究记录

范围是已批准 v2 的固定 F01–F05、T01–T05 和独立 24h 窗口。工具只使用当前 execution envelope，业务数据只使用已有清单 `text/done` 和文档 `title/body`。不创建新任务系统，不创建/发布/清理云资源，不更改额度。源码测试不构成真实包、模型、设备或研究验收。

## 本地运行

在 `lingxi-app` 下执行：

```sh
node --test apps/firebase/test/team-value-acceptance.test.mjs apps/firebase/test/team-value-http-adapter.test.mjs apps/firebase/test/team-value-host-receipts.test.mjs
node apps/firebase/scripts/team-value-acceptance.mjs fixtures
node apps/firebase/scripts/team-value-acceptance.mjs workflow-command F03 /absolute/private/workflow-config.json
node apps/firebase/scripts/team-value-acceptance.mjs workflow-command F04 /absolute/private/workflow-config.json
node apps/firebase/scripts/team-value-acceptance.mjs grade-workflows /absolute/private/workflow-observations.json /absolute/private/workflow-grade.json
node apps/firebase/scripts/team-value-acceptance.mjs stable /absolute/private/stable-config.json /absolute/private/stable-checkpoint.json /absolute/private/existing-qa-adapter.mjs
node apps/firebase/scripts/team-value-acceptance.mjs stable /absolute/private/stable-config.json /absolute/private/stable-checkpoint.json ./apps/firebase/scripts/team-value-http-adapter.mjs
```

`workflow-command` 只输出 synthetic fixture 的当前 envelope，交由现有 QA runner 的 authenticated messages API 执行。F04 和 F03 的整个 command、actor 均相同；必须保存并原样重放 F03，不能新建 IDs，也不能重新说相同一句话。F05 的命令是 `readList`，换设备打开和历史仍须真实 UI 证据。T05 先在第二设备打开原清单/文档，再发送生成的追问。固定 fixture 在 [P0-VALUE-FIXTURES.json](P0-VALUE-FIXTURES.json)，源码在 `apps/firebase/scripts/team-value-fixtures.mjs`；二者 digest 必须一致。

workflow-config 的无凭据结构：

```json
{
  "runId": "qa_workflow_01",
  "scope": { "kind": "team", "id": "EXISTING_QA_TEAM_ID" },
  "binding": {
    "conversationId": "EXISTING_QA_CHAT_ID", "sharedBotId": "EXISTING_QA_BOT_ID",
    "first": { "generation": 1 }, "second": { "generation": 1 }, "secondDevice": { "generation": 1 }
  }
}
```

两个独立 QA scope/Chat 使用分别冻结的家庭与团队资料；配置填写真实 ID 与当前 membership generation。`first`、`second` 是不同成员；`secondDevice` 是第二成员在另一真实设备的相同身份。任务负责人甲/乙/丙是项目资料字段，不要求是登录账户。不把真实账号或凭据复制进仓库。

## 两条业务验收

日期/来源/参与者均在 fixture 中固定为 2026-11-16 和 v2 计划来源。源资料先通过当前发布流程共享并保存资源 revision/digest；执行中不得补充未共享的业务答案。所有采购、负责人、日期、依赖使用 `text`，完成状态使用 `done`，不存在结构化库存/负责人/日期新字段。

| Case | 执行与预期 |
| --- | --- |
| F01 | 家庭成员甲从牛奶0盒/鸡蛋2个/大米1袋，目标2/6/1生成两项：采购牛奶2盒、鸡蛋4个；done=false；无大米采购项。 |
| F02 | 家庭成员乙把鸡蛋目标改4个；同一鸡蛋项采购变2个；牛奶保留，不新增重复项。 |
| F03 | 成员甲标记原牛奶项done=true；鸡蛋仍false，text不改。保存完整原command与任务/费用/副作用快照。 |
| F04 | 原样重放F03的commandId/messageId/changeId及全部字段；同taskId；taskIds、quotaCounts、usage、operationIds、listDataRevision、historyDigest前后完全相等。 |
| F05 | 另一设备上成员乙重新打开同清单和历史；看到F03最终状态、相同item IDs、相同revision，并核对F02与F03操作者/变更来源；F04不增加历史。 |
| T01 | 从资料生成A提交需求清单/甲/2026-11-20，B审核方案/乙/2026-11-21，C发布通知/丙/2026-11-23且依赖B，全部未完成。 |
| T02 | 第二成员报A完成，只改原A的done=true。 |
| T03 | 第二成员把原B截止改2026-11-22；C依赖、负责人、截止保留。 |
| T04 | Bot按最新清单生成说明文档，每行原text加“｜已完成/未完成”；A完成，B为11-22，C仍依赖B。 |
| T05 | 第二设备重开同清单和文档及历史后追问；正确回答A完成、B最新截止、C负责人/依赖，并从共享Chat历史指出最后修改B的成员。作者oracle来自T03实际消息/变更，不在prompt中提供。 |

逐项确切 `text/done`、prompt 和说明文档期望值见 fixture JSON。模型prompt只给业务请求和通用格式：F01不提供计算后的采购数，F02不提供重新计算的采购数，T05不提供具体完成状态/日期/负责人/依赖答案；这些结果只在grader oracle中。严格文本是这组确定性 fixture 的 oracle，不是对其他产品任务的语言限制。模型未遵循即记录失败，不在事后修改 fixture。证据索引保留实际已共享source的revision/digest、实际清单/文档与历史作者/task/时间顺序，不能用expected值充当实际观察。

输入 grader 的私有 observations 是数组，每行有 `caseId/items/taskId/state/sourceRef/participantRef/observedAt`。`items` 是实际 `readList` 返回的 items（保留 itemId）；`state` 来自 `getTask`。sourceRef 指向既有脱敏证据，不能用空值或虚构值。T04 加实际 `documentBody`，T05 加实际 `answer`。F05/T05 加 `differentDevice/historyVerified/sameArtifact=true`，这些是人工证据审查的断言，grader 本身不能证明硬件设备或真人身份。F05关联F03任务；T05关联追问任务。F03/F04加原 `command`；F04加 `before/after` 快照。`quotaCounts` 必须包含团队/成员/Bot UTC日计数，usage保留Missing/Partial，不能换成零。`operationIds`含所有写入ledger操作，historyDigest从完整分页历史规范化计算。

T05作者校验不依赖historyVerified布尔：T03 observation另附实际 `command`、已冻结second成员的 `participantIdentity:{uid,generation}`、正常sync返回的 `requestMessage:{id,author:{kind,uid,displayName},createdAt}` 和 `requestChange:{id,messageId,actorUid,generation,at}`。command的generation/messageId/changeId、消息作者、变更actorUid/generation必须一致，且确属T03这条改期请求。T05附从Chat历史实际取到的 `modifierSource:{messageId,changeId,actorUid,generation,at}`，须逐字段指向该T03变更，变更时间不晚于T03观察且早于T05观察。实际答案严格为基础事实“A已完成；B截止2026-11-22；C负责人丙，依赖B”加“；B最后修改者”及T03实际displayName或稳定uid。作者名称和UID仅在私有observation，公开grade只给caseId与失败代码。缺实际来源、错作者、旧changeId或倒置时序均失败，T05仍是原5例中的第5例。

观察原始输入可能含synthetic正文和身份映射，放现有受限私有QA目录；grader输出只含caseId/失败原因/分数，不抄正文或身份。每套独立5/5。原86项及C01–C30继续原runner（现有私有 `.omx/private/teams-m1-qa/` 路线，由原负责人定位具体入口）；此工具不读取该目录，不复制凭据，不判定旧C用例。C固定30例、至少27通过；F/T不填补旧C失败，不与C合并分母。

## 稳定窗口：真实HTTP＋现有runTeamHost旁证

批准范围是独立QA三维额度200/200/200、已有每task输出与物理调用限制、真实provider成功链路、144个固定probe。本工具没有窗口累计token/调用hard guard，不要求provider原始requestId，也不把Missing/Partial usage当失败或零费用。输入context/output/call limits可查，成本完整性与业务正确率分别记录。

实际HTTP adapter在 `apps/firebase/scripts/team-value-http-adapter.mjs`，直接调用当前 `POST {baseUrl}/v1/teams/messages`。原command原ID仅submit一次；getTask核对task/requester/Bot/Chat，sync完整分页按replyId/message.taskId/Bot作者取答案。token来自显式0600文件（纯ID token或 `{"idToken":"..."}`），逐请求重读支持外部轮换；程序调用也可传getToken刷新回调。禁止redirect、HTTP调用20s timeout，不输出token/HTTP响应正文/底层错误文本。

正常签名Desktop当前并未接入下述可选QA wrapper。正常执行优先使用同模块已实现的 `createExistingJournalReader`：无需修改connector/runtime，不调用模型，只读显式ownerRoot下既有文件。HTTP配置使用 `http.desktopJournal:{candidateFile,ownerRoot,initialTaskId}` 即可启用，直接交给已交付HTTP adapter，无需新私有模块。

- 从 `bindings/<sha256(JSON.stringify([scopeId,botId]))>` 读取当前发布binding，核对scope/Bot/revision，按binding.snapshot精确读取一个不可变 `snapshots/<digest>`，先验实际文件SHA，再只提取provider/model/protocol/endpoint origin；不扫描账号或凭据目录，不输出snapshot正文/profile/credential_id。
- 按唯一taskId读取 `runtime/results/<taskId>.json`：这是当前真实host在发出终态前写入的version3/type=result缓存。核对completed、cursor.scope中的Team/Bot/Chat、dispatchedRequests>0及现有物理调用限制；以实际文本digest匹配正常HTTP getTask/replyId/message.taskId/Bot作者和回答。
- `delivered()`会删 `executions/<taskId>` 与 `outbox/<taskId>`，不删上述缓存；因此ACK后仍能验收。两文件尚在时额外校验SavedExecution的scope/task/dispatched，以及SavedDelivery的task/outcome/文本digest且非synthesized；两文件消失本身不判失败。
- 已有缓存不含invocationId和provider requestId，均明确记录missing，不能补造；每task limits来源标为reviewed_fixed_connector_limits（W00已核对固定包的现有connector限制），不声称缓存记录了原bootstrap的limits。当前binary/process模块hash和发布snapshot route共同证明固定候选链路，不能单靠普通消息正文推断provider。
- 缓存缺失、scope错、零派发、snapshot/binary漂移会具体阻断；不自动触发run/continue或重跑模型。此reader只读现有normal Desktop结果，源码/fixture测试不构成已读取过实际签名host或已跑24h。

`apps/firebase/scripts/team-value-host-receipts.mjs` 已实现QA wrapper和只读reader，不要求一个尚不存在的ledger模块，不改runtime协议。最小接线是在现私有QA runner中把原 `runTeamHost(existingOptions)` 改为下列调用，或把recordedRun传给现QA connector的既有 `run` 注入点；运行仍由真实fixed process module导出的runTeamHost拥有：

```js
import { createHostReceiptRecorder } from '/ABSOLUTE/lingxi-app/apps/firebase/scripts/team-value-host-receipts.mjs';
const recordedRun = await createHostReceiptRecorder({
  candidateFile: '/absolute/private/frozen-candidate.json',
  receiptDir: '/absolute/private/p0-host-receipts',
});
const result = await recordedRun(existingRunTeamHostOptions);
// result不修改，按现逻辑保存SavedExecution/SavedDelivery并经正常completeTask交付。
```

wrapper自己导入已冻结processModule中的实际runTeamHost，不接受run函数或spawnProcess替身。`run/continue`核对实际host/worker哈希、scope/task/invocation/model route与现有limits后执行，原result原样返回；只记录脱敏sidecar。inspect类调用沿原真实实现，不冒充模型派发。`dispatchedRequests>0`、completed结果正文digest和同task的正常API回复对照，建立真实进程＋已核验provider route的成功链路旁证；这不冒称provider原始request receipt。原始providerRequestId缺失明确保存为missing/null。SavedExecution/SavedDelivery单独只有执行/交付状态，不能自行证明provider调用，故不扫描或复制其私有正文；原QA交付流程不变。

先用W00核对过的真实候选创建私有0600 frozen-candidate.json，示意结构如下；路径/哈希/revision均填写实际已审查候选，不要使用示例占位值运行：

```json
{
  "schemaVersion": 1,
  "provenance": "packaged",
  "scope": { "kind": "team", "id": "EXISTING_QA_TEAM_ID" },
  "binding": { "sharedBotId": "EXISTING_QA_BOT_ID", "conversationId": "EXISTING_QA_CHAT_ID" },
  "requester": { "uid": "ACTUAL_QA_MEMBER_UID", "generation": 1 },
  "isolatedQa": true,
  "package": { "revision": "ACTUAL_PACKAGE_REVISION", "sha256": "ACTUAL_PACKAGE_SHA256", "sharedBotRevision": 1, "evidenceRef": "reviewed-package-receipt" },
  "host": { "path": "/actual/fixed/harness-team-host", "sha256": "ACTUAL_HOST_SHA256" },
  "worker": { "path": "/actual/fixed/harness-team-worker", "sha256": "ACTUAL_WORKER_SHA256" },
  "processModule": { "path": "/absolute/private/actual-process.mjs", "sha256": "ACTUAL_PROCESS_MODULE_SHA256" },
  "modelRoute": { "provider": "ACTUAL_PROVIDER", "model": "ACTUAL_MODEL", "protocol": "ACTUAL_PROTOCOL", "origin": "https://ACTUAL_PROVIDER_ORIGIN" },
  "limits": { "context_window_tokens": 2000000, "output_tokens_per_request": 4096, "physical_dispatches": 32 }
}
```

processModule是现QA构建实际 `apps/electron/src/main/teams/process.ts` 得到的Node ESM模块，导出runTeamHost；可复用现私有case-runner同一个esbuild构建。它不包含一个mock host。hash用模块导出的sha256File读取真实文件计算，不读模型凭据。host/worker使用实际候选包中的固定binary，不由脚本构建、替换或下载。示例limits来自当前connector既有限制，操作者须核对实际调用bootstrap；wrapper只核对不扩大这些限制。共享模型route只存provider/model/protocol/endpoint origin，不保存credential_id、profile、账号、完整bootstrap、instructions、knowledge或原回复。候选文件不要附带凭据或业务正文。

若现QA没有单独process模块，可用已有Electron依赖构建该真实源码（只做本地QA bundle）：

```sh
node --input-type=module -e 'import {build} from "./apps/electron/node_modules/esbuild/lib/main.js"; await build({entryPoints:["apps/electron/src/main/teams/process.ts"],bundle:true,platform:"node",format:"esm",outfile:"/absolute/private/actual-process.mjs"})'
```

依赖或bundle失败时沿用现case-runner已验证的构建设置，不把synthetic process替换进去。该构建本身是源码证据；实际固定binary执行及返回结果另由wrapper记录，签名包归属仍用W00报告核对。

`provenance`区分packaged/source/fixture。真实source构建binary可记录真实进程证据，但不能宣称已做签名包验收；package.provenance会出现在最终receipt。只有真实进程证据可得live，fixture永为non-live。配置mode字符串不产生证据。候选/模块/receipt是受信QA证据链，须核对真实源码和W00候选归属，单靠自行填写哈希/重写本地receipt不能独立证明签名或provider服务端行为。

先在同scope/Bot/Chat/成员上通过正常Desktop执行一条真实任务，读取其既有cached结果作为初始provider链路核对；该任务不计入144。也可在现QA runner显式使用可选wrapper旁证，其HTTP配置改用hostReceipts:{receiptDir,initialTaskId}。正常Desktop稳定config为：

```json
{
  "runId": "qa_stable_0001",
  "startAt": 1794873600000,
  "binding": { "generation": 1, "conversationId": "EXISTING_QA_CHAT_ID", "sharedBotId": "EXISTING_QA_BOT_ID" },
  "http": {
    "baseUrl": "https://ACTUAL_TEAMS_API_BASE",
    "scope": { "kind": "team", "id": "EXISTING_QA_TEAM_ID" },
    "requesterUid": "ACTUAL_QA_MEMBER_UID",
    "tokenFile": "/absolute/private/rotating-id-token",
    "desktopJournal": { "candidateFile": "/absolute/private/frozen-candidate.json", "ownerRoot": "/actual/desktop/userData/shared-bot-host-v2/ACTUAL_OWNER_UID_SHA256", "initialTaskId": "ACTUAL_INITIAL_TASK_ID" }
  }
}
```

baseUrl使用现应用真实transport base，不带 `/v1/teams/messages`。startAt由操作者明确填写未来UTC毫秒，例值只说明格式。reader只读上述normal Desktop精确文件（可选wrapper模式才读专用receipt目录），对实际binary和processModule重新hash、逐task核对身份/route/正文digest；初始task另用实际getTask核对。缺文件时明确报DESKTOP_PUBLISHED_BINDING_UNAVAILABLE、PUBLISHED_SNAPSHOT_UNAVAILABLE_OR_CHANGED、DESKTOP_CACHED_TEAM_RUN_RESULT_UNAVAILABLE等（wrapper模式报ACTUAL_TASK_RECEIPT_UNAVAILABLE），并保留失败；不会凭空制造live receipt。任何故障/receipt缺失仍按原5分钟deadline处理，不改变144分母。

团队额度从实际listSharedBots读取三维limits及有效remaining最小值，必须200/200/200、开始remaining至少144；恢复时实际remaining须覆盖剩余待提交数。service caller公开接口只给user额度，若要测service scope，还需同已授权真实service侧三维额度只读记录；不能从用户额度猜其余额度。脚本不创建、恢复、发布、调额度或清理任何账号/Team/服务。没有可用QA身份、可读normal Desktop缓存或实际候选来源时是具体外部验收前提；正常Desktop无需接wrapper，不能以源码测试冒充已经运行。

CLI：

```sh
node apps/firebase/scripts/team-value-acceptance.mjs stable /absolute/private/stable-config.json /absolute/private/stable-checkpoint.json ./apps/firebase/scripts/team-value-http-adapter.mjs
```

先通过现发布流程共享下列稳定性资料和校验值，保存实际resource revision/digest。每次prompt只给probeId，不给checksum，要求读取共享资料：

```sh
node --input-type=module -e 'import {STABLE_SOURCE,SHARED_CHECKSUM} from "./apps/firebase/scripts/team-value-fixtures.mjs"; console.log(JSON.stringify({STABLE_SOURCE,SHARED_CHECKSUM},null,2))'
```

首slot=startAt，每600秒一个，总144；窗口到startAt+24h结束。每个槽固定为[scheduledAt,scheduledAt+10分钟)，槽内慢调度只记录dispatchDelayMs；1.1秒等正常延迟不判失败。整个槽漏过才记missed，不能跨槽补跑/缩分母/移动startAt或IDs。提交开始后5分钟内须观察到正确实际任务结果，严格JSON只含probeId和checksum，并有上述真实旁证与正物理调用计数。144/144且完整24h才通过。错probe/checksum、重复task/result、迟到、失败或缺真实来源均失败；provider原始requestId不存在或重复不单独判失败。

Missing/Partial usage原样记录，不当零，不使正确probe失败。所有已提交或准入结果不明的槽都在成本分母；失败且未取得usage记unknownTasks，不因仅一条complete而把其余143条未知汇成complete。receipt的usage.inputTokens/outputTokens和总dispatchedRequests在不完整时为null，knownInputTokens/knownOutputTokens/knownDispatchedRequests只是已报告下界；逐slot保留reported usage/costMicros/currency。dispatchedRequests来自实际TeamRunResult，不由费用字段猜测。最终总费用仍需覆盖失败/重试的原始账本；不能从144条正确结果推断总费用完整。

轮询30秒，单adapter调用55秒上限；SIGINT/SIGTERM保存后中止，同命令恢复。遗漏槽位失败，已提交只观察，崩溃停在submitting未知是否准入时记失败而不重提交。结果重复投递只计一次。断电留下的.lock需人工核对无活跃PID/写入者后仅移除本地锁再恢复；失败证据不删。检查点/receipt0600，只有IDs/时间/状态/digest/计数与非敏感来源引用，无消息/资料正文或credential。此次所有node:test旁证均为fixture，未执行真实24h窗口。

## 等待、恢复与七故障边界

继续复用当前 `pauseTask/answerWait/decideWait/continueTask`、operation ledger及既有fault-matrix测试/私有runner，不为通用工具能力增加越权清单。

真实模型人工提问和工具许可各等待至少30分钟；重启后核对同taskId/waitId/requester与membership generation、原问题/批准目标、deadline与检查点身份；用户回答/批准后只恢复原任务。完成结果丢ACK时只重交completeTask结果，不新开模型任务。

七边界按现有 `apps/firebase/test/fault-matrix.emulator.test.mjs` 定义，单独各10轮，保留失败：创建请求；取得permit；保存checkpoint；dispatch工具；收到工具结果；写最终结果；服务ACK。由原runner按现故障矩阵注入，不在稳定24h窗口中注入。human_input/tool_approval长等待与重启另测，不替换原七边界分母。每轮核对原operationId、清单恰好一次、最终task状态、quota与provider calls；queryable/unqueryable未知结果必须走现有reconciliation规则，不能臆测成功或直接重写。各边界10为固定分母；四类既有写入证据继续保留。不适用某工具的证据单列范围，不冒充具体模型/协议已测。

## W04 任务效果与维护研究

使用 [P0-TASK-OUTCOME-TEMPLATE.csv](P0-TASK-OUTCOME-TEMPLATE.csv) 和 [P0-MAINTENANCE-TEMPLATE.json](P0-MAINTENANCE-TEMPLATE.json)，只导出本人授权范围内的任务摘要，所有研究行以taskId关联。不同任务不按同一句话或标题合并。记录首次配置、后续维护、日常检查、人工接管分钟数；节省来自事前冻结的人工基线与实测主动耗时，允许负值。等待时间与主动维护分开，部分完成/部分接管保留分母，不删失败任务。

表格不收消息正文、文件内容、shared资料、账号、token或provider请求正文。真实sourceRef指向受限证据索引，不把私有内容复制入仓库。数值未知留空/null并注明Missing，不能填0。工程通过不代表研究收益成立；三家庭两团队七天及28天仍属后续研究门。
