# 团队、群聊与私信

创建团队，开群聊、频道或私信，发送消息，和共享 Bot 一起工作。

<a id="create-team"></a>

## 创建团队

1. 登录后打开「团队」，在团队切换菜单中选择「创建团队」。
2. 填写名称并确认。创建成功后会打开新团队。

- [打开团队](https://lingxios.net/teams)

<a id="invite"></a>

## 邀请成员或加入团队

团队所有者或管理员在团队详情里创建邀请并分享链接。接收者可以打开链接，也可以在「通过邀请加入」里粘贴邀请码，登录并确认。邀请过期或已撤销时，请对方重新邀请。

<a id="roles"></a>

## 角色、所有权与退出团队

- 所有者：拥有管理员的全部权限，还可以删除团队、设置或取消管理员，以及「设为所有者」。转让后原所有者保留管理员身份。唯一的所有者不能退出团队。
- 管理员：创建和撤销邀请、邀请好友、移除普通成员。成员：使用有权限的对话和共享 Bot，也可以「退出团队」。
- 邀请链接可用 1–50 次，有效期 1 小时到 7 天。退出或被移除后将无法访问团队对话和共享 Bot，重新加入需要新邀请。「删除团队」会让所有人失去访问权限，且无法撤销。每个团队最多 200 名成员，每人最多加入 20 个团队。

<a id="chat-type"></a>

## 新建哪种对话

想单独问灵犀助手，在主界面新建对话。想找团队成员说话，进入团队后选成员私信、多人群聊或有名称的频道；想找共享 Bot，选择 Bot 对话。联系好友可以从「好友」选人打开对话。

<a id="group-chat"></a>

## 创建群聊或频道

1. 先选团队。建群时点「私信」旁的加号，在「新建私信」里选「群聊」，勾选成员以及需要参与的 Bot。
2. 确认后打开群聊。有其他真人成员时可沿用自动生成的群名。只选了 Bot 时需要填写群名。建频道请点「频道」旁的加号，输入频道名称，选择参与者并确认。

<a id="direct-message"></a>

## 发起成员私聊（DM）

在团队里打开「私信 → 新建私信 → 私信」，选择一名当前成员。双方已有私信时会重新打开原对话。联系团队外的好友可以打开「好友」里的对话。想与灵犀助手单独交谈，则使用主界面的新对话入口。

- [打开好友](https://lingxios.net/friends)

<a id="send-message"></a>

## 发送消息或附件

打开目标私信、群聊或频道，输入文字或添加文件，等附件上传完成后点发送。Web 和桌面端按 Enter 发送，Shift+Enter 换行。要回复某条消息，可在那条消息上选择「回复」。

遇到待发送状态，重新连接后查看原消息。Web 和桌面端会自动重试临时失败的消息。原记录上出现「重试」按钮时再点。消息送达后，可在 Bot 任务卡里另行查看执行结果。

<a id="bot-chat"></a>

## 与 Bot 私聊或在群里点名

在「Bot 对话」旁点加号，选择可用的共享 Bot 并打开对话。Bot 私聊里的消息直接发给它，无需 @。在群聊或频道里要明确派给某个 Bot，可以输入 @ 并从候选列表中点选它。

- [让 Bot 自主判断普通消息是否需要回复](teams.md#auto-reply)

<a id="auto-reply"></a>

## 让 Bot 自主判断是否回复

1. 在发布这一个共享 Bot 的 Mac 上，在它的发布设置中勾选「允许 Bot 自主判断是否回复」，设置每日上限并确认发布。每个 Bot 的这个选项默认关闭。如果无法勾选，先到发布端的「设置 → 模型与服务 → Decision」配置判断服务。
2. 参与者在群聊、频道或 Bot 私聊中打开「自主回复的 Bot」，选中该 Bot 后保存。这个设置只作用于当前对话。群聊和频道里，同一个窗口还有两项设置：「每条消息最多几个 Bot 自主回复」（1 到 5，默认 1）和「让 Bot 也判断只 @ 了成员的消息」（默认关闭）。
3. 在群聊或频道里直接发一条普通消息，无需 @。满足配置且可用的 Bot 可以回复，也可以保持安静。Bot 或当前会话的开关关闭时，未点名的普通群消息不会触发它。Bot 私聊本身就是直接请求。在私聊里开启自主判断后，它可以选择不再回复一句「谢谢」。

- [多个 Bot 都能回复时由谁回复](teams.md#reply-rounds)
- [哪些消息会让 Bot 回复](teams.md#bot-mentions)

<a id="reply-rounds"></a>

## 多个 Bot 都能回复时由谁回复

在群聊或频道里，没有点名任何 Bot 的消息，不再由每个 Bot 各自决定要不要回。服务会开启一个约 15 秒的轮次：本对话中开启了自主回复的 Bot 各自报告对「需要回复」有多大把握，服务挑出把握最大的几个，人数不超过「每条消息最多几个 Bot 自主回复」（默认 1）。没被选中的 Bot 不会运行，也不占用额度。有人发出更新的消息时，尚未结束的轮次即作废；在轮次内没有回应的 Desktop 不参与本轮。

Bot 按把握程度排序：被直接对话的最先，其次是命中了自己关键词的，其次是自己判断需要回复的，最后是愿意回复但可以选择不回的。不同 Desktop 上的 Bot 先比较这个等级。不同维护者的判断服务给出的概率不直接比较，只用来给同一等级的 Bot 排先后，仍相同时按固定顺序决定。自己判断「不需要回复」的 Bot 不会回复。被选中的 Bot 约 20 秒后仍没有开始时，由排名下一位的 Bot 顶替，只顶替一次。

<a id="bot-mentions"></a>

## 哪些消息会让 Bot 回复

- @ 了某个 Bot：由它回复，其他 Bot 不会插话，不管它们怎么设置。在群里它不能选择沉默。
- @ 了几个 Bot：每个都按各自的角色回复，并知道还有哪些 Bot 被问到。
- 没有 @，但回复了某个 Bot 的消息，或在文字里叫了它的名字：视为在对这个 Bot 说话。除非消息明显不需要回答，否则它会回复。
- 只 @ 了成员：Bot 保持安静，除非本对话开启了「让 Bot 也判断只 @ 了成员的消息」。开启后，这条消息和普通消息一样参与轮次。
- 既没有 @ 也没有叫名字：本对话中开启了自主回复的 Bot 参与轮次。
- 为 Bot 设置的安静时段和冷却时间只在它自行判断时生效，不适用于点名它的消息。
- Bot 所在的 Desktop 离线：在群聊或频道里消息照常发出，对话里会显示该 Bot 已离线、上线后会回复。Desktop 回来后 Bot 再回复，最长等一天，超过一天任务卡显示已过期。Bot 私聊和托管的 Bot 服务在宿主离线时不接收消息。
- 编辑消息不会通知 Bot，因为编辑不会产生新的 Bot 任务。要请求 Bot，请另发一条消息。撤回消息会结束它尚未结束的轮次，并取消仍在排队的 Bot 请求。
- Bot 写出的文字不会让别的 Bot 回复，也不会开启轮次，文字里的 @ 不会触发任何事。Bot 只能通过下一节说明的委托去请求另一个 Bot。

<a id="bot-delegation"></a>

## 让 Bot 请另一个 Bot 帮忙

在群聊或频道里，正在处理你请求的 Bot 可以把其中一部分交给同一对话里的另一个 Bot。这是它工作中的一个步骤，并不是因为它的文字里写了 @。它可以等对方回复后接着做，也可以让对方直接在对话里回答。

双方 Bot 都必须同意。被请求的 Bot 发布时决定接受谁的请求：不接受（默认）、团队中的任何 Bot，或只接受维护者列出的 Bot。发起请求的 Bot 发布时列出它可以请求的 Bot，并设置每日上限。这些只能在发布它们的 Mac 上设置，做法见「个人与共享 Bot」。其他成员只能在 Bot 目录和 Bot 详情里查看，对应「接受谁的委托」和「可委托给」，不能修改。

你自己的权限同样适用。对方 Bot 是在为发出消息的你工作：你必须有权使用它，和你直接给它发消息时一样；它只能使用共享给它的内容，不会用到第一个 Bot 能用的内容。

- 最多传两层：从你的消息算起最多两跳（你 → Planner → Travel → Chef，Chef 不能再请求别人），也不会再请求链上已有的 Bot。
- 等待回复：第一个 Bot 的任务卡显示「等待 Travel 回复」，不需要任何人回答。对方 Bot 的任务卡显示链条「委托：Planner → Travel」。第一个 Bot 最多等十分钟；之后它会在没有对方回复的情况下继续并如实说明，对方回复好后仍会发到对话里。
- 直接回复：第一个 Bot 不需要对方的答案时，只发出请求，不会等待，由对方 Bot 自己把回复发到对话里。
- 对方 Bot 所在的 Desktop 离线时，请求最多保留一天，它的任务卡会显示该 Bot「已离线，上线后会回复」。第一个 Bot 仍然最多只等十分钟。
- 取消：你、被请求 Bot 的维护者，以及发起请求的 Bot 的维护者，都可以在对方 Bot 的任务卡上取消它。取消会同时停止由它引出的其他任务，正在等它的 Bot 会得知请求已被取消，然后继续。取消第一个 Bot 那张等待中的任务卡，也会取消它对另一个 Bot 的请求。
- Bot 的自主回复记录里，来自另一个 Bot 的请求会标注「由 Planner 委托」。

- [发布可以互相请求的 Bot](bots.md#delegation)

<a id="share-bot"></a>

## 把 Bot 分享给团队

1. 在运行该 Bot 的 Mac 上打开团队详情和 Bot 目录，选择「发布 Desktop Bot」，选中本机 Bot。
2. 设置角色、可使用的成员、资料和额度，复核共享范围并确认。宿主可用时，获准的成员即可打开这个 Bot 的对话。

- [其他 Bot 共享方式](bots.md#audience)

## 源码与文档基线

[源码仓库](https://github.com/lingxi-coder/lingxi-app)

- [lingxi-app/packages/teams/TeamSidebar.tsx](https://github.com/lingxi-coder/lingxi-app/blob/7fecdf10d044c94cbebbcbb4511ebede0c3c97ed/packages/teams/TeamSidebar.tsx) — 已提交源码 (7fecdf10d044c94cbebbcbb4511ebede0c3c97ed)
- [lingxi-app/packages/teams/NewConversationDialog.tsx](https://github.com/lingxi-coder/lingxi-app/blob/a3ecab5a900bae02175bfc9c2bc4507d8f2e6659/packages/teams/NewConversationDialog.tsx) — 已提交源码 (a3ecab5a900bae02175bfc9c2bc4507d8f2e6659)
- [lingxi-app/packages/teams/Composer.tsx](https://github.com/lingxi-coder/lingxi-app/blob/b9841c6515f9716c0149ec13c487b3779bf25167/packages/teams/Composer.tsx) — 已提交源码 (b9841c6515f9716c0149ec13c487b3779bf25167)
- [lingxi-app/packages/teams/BotDirectory.tsx](https://github.com/lingxi-coder/lingxi-app/blob/c0eeb5af8aad9e218c510597a301176cf8db53e2/packages/teams/BotDirectory.tsx) — 已提交源码 (c0eeb5af8aad9e218c510597a301176cf8db53e2)
- [lingxi-app/packages/teams/AutoReplyDialog.tsx](https://github.com/lingxi-coder/lingxi-app/blob/c00607ed4841d7d032a571105471e9c8b7c0b6ce/packages/teams/AutoReplyDialog.tsx) — 已提交源码 (c00607ed4841d7d032a571105471e9c8b7c0b6ce)
- [lingxi-app/apps/firebase/src/shared-bots.mjs](https://github.com/lingxi-coder/lingxi-app/blob/b9841c6515f9716c0149ec13c487b3779bf25167/apps/firebase/src/shared-bots.mjs) — 已提交源码 (b9841c6515f9716c0149ec13c487b3779bf25167)
- [lingxi-app/apps/electron/src/main/teams/replyTriage.ts](https://github.com/lingxi-coder/lingxi-app/blob/c00607ed4841d7d032a571105471e9c8b7c0b6ce/apps/electron/src/main/teams/replyTriage.ts) — 已提交源码 (c00607ed4841d7d032a571105471e9c8b7c0b6ce)

- lingxi-app: HEAD e74379a16bb1946787a7edf327cd8367128c4357; 所据文件指纹 sha256:b2d7aabb4ca87d35400a12fe665e0a43040123560edaadd23961a53bf191fc66

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
