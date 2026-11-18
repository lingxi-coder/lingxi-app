import type { DocPage } from './types';

const t = (en: string, zh: string) => ({ en, zh });
const sourceUrl = 'https://github.com/lingxi-coder/lingxi-app';

export const collaborationPages: DocPage[] = [
  {
    id: 'teams', group: 'product', sourceUrl,
    title: t('Teams, chats and messages', '团队、群聊与私信'),
    description: t('Create a team, start a conversation, send a message or work with a shared Bot.', '创建团队，开群聊、频道或私信，发送消息，和共享 Bot 一起工作。'),
    sources: [
      { repo: 'lingxi-app', path: 'packages/teams/TeamSidebar.tsx' },
      { repo: 'lingxi-app', path: 'packages/teams/NewConversationDialog.tsx' },
      { repo: 'lingxi-app', path: 'packages/teams/Composer.tsx' },
      { repo: 'lingxi-app', path: 'packages/teams/BotDirectory.tsx' },
      { repo: 'lingxi-app', path: 'packages/teams/AutoReplyDialog.tsx' },
      { repo: 'lingxi-app', path: 'apps/firebase/src/shared-bots.mjs' },
      { repo: 'lingxi-app', path: 'apps/electron/src/main/teams/replyTriage.ts' },
    ],
    sections: [
      { id: 'create-team', title: t('Create a team', '创建团队'), steps: [
        t('Sign in and open Teams. From the team switcher choose Create a team.', '登录后打开「团队」，在团队切换菜单中选择「创建团队」。'),
        t('Enter a name and confirm. The new team opens under your account.', '填写名称并确认。创建成功后会打开新团队。'),
      ], links: [{ label: t('Open Teams', '打开团队'), href: '/teams' }] },
      { id: 'invite', title: t('Invite a member or join a team', '邀请成员或加入团队'), paragraphs: [
        t('An owner or admin opens the team details, creates an invitation and shares its link. The recipient can open the link or paste the code under Join with an invite, sign in and confirm. An expired or revoked invitation needs replacing.', '团队所有者或管理员在团队详情里创建邀请并分享链接。接收者可以打开链接，也可以在「通过邀请加入」里粘贴邀请码，登录并确认。邀请过期或已撤销时，请对方重新邀请。'),
      ] },
      { id: 'chat-type', title: t('Choose a chat', '新建哪种对话'), paragraphs: [
        t('To chat privately with the LingXi assistant, use New Chat in the main workspace. Open a team to start a DM with a member, a group chat with several participants, a named channel, or a direct chat with a shared Bot. A conversation in Friends reaches the friend you select.', '想单独问灵犀助手，在主界面新建对话。想找团队成员说话，进入团队后选成员私信、多人群聊或有名称的频道；想找共享 Bot，选择 Bot 对话。联系好友可以从「好友」选人打开对话。'),
      ] },
      { id: 'group-chat', title: t('Start a group chat or channel', '创建群聊或频道'), steps: [
        t('Select the team. For a group, use the + beside Direct messages, choose New message → Group and select the people and any participating Bots.', '先选团队。建群时点「私信」旁的加号，在「新建私信」里选「群聊」，勾选成员以及需要参与的 Bot。'),
        t('Confirm to open the group. A group with other people can use a generated name. If you selected only Bots, enter a name. To create a named channel, use the + beside Channels, enter the channel name, choose its participants and confirm.', '确认后打开群聊。有其他真人成员时可沿用自动生成的群名。只选了 Bot 时需要填写群名。建频道请点「频道」旁的加号，输入频道名称，选择参与者并确认。'),
      ] },
      { id: 'direct-message', title: t('Start a direct message (DM)', '发起成员私聊（DM）'), paragraphs: [
        t('In the team, choose Direct messages → New message → Direct message and select one current member. An existing DM with that person opens again. For a friend outside the team, open the conversation in Friends. To start a personal conversation with the LingXi assistant, use New Chat.', '在团队里打开「私信 → 新建私信 → 私信」，选择一名当前成员。双方已有私信时会重新打开原对话。联系团队外的好友可以打开「好友」里的对话。想与灵犀助手单独交谈，则使用主界面的新对话入口。'),
      ], links: [{ label: t('Open Friends', '打开好友'), href: '/friends' }] },
      { id: 'send-message', title: t('Send a message or attachment', '发送消息或附件'), paragraphs: [
        t('Open the intended DM, group or channel. Type a message or attach a file, wait for any uploads and choose Send. On Web and Desktop, Enter sends and Shift+Enter inserts a new line. For a reply to a particular message use Reply on that message.', '打开目标私信、群聊或频道，输入文字或添加文件，等附件上传完成后点发送。Web 和桌面端按 Enter 发送，Shift+Enter 换行。要回复某条消息，可在那条消息上选择「回复」。'),
        t('If a message is pending, reconnect and check the original entry. Web and Desktop retry temporary failures automatically. If the entry shows Retry, use that button. A delivered message and a completed Bot task have separate statuses.', '遇到待发送状态，重新连接后查看原消息。Web 和桌面端会自动重试临时失败的消息。原记录上出现「重试」按钮时再点。消息送达后，可在 Bot 任务卡里另行查看执行结果。'),
      ] },
      { id: 'bot-chat', title: t('Message a Bot directly or address it in a group', '与 Bot 私聊或在群里点名'), paragraphs: [
        t('Use the + beside Bot chats, select an available shared Bot and open the chat. A Bot DM already addresses the Bot. In a group or channel, type @ and pick a participating Bot from the suggestions to request work from it explicitly.', '在「Bot 对话」旁点加号，选择可用的共享 Bot 并打开对话。Bot 私聊里的消息直接发给它，无需 @。在群聊或频道里要明确派给某个 Bot，可以输入 @ 并从候选列表中点选它。'),
      ], links: [{ label: t('Let a Bot respond to ordinary messages', '让 Bot 自主判断普通消息是否需要回复'), href: '/docs/teams#auto-reply' }] },
      { id: 'auto-reply', title: t('Let a Bot decide whether to reply', '让 Bot 自主判断是否回复'), steps: [
        t('In this shared Bot’s publication settings on its Mac, enable “Let the Bot decide when to answer”, set its daily limit and confirm publication. This option is off by default for each Bot. If unavailable, configure Settings → Models and services → Decision on the publishing Mac first.', '在发布这一个共享 Bot 的 Mac 上，在它的发布设置中勾选「允许 Bot 自主判断是否回复」，设置每日上限并确认发布。每个 Bot 的这个选项默认关闭。如果无法勾选，先到发布端的「设置 → 模型与服务 → Decision」配置判断服务。'),
        t('In the group, channel or Bot DM, a participant opens “Bots that answer on their own”, selects the participating Bot and saves. This setting applies to that conversation.', '参与者在群聊、频道或 Bot 私聊中打开「自主回复的 Bot」，选中该 Bot 后保存。这个设置只作用于当前对话。'),
        t('Send a normal group or channel message without an @. An enabled, available Bot can answer or stay silent. With either switch off, an ordinary unaddressed group message does not start a task for it. A Bot DM already makes a direct request. Enabling this option there lets it leave an acknowledgement unanswered.', '在群聊或频道里直接发一条普通消息，无需 @。满足配置且可用的 Bot 可以回复，也可以保持安静。Bot 或当前会话的开关关闭时，未点名的普通群消息不会触发它。Bot 私聊本身就是直接请求。在私聊里开启自主判断后，它可以选择不再回复一句「谢谢」。'),
      ] },
      { id: 'share-bot', title: t('Share your Bot with a team', '把 Bot 分享给团队'), steps: [
        t('On the Mac hosting the Bot, open the team details and its Bots. Choose Publish a Desktop Bot and select your local Bot.', '在运行该 Bot 的 Mac 上打开团队详情和 Bot 目录，选择「发布 Desktop Bot」，选中本机 Bot。'),
        t('Set its role, members allowed to use it, files and limits. Review the audience and materials, then confirm. A permitted member can now open its Bot chat while the publishing host is available.', '设置角色、可使用的成员、资料和额度，复核共享范围并确认。宿主可用时，获准的成员即可打开这个 Bot 的对话。'),
      ], links: [{ label: t('Other ways to share a Bot', '其他 Bot 共享方式'), href: '/docs/bots#audience' }] },
    ],
  },
  {
    id: 'bots', group: 'product', sourceUrl,
    title: t('Personal and shared Bots', '个人与共享 Bot'),
    description: t('Create a persistent assistant, choose who can use it and check a missing reply.', '创建持久助手，选择谁能使用，排查 Bot 没回复的原因。'),
    sources: [
      { repo: 'lingxi-app', path: 'apps/electron/src/renderer/bridge/botCreation.ts' },
      { repo: 'lingxi-app', path: 'packages/teams/FriendBotsPanel.tsx' },
      { repo: 'lingxi-app', path: 'packages/teams/HostedServices.tsx' },
      { repo: 'lingxi-app', path: 'apps/electron/src/main/bots/marketHost.ts' },
    ],
    sections: [
      { id: 'create', title: t('Create a personal Bot', '创建个人 Bot'), paragraphs: [
        t('In Chat or Code enter /create-bot followed by what the assistant should do. Review its proposed name, description and role. On Mac choose Review & create, check the model and permissions and confirm Create Bot. On clients without that card, copy the draft into the manual form in Desktop → Bots → New Bot.', '在 Chat 或 Code 输入 /create-bot，再描述助手要做的事。核对草案里的名字、简介和角色。Mac 上打开审核创建卡片，检查模型与权限，再确认创建。当前客户端没有卡片时，可将草案复制到桌面版「Bot → 新建 Bot」的手动表单。'),
      ] },
      { id: 'audience', title: t('Choose who uses the Bot', '选择 Bot 的使用对象'), bullets: [
        t('Your own second device: enable remote access for the personal Bot on the Mac and open it from a paired device.', '自己的另一台设备：在 Mac 上为个人 Bot 开启远程访问，从已配对设备打开。'),
        t('One friend or one team: use Shared Bots in the friend chat or publish to the team’s Bot directory. The Bot runs on the maintainer’s Mac.', '一位好友或一个团队：在好友对话的共享 Bot 面板中分享，或发布到团队的 Bot 目录。Bot 运行在维护者的 Mac 上。'),
        t('For people using a hosted service, set its audience in Bot services. For someone who wants their own copy and model, share the Bot Market listing.', '希望别人使用你托管的服务，可以在 Bot 服务里选受众。希望对方安装副本、使用自己的模型，可以分享 Bot 市场条目。'),
      ], links: [{ label: t('Publish to a team', '发布给团队'), href: '/docs/teams#share-bot' }, { label: t('Open Bot services', '打开 Bot 服务'), href: '/services' }] },
      { id: 'no-reply', title: t('When a Bot does not reply', 'Bot 不回消息时'), paragraphs: [
        t('Check the conversation first. In a team, check the published Bot’s availability, your access, remaining quota and any waiting task card. For an unaddressed group message, check this Bot’s auto-reply publication option and then the conversation switch. Even with both enabled it may choose silence. For a personal Bot opened remotely, check the Mac and the Bot’s remote access.', '先确认是哪种对话。团队里先看 Bot 的发布状态、宿主是否在线、自己的权限和剩余额度，再看任务卡是否在等人回答。若发的是未点名的群消息，先查这个 Bot 的自主回复发布选项，再查本对话的开关。两者都开启时它仍可能选择沉默。远程打开个人 Bot 时则检查 Mac 和该 Bot 的远程访问开关。'),
      ], links: [{ label: t('Auto-reply settings', '自主回复设置'), href: '/docs/teams#auto-reply' }, { label: t('Remote connection checks', '检查远控连接'), href: '/docs/remote-control#direct' }] },
    ],
  },
  {
    id: 'local-apps', group: 'product', sourceUrl,
    title: t('Publish and share Local Apps', '发布和共享 Local App'),
    description: t('Release to your Apps library, share an installable copy or use your running desktop app from a paired device.', '发布到应用库，让别人安装副本，或从自己的已配对设备使用桌面上的同一份应用。'),
    sources: [
      { repo: 'harness-runtime', path: 'crates/local-app/assets/skill/SKILL.md' },
      { repo: 'harness-runtime', path: 'crates/skill/src/builtin/lingxi-api/references/local-app.md' },
      { repo: 'lingxi-app', path: 'apps/electron/src/renderer/components/LocalApps.tsx' },
      { repo: 'lingxi-app', path: 'apps/electron/src/renderer/components/LocalAppPublish.tsx' },
      { repo: 'lingxi-app', path: 'apps/electron/src/main/local-apps/remoteGateway.ts' },
      { repo: 'lingxi-app', path: 'apps/ios/native/Sources/Apps/AppsPages.swift' },
      { repo: 'lingxi-app', path: 'apps/android/native/app/src/main/java/com/lingxi/code/apps/UnifiedAppsPages.kt' },
      { repo: 'lingxi-app', path: 'packages/teams/attachments.ts' },
      { repo: 'lingxi-app', path: 'docs/LOCAL_APP_REMOTE_ACCESS.md' },
    ],
    sections: [
      { id: 'publish-local', title: t('Publish a local release', '发布到本机应用库'), steps: [
        t('In Code mode open the source project on the host you want to use. Ask LingXi to build or update the Local App and publish it into Apps. The local-app skill guides the release through the available LocalApp tool.', '在要运行应用的设备上进入 Code 模式，打开源项目，请灵犀制作或更新 Local App 并发布到「应用」。local-app 技能会按当前宿主提供的 LocalApp 工具完成发布。'),
        t('Open the released revision from Apps and check that it runs. For an update, open Modify on the existing app so the new release uses the same app ID. If the revision changed elsewhere, review the newer source before publishing.', '在「应用」中打开刚发布的版本，确认它能运行。更新时从原应用的「修改」进入，沿用同一应用 ID。如其他地方已发布新版本，先核对最新源码再发布。'),
      ] },
      { id: 'share-options', title: t('Choose how to share a Local App', '选择 Local App 的共享方式'), bullets: [
        t('For anyone to install a copy, make the market listing public and share its link.', '要让大家都能安装副本，在市场上架应用后分享详情页链接。'),
        t('To give the source to one person, share a source ZIP. The Export source ZIP entry is available only on iOS and Android.', '只想把源码给指定的人，可以私下发送源码 ZIP。「导出源码 ZIP」入口仅在 iOS 和 Android 提供。'),
        t('To use an app running on your own computer, open it from a paired device. You can continue working with records saved by its backend.', '要从自己的另一台设备继续使用电脑上的应用，通过已配对设备打开，就能继续使用应用后端保存的记录。'),
      ], links: [{ label: t('Publish a market listing', '上架应用并分享链接'), href: '/docs/local-apps#market' }, { label: t('Share a source ZIP privately', '单独分享源码 ZIP'), href: '/docs/local-apps#source-zip' }, { label: t('Open the running app', '访问正在运行的应用'), href: '/docs/local-apps#paired-devices' }] },
      { id: 'market', title: t('Publish to the market and send the listing', '发布到市场并发送链接'), steps: [
        t('Sign in on the device that holds the release and enter Code mode. On Desktop, choose Apps → the app’s ••• menu → Publish to market, fill in the listing and release notes, then choose Review source and publish. On iOS or Android, open the app’s details and choose Publish to market. Review the source files and confirm the upload.', '在保存这个版本的设备上登录并切到 Code 模式。桌面版从「应用 → 目标应用的 ••• → Publish to market」进入，填写条目与版本说明，点「Review source and publish」。iOS 和 Android 从「应用 → 应用详情 → 发布到市场」进入。核对要上传的源码文件后确认提交。'),
        t('A new listing starts hidden. Open the publication manager on the publishing device and check the release status. On Desktop this is Manage publications. After approval, choose Make public. Anyone can now open the listing.', '新条目默认隐藏。在发布设备上打开市场管理页查看版本状态。桌面版入口是「Manage publications」，iOS 是「管理市场发行与审核」，Android 是「管理市场发布」。审核通过后点「上架应用」。此时所有人都可以打开该条目。'),
        t('Open the public market detail page and send its link in the intended DM or group. The recipient reviews the release and installs a compatible copy with their own permissions and data.', '打开已上架应用的市场详情页，把链接发到目标私信或群聊。接收者查看版本并在兼容设备上安装，使用自己的权限和数据。'),
      ], links: [{ label: t('Send a team message', '发送团队消息'), href: '/docs/teams#send-message' }] },
      { id: 'source-zip', title: t('Give someone a source ZIP privately', '单独分享源码 ZIP'), steps: [
        t('The Export source ZIP entry is available only on iOS and Android. On the device that holds the release, open Apps → app details → Export source ZIP and save the file. The archive contains the release source. Runtime databases, browser data and permissions stay with the original installation.', '「导出源码 ZIP」入口仅在 iOS 和 Android 提供。在保存该版本的移动设备上进入「应用 → 应用详情 → 导出源码 ZIP」并保存文件。压缩包包含发布版本的源码。运行数据库、浏览器数据和授权保存在原设备上。'),
        t('Send the ZIP privately using a channel that accepts it, such as AirDrop or email. To share through a LingXi team DM, send a download link restricted to that person. Team attachments accept PNG, JPEG, PDF and plain text.', '通过隔空投送或邮件等支持 ZIP 的方式单独发送。要在灵犀团队私信里分享，可以发送仅接收者可访问的下载链接。团队附件支持 PNG、JPEG、PDF 和纯文本。'),
        t('On iOS in Code mode, open Apps → + → Import source folder and select the ZIP. On Android in Code mode, choose Import source in Apps and select the ZIP. Open the imported app from its details.', '接收者在 iOS 的 Code 模式打开「应用 → + → 导入源码文件夹」并选择 ZIP。在 Android 的 Code 模式打开「应用 → 导入源码」，选择 ZIP。导入完成后，在应用详情打开本机副本。'),
      ] },
      { id: 'paired-devices', title: t('Open the same running app on your own device', '在自己的设备访问同一运行实例'), steps: [
        t('On Desktop configure the remote app gateway and enable Remote Control. In Apps → •••, turn on Allow access from paired devices for this app.', '先在桌面版配置远程应用网关并开启远控。在「应用」中打开目标应用的 •••，勾选允许已配对设备访问。'),
        t('With the same account, open Remote Control, pair with the computer and select the app under Apps. It uses the same backend and the records that backend saves. To stop access, turn off this app’s remote-access setting on Desktop.', '在自己的另一台设备用同一账号打开远控，完成配对，在所连电脑的「应用」里打开它。访问的是同一后端，可以继续使用后端保存的记录。要停止访问，在桌面版关闭该应用的远程访问。'),
        t('Each remote opening gets a new browser origin. Cookies, localStorage and IndexedDB are isolated for that opening. If an app saves a counter only in the browser, reopening it on the same phone shows the initial count.', '每次远程打开都会获得新的浏览器源。Cookie、localStorage 和 IndexedDB 都按这次打开隔离。如果应用只把计数存在浏览器里，在同一手机上重新打开也会看到初始值。'),
      ], links: [{ label: t('Pair a device', '配对设备'), href: '/docs/remote-control#same-account' }] },
    ],
  },
];
