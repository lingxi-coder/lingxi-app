import type { DocPage, DocText } from './types';

const t = (en: string, zh: string): DocText => ({ en, zh });
const app = 'https://github.com/lingxi-coder/lingxi-app';
const harness = 'https://github.com/lingxi-coder/harness-runtime';

export const guidePages: DocPage[] = [
  {
    id: 'install', group: 'guide',
    title: t('Install LingXi', '安装灵犀'),
    description: t('See which LingXi apps you can install today, and how the command-line tool is named.', '查看今天可以安装的灵犀应用，以及命令行工具的名称。'),
    sourceUrl: app,
    sources: [
      { repo: 'lingxi-app', path: 'packaging/npm/package.json' },
      { repo: 'lingxi-app', path: 'apps/web/src/data/mockData.ts' },
      { repo: 'lingxi-app', path: 'apps/cli/host/Cargo.toml' },
      { repo: 'lingxi-app', path: 'README.md' },
    ],
    sections: [
      { id: 'download-page', title: t('Start from the download page', '从下载页开始'), paragraphs: [
        t('The download page is the list of builds this site will offer. A row there is not a public store listing until it has a release link.', '下载页列出本站准备提供的构建。某一行还没有发布链接时，它还不是应用商店里的公开版本。'),
      ], links: [{ label: t('Download page', '下载页'), href: '/download' }] },
      { id: 'availability', title: t('What is available', '现在有什么'), tables: [{
        headers: [t('App', '应用'), t('Status recorded in this site', '本站记录的状态'), t('How you get it', '获取方式')],
        rows: [
          [t('CLI', '命令行'), t('Development package, version 0.0.0-dev', '开发包，版本 0.0.0-dev'), t('The npm package name is lingxi. Its command is lingxi. This repository does not record a public registry release.', 'npm 包名是 lingxi，命令是 lingxi。本仓库没有记录已发布到公共仓库的版本。')],
          [t('Desktop for Mac, Apple silicon', 'Mac 桌面版，Apple 芯片'), t('Internal beta 0.1.0-internal', '内部测试版 0.1.0-internal'), t('Runs the local engine. Cloud accounts and cross-device sync are not part of this beta. No public download link is recorded.', '运行本地引擎。这个测试版不包含云账号和跨设备同步。没有记录公开下载链接。')],
          [t('Desktop for Intel Mac, Windows, and Linux', 'Intel Mac、Windows 与 Linux 桌面版'), t('Not offered', '未提供'), t('No Intel, Windows, or Linux desktop package is published from this repository.', '本仓库不发布 Intel、Windows 或 Linux 桌面安装包。')],
          [t('iPhone and Android', 'iPhone 与 Android'), t('Internal beta', '内部测试版'), t('No App Store or public APK link is recorded.', '没有记录 App Store 或公开 APK 链接。')],
        ],
      }] },
      { id: 'from-source', title: t('Run the CLI from a checkout', '从源码运行命令行'), paragraphs: [
        t('In a product checkout the Cargo binary is lingxi-cli. The npm package wraps that program as the lingxi command.', '在产品源码目录里，Cargo 二进制文件叫 lingxi-cli。npm 包把它包装成 lingxi 命令。'),
      ], code: [{
        label: t('Show CLI help', '查看命令行帮助'),
        language: 'shell',
        code: 'cargo run --locked -p cli --bin lingxi-cli -- --help',
      }], note: t('Build the CLI from the lingxi-app repository root. This command compiles the current checkout. It does not install a public release.', '在 lingxi-app 仓库根目录构建。这条命令编译当前检出的源码，不会安装一个公开发布版本。') },
    ],
  },
  {
    id: 'first-session', group: 'guide',
    title: t('Your first session', '第一次会话'),
    description: t('Open LingXi in a project, confirm the folder, and see where instructions and settings are stored.', '在项目里打开灵犀，确认文件夹，并查看说明与设置的保存位置。'),
    sourceUrl: app,
    sources: [
      { repo: 'lingxi-app', path: 'apps/cli/host/src/argv.rs' },
      { repo: 'lingxi-app', path: 'apps/cli/tui/src/session.rs' },
      { repo: 'lingxi-app', path: 'apps/cli/host/src/repl.rs' },
      { repo: 'harness-runtime', path: 'crates/branding/src/lib.rs' },
      { repo: 'harness-runtime', path: 'crates/memory/src/lingxi_md/agents.rs' },
      { repo: 'harness-runtime', path: 'crates/orchestrator/src/prompt/nested_memory.rs' },
    ],
    sections: [
      { id: 'open', title: t('Open a project', '打开一个项目'), steps: [
        t('Go to a project folder you created or otherwise trust.', '进入一个你创建的、或者你信任的项目文件夹。'),
        t('Run lingxi. From a source checkout, run lingxi-cli.', '运行 lingxi。从源码运行时用 lingxi-cli。'),
        t('If LingXi asks whether you trust the folder, confirm only when you do. After you confirm, it can read, edit, and run files there.', '如果灵犀询问你是否信任这个文件夹，只在你确实信任时确认。确认之后，它可以在这里读取、修改和运行文件。'),
      ], code: [{ label: t('Start an interactive session', '开始交互会话'), language: 'shell', code: 'lingxi' }] },
      { id: 'one-turn', title: t('Ask for one result', '只要一次结果'), paragraphs: [
        t('Pass the request as an argument to run one turn. Add -p or --print to print the result and exit. With no argument, LingXi stays in the interactive session.', '把请求作为参数传入，就只跑一轮。加上 -p 或 --print 会打印结果并退出。不带参数时，灵犀留在交互会话里。'),
      ], code: [{ label: t('One printed turn', '打印一轮结果'), language: 'shell', code: 'lingxi -p "Summarize this repository"' }] },
      { id: 'instructions', title: t('Project instructions', '项目说明'), paragraphs: [
        t('LingXi reads LINGXI.md at the project root, .lingxi/LINGXI.md, and LINGXI.local.md. Your personal copy lives at ~/.lingxi/LINGXI.md. Type /init in a session to create a project LINGXI.md.', '灵犀读取项目根目录的 LINGXI.md、.lingxi/LINGXI.md 和 LINGXI.local.md。你自己的那份在 ~/.lingxi/LINGXI.md。在会话里输入 /init 可以创建项目的 LINGXI.md。'),
        t('When the project has none of those files, the default setting reads AGENTS.md or .lingxi/AGENTS.md instead. A project that already has a LINGXI.md file does not also load AGENTS.md under that default.', '项目里没有这些文件时，默认改为读取 AGENTS.md 或 .lingxi/AGENTS.md。项目里已经有 LINGXI.md 时，这个默认设置不会再同时加载 AGENTS.md。'),
        t('If a LINGXI.md file imports something outside the project, LingXi asks before reading it.', '如果 LINGXI.md 引用了项目以外的文件，灵犀会先询问再读取。'),
      ], links: [{ label: t('Project instructions', '项目说明'), href: '/docs/instructions' }] },
      { id: 'resume', title: t('Come back to a session', '回到一个会话'), paragraphs: [
        t('Use --continue for the latest session in this directory. Use --resume to pick one, or pass its id. Add --fork-session to continue from that history under a new session id.', '用 --continue 打开当前目录里最近的会话。用 --resume 选择一个会话，也可以直接写上它的 id。加上 --fork-session 会沿用那段历史，但使用新的会话 id。'),
      ], code: [
        { label: t('Continue or resume', '继续或恢复'), language: 'shell', code: 'lingxi --continue\nlingxi --resume\nlingxi --resume SESSION_ID' },
      ], links: [{ label: t('Sessions', '会话'), href: '/docs/sessions' }] },
      { id: 'config-home', title: t('Where settings live', '设置放在哪里'), paragraphs: [
        t('The config directory is $LINGXI_CONFIG_DIR when that variable is set, including when it is set to an empty value. Otherwise it is ~/.lingxi. Session settings are in settings.json inside that directory. /doctor prints the config directory it is using.', '设置了 LINGXI_CONFIG_DIR 时，配置目录就是它的值，即使这个值是空的。否则配置目录是 ~/.lingxi。会话设置写在该目录的 settings.json 里。/doctor 会打印当前使用的配置目录。'),
      ] },
    ],
  },
  {
    id: 'accounts', group: 'guide',
    title: t('Models and API keys', '模型与 API key'),
    description: t('Connect the model provider you want to use. Keys stay in the credential store on this computer.', '接入你要使用的模型服务。密钥保存在这台电脑的凭据存储里。'),
    sourceUrl: app,
    sources: [
      { repo: 'lingxi-app', path: 'apps/cli/host/src/commands/provider.rs' },
      { repo: 'lingxi-app', path: 'apps/cli/tui-core/src/recent_models.rs' },
      { repo: 'lingxi-app', path: 'apps/cli/host/src/argv.rs' },
    ],
    sections: [
      { id: 'your-provider', title: t('You choose the provider', '由你选择服务商'), paragraphs: [
        t('LingXi sends each request through a provider account on this computer. Adding an account does not create a LingXi subscription, and the key is not a LingXi password.', '灵犀通过这台电脑上的服务商账号发送请求。添加账号不会创建灵犀订阅，这个密钥也不是灵犀密码。'),
      ] },
      { id: 'add-key', title: t('Add an API key', '添加 API key'), paragraphs: [
        t('Pass the key on standard input, or point at a slot that is already in this computer’s secure store. The command does not accept the key as an argument, and it does not print the key back.', '从标准输入传入密钥，或者指向这台电脑安全存储里已经存在的槽位。命令不接受把密钥写在参数里，也不会把密钥打印出来。'),
      ], code: [{
        label: t('Add a key from stdin', '从标准输入添加密钥'),
        language: 'shell',
        code: 'printf %s "$API_KEY" | lingxi provider accounts add PROVIDER --name WORK --api-key-stdin',
      }] },
      { id: 'oauth', title: t('Sign in with the provider', '向服务商登录'), paragraphs: [
        t('Providers that support OAuth use login. The command can print the address instead of opening a browser, and a remote shell can paste the callback address on stdin.', '支持 OAuth 的服务商使用 login。命令可以只打印地址而不打开浏览器；远程终端可以把回调地址从标准输入贴回来。'),
      ], code: [{
        label: t('Start provider login', '开始服务商登录'),
        language: 'shell',
        code: 'lingxi provider accounts login PROVIDER --name WORK\nlingxi provider accounts list PROVIDER',
      }] },
      { id: 'choose-model', title: t('Choose a model', '选择模型'), paragraphs: [
        t('In a session, /model lists the models you can use. On the command line, --model sets the model for that launch. The selection is saved as model in ~/.lingxi/settings.json, and recent choices are kept in recentModels.', '在会话里，/model 列出可用模型。命令行用 --model 指定这一次使用的模型。选择会写入 ~/.lingxi/settings.json 的 model，最近的选择保存在 recentModels。'),
        t('A managed policy can hide models. A normal install has no such list, so the picker shows the models the providers offer.', '托管策略可以隐藏模型。普通安装没有这份名单，选择器显示各服务商提供的模型。'),
      ], links: [{ label: t('Permissions', '权限'), href: '/docs/permissions' }] },
    ],
  },
];

export const usePages: DocPage[] = [
  {
    id: 'using-cli', group: 'use',
    title: t('Command line', '命令行'),
    description: t('Talk to LingXi in the terminal, print one answer, or reopen a session.', '在终端里和灵犀对话，打印一次回答，或重新打开会话。'),
    sourceUrl: app,
    sources: [
      { repo: 'lingxi-app', path: 'apps/cli/host/src/argv.rs' },
      { repo: 'lingxi-app', path: 'apps/cli/host/Cargo.toml' },
      { repo: 'lingxi-app', path: 'packaging/npm/package.json' },
      { repo: 'lingxi-app', path: 'apps/cli/tui/src/session.rs' },
    ],
    sections: [
      { id: 'names', title: t('The command name', '命令名称'), paragraphs: [
        t('After the npm package is installed, the command is lingxi. The program’s own name in help and in /doctor is lingxi-cli. Both refer to the same CLI.', '安装 npm 包之后，命令是 lingxi。帮助和 /doctor 里程序自己的名字是 lingxi-cli。两者是同一个命令行工具。'),
      ] },
      { id: 'everyday', title: t('Everyday commands', '常用命令'), tables: [{
        headers: [t('You want', '你想'), t('Command', '命令')],
        rows: [
          [t('Open an interactive session', '打开交互会话'), t('lingxi', 'lingxi')],
          [t('Run one turn and print it', '跑一轮并打印'), t('lingxi -p "…"', 'lingxi -p "…"')],
          [t('Continue the latest session here', '继续当前目录最近的会话'), t('lingxi --continue', 'lingxi --continue')],
          [t('Pick or reopen a session', '选择或重开一个会话'), t('lingxi --resume [SESSION_ID]', 'lingxi --resume [SESSION_ID]')],
          [t('Branch that history into a new id', '把那段历史分到新 id'), t('lingxi --resume SESSION_ID --fork-session', 'lingxi --resume SESSION_ID --fork-session')],
          [t('Set the model for this launch', '指定这一次的模型'), t('lingxi --model NAME', 'lingxi --model NAME')],
          [t('Check the setup', '检查环境'), t('/doctor inside the session', '在会话里输入 /doctor')],
        ],
      }], links: [
        { label: t('First session', '第一次会话'), href: '/docs/first-session' },
        { label: t('Sessions', '会话'), href: '/docs/sessions' },
        { label: t('Permissions', '权限'), href: '/docs/permissions' },
        { label: t('CLI flags', '命令行参数'), href: '/docs/cli-flags' },
      ] },
      { id: 'slash', title: t('Inside the session', '会话里'), paragraphs: [
        t('Type / to see commands. /init creates LINGXI.md. /model changes the model. /memory lists the instruction files loaded for this session. /doctor shows the version, config directory, working directory, and MCP connection counts.', '输入 / 查看命令。/init 创建 LINGXI.md。/model 切换模型。/memory 列出这次会话加载的说明文件。/doctor 显示版本、配置目录、工作目录和 MCP 连接数量。'),
      ], links: [{ label: t('Slash commands', '斜杠命令'), href: '/docs/slash-commands' }] },
    ],
  },
  {
    id: 'using-desktop', group: 'use',
    title: t('Desktop', '桌面'),
    description: t('The Mac desktop app runs LingXi on that computer. Other desktop systems are not downloadable yet.', 'Mac 桌面应用在那台电脑上运行灵犀。其他桌面系统还不能下载。'),
    sourceUrl: app,
    sources: [
      { repo: 'lingxi-app', path: 'apps/web/src/data/mockData.ts' },
      { repo: 'lingxi-app', path: 'apps/README.md' },
      { repo: 'lingxi-app', path: 'apps/web/src/remote/strings.ts' },
    ],
    sections: [
      { id: 'mac', title: t('Mac', 'Mac'), paragraphs: [
        t('The current desktop beta is for Apple silicon Macs. It runs the local engine on that Mac. It does not include a cloud account or sync sessions across devices.', '当前桌面测试版面向 Apple 芯片的 Mac。它在这台 Mac 上运行本地引擎，不包含云账号，也不会把会话同步到其他设备。'),
        t('Intel Mac, Windows, and Linux desktop packages are not produced for download. On those systems, use the CLI when you have a build of it.', '没有可供下载的 Intel Mac、Windows 或 Linux 桌面安装包。在这些系统上，如果你已经有命令行构建，就使用命令行。'),
      ], links: [{ label: t('Download page', '下载页'), href: '/download' }] },
      { id: 'same-files', title: t('Same project files', '同一套项目文件'), paragraphs: [
        t('The desktop app uses the same config directory and LINGXI.md files as the CLI. Signing in on the desktop is what lets LingXi Remote see this computer.', '桌面应用和命令行使用同一个配置目录和同一批 LINGXI.md。在桌面版登录之后，灵犀远控才能看到这台电脑。'),
      ], links: [{ label: t('Remote control', '远控'), href: '/docs/remote-control' }] },
    ],
  },
  {
    id: 'using-mobile', group: 'use',
    title: t('iPhone and Android', 'iPhone 与 Android'),
    description: t('The phone apps are internal betas. Store links are not published on this site.', '手机应用是内部测试版。本站还没有公布商店链接。'),
    sourceUrl: app,
    sources: [
      { repo: 'lingxi-app', path: 'apps/web/src/data/mockData.ts' },
      { repo: 'lingxi-app', path: 'apps/README.md' },
    ],
    sections: [
      { id: 'status', title: t('Current builds', '当前构建'), paragraphs: [
        t('LingXi for iPhone and LingXi for Android are in internal development. The download page records their status. It does not list an App Store, TestFlight, or public APK address.', 'iPhone 版和 Android 版灵犀都在内部开发。下载页记录它们的状态，没有列出 App Store、TestFlight 或公开 APK 地址。'),
      ], links: [{ label: t('Download page', '下载页'), href: '/download' }] },
      { id: 'on-device', title: t('What the phone app is', '手机应用是什么'), paragraphs: [
        t('The phone app is a LingXi client on that device. It is not a remote view of a cloud virtual machine. Embedding the on-device Linux environment into another app is covered by the Mobile Linux SDK.', '手机应用是这台设备上的灵犀客户端，不是去看一台云端虚拟机。把设备上的 Linux 环境嵌进另一个应用，见 Mobile Linux SDK。'),
      ], links: [
        { label: t('On-device execution', '在手机上执行'), href: '/docs/mobile-execution' },
        { label: t('Mobile Linux SDK', 'Mobile Linux SDK'), href: '/docs/mobile-linux' },
      ] },
    ],
  },
  {
    id: 'permissions', group: 'use',
    title: t('Permissions', '权限'),
    description: t('Choose whether LingXi asks before it uses a tool. The permission mode is separate from an operating-system sandbox.', '选择灵犀使用工具之前是否先询问你。权限模式和操作系统沙箱是两件事。'),
    sourceUrl: harness,
    sources: [
      { repo: 'harness-runtime', path: 'crates/permission/src/mode.rs' },
      { repo: 'lingxi-app', path: 'apps/cli/host/src/argv.rs' },
      { repo: 'lingxi-app', path: 'apps/cli/tui/src/bottom_pane/permission_mode_indicator.rs' },
    ],
    sections: [
      { id: 'ask-first', title: t('Ask first', '先询问'), paragraphs: [
        t('The starting mode is manual. LingXi asks you before a tool call that no allow rule matches. In the terminal this mode is also accepted under the name default. The manual mode does not print a mode line.', '初始模式是 manual。没有允许规则匹配时，灵犀会在调用工具前询问你。在终端里，这个模式也可以写成 default。manual 模式不显示模式提示行。'),
        t('Press Shift+Tab in the terminal session to cycle the mode. The line above the input names the mode you are in.', '在终端会话里按 Shift+Tab 切换模式。输入框上方的一行会写明当前模式。'),
      ] },
      { id: 'modes', title: t('Modes', '模式'), tables: [{
        headers: [t('Mode', '模式'), t('What LingXi does', '灵犀会怎么做')],
        rows: [
          [t('manual', 'manual'), t('Ask before an unmatched tool call.', '未匹配的工具调用前先询问。')],
          [t('acceptEdits', 'acceptEdits'), t('Allow file edits without asking.', '允许文件编辑而不再询问。')],
          [t('plan', 'plan'), t('Stay with read-only work and planning. Do not mutate the project.', '只做只读查看和计划，不改动项目。')],
          [t('dontAsk', 'dontAsk'), t('Deny every action that lacks an explicit allow rule.', '拒绝所有没有明确允许规则的操作。')],
          [t('bypassPermissions', 'bypassPermissions'), t('Allow actions without asking, unless the bypass kill switch is on.', '不再询问就允许操作，除非绕过开关被关闭。')],
          [t('auto', 'auto'), t('Let the classifier decide, with a fallback when it denies. You can select this for a launch. User settings may store auto, and the session stays in manual when auto mode is unavailable. A project settings file cannot grant auto.', '由分类器决定，拒绝时有回退。你可以在这一次启动时选用。用户设置可以保存 auto；自动模式不可用时，会话仍从 manual 开始。项目设置文件不能授予 auto。')],
        ],
      }] },
      { id: 'set-mode', title: t('Set a mode', '设置模式'), paragraphs: [
        t('For one launch, pass --permission-mode. Help shows manual for the ask-first mode; default is a hidden alias. --dangerously-skip-permissions selects bypassPermissions. In settings.json, set permissions.defaultMode. A project settings file cannot grant auto or bypassPermissions.', '单次启动时传入 --permission-mode。帮助里先询问模式显示为 manual，default 是隐藏别名。--dangerously-skip-permissions 用来打开 bypassPermissions。在 settings.json 里设置 permissions.defaultMode。项目设置文件不能授予 auto 或 bypassPermissions。'),
      ], code: [{
        label: t('Plan mode for one launch', '这一次使用计划模式'),
        language: 'shell',
        code: 'lingxi --permission-mode plan',
      }], note: t('A permission mode decides whether LingXi asks you. It does not by itself start an operating-system sandbox.', '权限模式决定灵犀是否询问你。它本身不会因此开启操作系统沙箱。') },
    ],
  },
];

export const productPages: DocPage[] = [
  {
    id: 'models', group: 'product',
    title: t('Models', '模型'),
    description: t('Pick a model for this session. LingXi keeps the choice in your config directory.', '为这次会话选择模型。灵犀把选择记在你的配置目录里。'),
    sourceUrl: app,
    sources: [
      { repo: 'lingxi-app', path: 'apps/cli/tui-core/src/recent_models.rs' },
      { repo: 'lingxi-app', path: 'apps/cli/host/src/argv.rs' },
      { repo: 'lingxi-app', path: 'apps/cli/tui/src/session.rs' },
    ],
    sections: [
      { id: 'picker', title: t('In a session', '在会话里'), paragraphs: [
        t('/model opens the list. The row you pick becomes the model setting, so the next launch starts there. Up to eight recent choices are stored, most recent first.', '/model 打开列表。你选中的一行会写成模型设置，下次启动就从这里开始。最近的选择最多保留八个，最新的排在前面。'),
      ], links: [{ label: t('API keys', 'API key'), href: '/docs/accounts' }] },
      { id: 'saved', title: t('The saved setting', '保存的设置'), paragraphs: [
        t('The file is settings.json in the config directory. The model field is the selected model id, such as a provider prefix plus a model name. recentModels stores the provider and model of each recent choice. If the config directory cannot be found, the choice stays in the current session only.', '文件是配置目录里的 settings.json。model 字段是选中的模型 id，例如服务商前缀加模型名。recentModels 保存每次最近选择的服务商和模型。如果找不到配置目录，这次选择只留在当前会话。'),
      ] },
    ],
  },
  {
    id: 'remote-control', group: 'product',
    title: t('Remote control', '远控'),
    description: t('Open a session on your own computer from a signed-in browser. The computer has to approve the browser first.', '用已登录的浏览器打开你自己电脑上的会话。电脑需要先允许这个浏览器。'),
    sourceUrl: app,
    sources: [
      { repo: 'lingxi-app', path: 'apps/web/src/remote/strings.ts' },
      { repo: 'lingxi-app', path: 'apps/web/src/pages/RemotePage.tsx' },
    ],
    sections: [
      { id: 'same-account', title: t('Use the desktop account', '使用桌面版账号'), steps: [
        t('On the computer, open LingXi Desktop, sign in, and turn on Remote control in settings.', '在电脑上打开灵犀桌面版，登录，并在设置里打开远程控制。'),
        t('On the phone or another computer, open the remote page and sign in with that same account.', '在手机或另一台电脑上打开远控页面，用同一个账号登录。'),
        t('Choose the computer. The first time, compare the pairing code in the browser with the code on that computer, then allow it there.', '选择这台电脑。第一次连接时，对照浏览器和电脑上的配对码，然后在电脑上允许。'),
      ], links: [{ label: t('Open remote control', '打开远控'), href: '/remote' }] },
      { id: 'direct', title: t('The browser talks to your computer', '浏览器直接连接你的电脑'), paragraphs: [
        t('Remote control lists your computers and opens one session at a time. The browser connects to the desktop app. The computer must be on, online, and running LingXi with remote control enabled.', '远控列出你的电脑，一次打开一个会话。浏览器连接的是桌面应用。电脑需要开机、在线，并且灵犀正在运行且已开启远程控制。'),
        t('Private browsing may be unable to keep the pairing. Forgetting a computer in the browser does not remove it from the desktop app. Remove it in the desktop settings as well.', '无痕浏览可能无法保留配对。在浏览器里忘记一台电脑，不会把它从桌面应用里删掉。还要在桌面版设置里移除。'),
      ], note: t('This connects you to a computer where you already run LingXi. The browser page is the remote control documented here. lingxi remote-control exits unimplemented, and --remote-control does not open this page.', '这是连接到一台已经在运行灵犀的电脑。这里记录的远控是这个浏览器页面。lingxi remote-control 会以未实现退出，--remote-control 也不会打开这个页面。') },
    ],
  },
  {
    id: 'help', group: 'product',
    title: t('Help', '帮助'),
    description: t('Check the local setup, then write to LingXi support. Privacy requests use the same address.', '先检查本机环境，再写信给灵犀支持。隐私请求使用同一个地址。'),
    sourceUrl: app,
    sources: [
      { repo: 'lingxi-app', path: 'apps/cli/tui/src/session.rs' },
      { repo: 'lingxi-app', path: 'apps/web/src/pages/PrivacyPage.tsx' },
    ],
    sections: [
      { id: 'doctor', title: t('Look at this computer first', '先看这台电脑'), steps: [
        t('In a session, run /doctor. It prints the CLI version, the config directory, the working directory, and how many MCP servers are configured and connected.', '在会话里运行 /doctor。它会打印命令行版本、配置目录、工作目录，以及已配置和已连接的 MCP 服务器数量。'),
        t('If the config directory is not the one you expect, check whether LINGXI_CONFIG_DIR is set in the environment that launched LingXi.', '如果配置目录不是你预期的那个，检查启动灵犀的环境里是否设置了 LINGXI_CONFIG_DIR。'),
        t('If a model request fails before any work starts, check the provider account and the selected model.', '如果模型请求在开始工作前就失败，检查服务商账号和选中的模型。'),
      ], links: [
        { label: t('Install', '安装'), href: '/docs/install' },
        { label: t('API keys', 'API key'), href: '/docs/accounts' },
      ] },
      { id: 'contact', title: t('Contact', '联系'), paragraphs: [
        t('Write to support.lingxi@gmail.com for product help and for privacy requests. The privacy policy is on this site.', '产品帮助和隐私请求都写到 support.lingxi@gmail.com。隐私政策在本站。'),
      ], links: [
        { label: t('Privacy policy', '隐私政策'), href: '/privacy' },
        { label: t('support.lingxi@gmail.com', 'support.lingxi@gmail.com'), href: 'mailto:support.lingxi@gmail.com' },
      ], note: t('Use this site and that address for LingXi. A screen that points at another product’s website is not the LingXi support page.', '灵犀的说明和支持以本站和这个地址为准。某个界面如果指向另一个产品的网站，那不是灵犀的支持页。') },
    ],
  },
];
