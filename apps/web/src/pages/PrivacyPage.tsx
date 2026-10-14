import { RouteLink } from '../components/RouteLink';
import type { Locale } from '../utils/locale';
import './privacy.css';

export const privacyContact = 'support.lingxi@gmail.com';
export const privacyUpdated = '2026-10-14';

const sections = [
  {
    id: 'scope', en: 'Who we are and what this covers', zh: '适用范围与开发者',
    enText: 'This policy describes how LingXi (灵犀), developed by the LingXi organization, handles information in its desktop, iOS and Android applications and on lingxios.net. Available features vary by platform and distribution. You can use configured AI providers independently of a LingXi account. This policy does not replace the policies of providers or services you choose to connect.',
    zhText: '本政策说明 LingXi（灵犀）组织开发的桌面、iOS、Android 应用及 lingxios.net 如何处理信息。不同平台与发行渠道的功能可能不同。你可以独立配置 AI 服务提供商，无需将其视为灵犀账号的一部分。本政策不替代你选择连接的提供商或服务的隐私政策。',
  },
  {
    id: 'accounts', en: 'Account information', zh: '账号信息',
    enText: 'When you create or sign in to a LingXi account, Firebase Authentication (Google) processes your email, account identifier, authentication credentials, and verification status. Google or Apple sign-in also supplies the profile information you authorize, such as name, email and provider identifier. Authentication services receive network and device information needed to operate and secure sign-in. Tokens are retained to maintain your session. Signing out ends the local session; it does not delete the account.',
    zhText: '注册或登录灵犀账号时，Firebase Authentication（Google）处理邮箱、账号标识、认证凭据及验证状态。使用 Google 或 Apple 登录时，还会提供你授权的姓名、邮箱和提供商标识等资料。认证服务会接收用于运行及保护登录流程的网络和设备信息。应用保留令牌以维持登录状态。退出登录会结束本地会话，但不会删除账号。',
  },
  {
    id: 'ai', en: 'Conversations, files and AI services', zh: '对话、文件与 AI 服务',
    enText: 'Conversations, project paths, settings and working files are stored locally as part of the app’s operation. To carry out a task, your selected AI endpoint may receive prompts, conversation context, selected attachments, relevant file contents, tool results and images. Requests use your configured API key or authorization token; credentials are sent to the endpoint for authentication. Custom endpoints, proxies, multi-provider strategies, remote bridges, MCP servers, plugins and local apps can introduce additional recipients. Review their access and policies before enabling them. LingXi does not operate its own model-training pipeline for your conversations; each connected provider controls its own retention and training practices.',
    zhText: '应用运行过程中会在本地保存对话、项目路径、设置及工作文件。执行任务时，你选择的 AI 端点可能接收提示词、对话上下文、选定附件、相关文件内容、工具结果和图片。请求使用你配置的 API 密钥或授权令牌，凭据会发送至对应端点进行认证。自定义端点、代理、多提供商策略、远程桥接、MCP 服务器、插件及本地应用可能引入额外接收方。启用前请核对其访问范围和政策。灵犀没有将对话用于自有模型训练的流程；各接入提供商自行决定其数据保留和训练规则。',
  },
  {
    id: 'permissions', en: 'Optional device access', zh: '可选的设备权限',
    enText: 'Microphone access supports voice input and audio conversations. Speech may be processed by local models, operating-system speech services or your configured cloud audio provider; system recognition can use a network service. Camera and photo access support images you choose to analyze. When supported and authorized, tools or local apps can access location, contacts and calendar events for your requested task. Notifications and scheduled-task permissions support reminders and background work. Desktop automation or Android accessibility features, where available, can read screen content and perform actions you authorize; captured content may become AI context. You can deny or revoke permissions in system settings. Enabling a permission does not require enabling every related feature.',
    zhText: '麦克风用于语音输入和音频对话。语音可由本地模型、操作系统语音服务或你配置的云端音频提供商处理；系统语音识别可能使用网络服务。相机和相册用于分析你选择的图片。在功能支持且获得授权时，工具或本地应用可为你请求的任务读取位置、联系人和日历。通知与定时任务权限用于提醒和后台执行。支持的桌面自动化或 Android 无障碍功能可以读取屏幕内容并执行你授权的操作，捕获的内容可能成为 AI 上下文。你可以在系统设置中拒绝或撤销权限。授权某项权限不代表必须启用所有相关功能。',
  },
  {
    id: 'sharing', en: 'Recipients and international processing', zh: '接收方与跨境处理',
    enText: 'Recipients depend on the features you use: Google/Firebase for account authentication, Apple or Google for their sign-in flows, your selected AI/search/audio providers, operating-system services, and connected tools or services. These services can process information in countries outside your own. Website hosting services receive ordinary HTTP request information, such as IP address and user agent, to deliver and protect the site. Support receives information you include in an email; do not send passwords, API keys or private project files. Consult the linked provider policies and your endpoint configuration before sending sensitive information.',
    zhText: '接收方由你使用的功能决定：Google/Firebase 用于账号认证，Apple 或 Google 用于对应登录流程，以及你选择的 AI、搜索、音频提供商、操作系统服务和连接的工具或服务。这些服务可能在你所在国家或地区以外处理信息。网站托管服务接收 IP 地址、浏览器标识等常规 HTTP 请求信息，用于交付和保护网站。支持邮箱会接收你邮件中提供的信息，请勿发送密码、API 密钥或私有项目文件。提交敏感信息前请查看对应提供商政策及端点配置。',
  },
  {
    id: 'retention', en: 'Storage, retention and security', zh: '保存、保留与安全',
    enText: 'Local app data remains until you remove it using available app controls or delete the app’s data; exported files and workspace files can remain separately. Backups may retain additional copies. Account records remain with the authentication service until account deletion is completed. Third-party request data follows each recipient’s retention policy. Support correspondence is kept while handling your request and as needed for follow-up or applicable obligations. Desktop account tokens use operating-system protected storage; protection of other local settings and credentials depends on the platform and configuration. No storage or transmission system can guarantee absolute security.',
    zhText: '本地应用数据会保留到你通过可用的应用功能移除或清除应用数据；导出文件与工作区文件可能独立保留，备份也可能保存副本。认证服务中的账号记录会保留至账号删除完成。第三方请求数据适用对应接收方的保留规则。支持往来邮件会在处理请求期间及必要的跟进或适用义务范围内保留。桌面账号令牌使用操作系统保护的存储；其他本地设置和凭据的保护方式取决于平台及配置。任何存储或传输系统都无法保证绝对安全。',
  },
  {
    id: 'choices', en: 'Your choices and deletion requests', zh: '你的选择与删除请求',
    enText: 'You can change providers, disconnect integrations, revoke system permissions, remove local data and sign out. To request access, correction or deletion of your LingXi account information, contact the support address below from the email associated with your account. We may need to verify ownership before acting. See the data deletion page for the request steps and the distinction between account, local and provider data. Rights and any legally required exceptions depend on your jurisdiction.',
    zhText: '你可以更换提供商、断开集成、撤销系统权限、移除本地数据及退出登录。如需访问、更正或删除灵犀账号信息，请使用与账号关联的邮箱联系下方支持地址。处理前可能需要核实账号归属。数据删除页面说明了申请步骤，以及账号、本地数据与提供商数据的区别。具体权利及法律要求的例外由适用司法辖区决定。',
  },
  {
    id: 'children', en: 'Children and policy changes', zh: '儿童与政策更新',
    enText: 'LingXi is a personal AI agent intended for a general audience, not a service directed at children. If you believe a child has supplied personal information without appropriate authorization, contact support. Changes to data handling will be reflected here with an updated date. Material changes that require consent will need to be disclosed before the affected processing begins.',
    zhText: '灵犀是面向一般用户的个人辅助 Agent，并非专门面向儿童的服务。如果你认为儿童在未经适当授权的情况下提供了个人信息，请联系支持。数据处理方式的变化会反映在本页面并更新日期；对于需要同意的重大变化，应在开始相关处理之前披露。',
  },
];

export function PrivacyPage({ locale }: { locale: Locale }) {
  const zh = locale === 'zh';
  return <article className="legal-page">
    <header className="legal-heading"><span className="eyebrow">LINGXI / PRIVACY</span>
      <h1>{zh ? '隐私政策' : 'Privacy policy'}</h1>
      <p>{zh ? '了解你的信息存在哪里、何时发送，以及如何管理。' : 'Understand where your information lives, when it is sent and how to manage it.'}</p>
      <small>{zh ? '更新日期' : 'Last updated'} · {privacyUpdated}</small>
    </header>
    <div className="legal-layout">
      <nav className="legal-toc" aria-label={zh ? '政策目录' : 'Policy contents'}>{sections.map((s, i) => <a key={s.id} href={`#${s.id}`}><span>{String(i + 1).padStart(2, '0')}</span>{zh ? s.zh : s.en}</a>)}<a href="#contact">{zh ? '联系与第三方政策' : 'Contact and provider policies'}</a></nav>
      <div className="legal-body">{sections.map((s, i) => <section id={s.id} key={s.id}><span className="legal-number">{String(i + 1).padStart(2, '0')}</span><h2>{zh ? s.zh : s.en}</h2><p>{zh ? s.zhText : s.enText}</p></section>)}
        <section id="contact"><h2>{zh ? '联系与第三方政策' : 'Contact and provider policies'}</h2><p>LingXi · <a href={`mailto:${privacyContact}`}>{privacyContact}</a></p>
          <p><RouteLink href="/data-deletion">{zh ? '数据与账号删除说明' : 'Data and account deletion'}</RouteLink></p>
          <ul><li><a href="https://firebase.google.com/support/privacy">Google / Firebase</a></li><li><a href="https://www.apple.com/legal/privacy/">Apple</a></li><li><a href="https://openai.com/policies/privacy-policy/">OpenAI</a></li><li><a href="https://www.anthropic.com/legal/privacy">Anthropic</a></li></ul>
          <p>{zh ? '此列表不是全部接收方清单；你配置的其他端点和集成适用各自的政策。' : 'This is not an exhaustive recipient list; other configured endpoints and integrations have their own policies.'}</p>
        </section>
      </div>
    </div>
  </article>;
}

export function DataDeletionPage({ locale }: { locale: Locale }) {
  const zh = locale === 'zh';
  const subject = 'LingXi account and data deletion request';
  const body = 'Account email: \nSign-in method (email / Google / Apple): \nRequest: Delete my LingXi account and associated account information.\n';
  return <article className="legal-page legal-page-narrow">
    <header className="legal-heading"><span className="eyebrow">LINGXI / YOUR DATA</span><h1>{zh ? '数据与账号删除' : 'Data and account deletion'}</h1><p>{zh ? '账号信息、本地工作与第三方数据分别管理。' : 'Account information, local work and third-party data are managed separately.'}</p></header>
    <div className="legal-body">
      <section><h2>{zh ? '申请删除灵犀账号' : 'Request deletion of your LingXi account'}</h2>
        <p>{zh ? '使用账号关联邮箱发送删除请求至 support.lingxi@gmail.com，注明登录方式（邮箱、Google 或 Apple）。Apple 隐藏邮箱用户请注明转发邮箱；如无法使用原邮箱，请说明情况，以便核实归属。无需提供密码或登录令牌。' : 'Email support.lingxi@gmail.com from the address associated with your account and specify your sign-in method (email, Google or Apple). If you use Apple Hide My Email, include the relay address. If you cannot access that email, explain this so ownership can be verified. Do not include passwords or authentication tokens.'}</p>
        <a className="button primary" href={`mailto:${privacyContact}?subject=${encodeURIComponent(subject)}&body=${encodeURIComponent(body)}`}>{zh ? '发送删除请求' : 'Send a deletion request'}</a>
        <p className="legal-note">{zh ? '这是通过邮件提交的人工处理申请，不会立即删除账号。支持团队需核实归属，并告知处理状态、预计时间及任何依法保留的数据。' : 'This submits a request for manual handling by email; it does not immediately delete an account. Support needs to verify ownership and communicate status, expected timing and any data retained by law.'}</p>
      </section>
      <section><h2>{zh ? '删除范围' : 'What the request covers'}</h2><p>{zh ? '请求范围是灵犀账号及其关联认证资料，例如账号标识、邮箱、姓名及登录关联。退出登录、卸载应用或撤销 Google/Apple 授权不等于删除灵犀账号。依法需要保留的信息如有例外，应在处理时明确说明。' : 'The request covers your LingXi account and associated authentication information, such as account identifier, email, name and sign-in associations. Signing out, uninstalling or revoking Google/Apple authorization does not delete a LingXi account. Any legally required retention exceptions should be explained during processing.'}</p></section>
      <section><h2>{zh ? '本地数据与工作区' : 'Local data and workspaces'}</h2><p>{zh ? '账号删除申请不会远程清除设备上的对话、设置、下载模型、项目和导出文件。请先备份需要保留的工作，再通过应用中的可用删除操作、Android 系统应用存储设置或移除 iOS 应用（不是卸载未使用应用）清理本地数据。桌面工作区、导出文件及系统备份需单独处理。' : 'An account deletion request does not remotely erase conversations, settings, downloaded models, projects or exports on your device. Back up work you need, then use available app deletion controls, Android app storage settings or delete the iOS app (rather than offloading it). Desktop workspaces, exports and system backups need to be handled separately.'}</p></section>
      <section><h2>{zh ? '第三方服务数据' : 'Data held by third-party services'}</h2><p>{zh ? '发送至 AI、语音、搜索或其他连接服务的数据由该服务管理。请直接向对应提供商申请删除，并撤销不再使用的 API 密钥、令牌和集成授权。' : 'AI, speech, search and other connected services manage the data sent to them. Request deletion directly from those providers and revoke API keys, tokens and integration authorizations you no longer use.'}</p></section>
      <p><RouteLink href="/privacy">{zh ? '阅读完整隐私政策' : 'Read the full privacy policy'}</RouteLink> · <a href={`mailto:${privacyContact}`}>{privacyContact}</a></p>
    </div>
  </article>;
}
