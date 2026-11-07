import type { DocPage, DocText } from './types';

const t = (en: string, zh: string): DocText => ({ en, zh });
const app = 'https://github.com/lingxi-coder/lingxi-app';
const harness = 'https://github.com/lingxi-coder/harness-runtime';
const mobile = 'https://github.com/lingxi-coder/mobile-linux-runtime';

export const sessionPages: DocPage[] = [
  {
    id: 'sessions', group: 'use',
    title: t('Sessions', '会话'),
    description: t('Continue the latest session, pick an older one, or keep working from a copy.', '继续最近的会话，挑选更早的一次，或从一份副本接着做。'),
    sourceUrl: app,
    sources: [
      { repo: 'lingxi-app', path: 'apps/cli/host/src/argv.rs' },
      { repo: 'lingxi-app', path: 'apps/cli/host/src/commands/mod.rs' },
      { repo: 'harness-runtime', path: 'crates/command-api/src/builtins/register.rs' },
      { repo: 'harness-runtime', path: 'crates/core/src/host/agent_view.rs' },
    ],
    sections: [
      { id: 'continue', title: t('Continue or pick one', '继续或挑选'), paragraphs: [
        t('lingxi --continue opens the latest session in this directory. lingxi --resume opens a picker, and lingxi --resume SESSION_ID opens that session. Inside a session, /resume does the same job. /clear starts a fresh context and leaves the previous session on disk, so /resume can still open it.', 'lingxi --continue 打开当前目录里最近的会话。lingxi --resume 打开选择列表，lingxi --resume SESSION_ID 打开指定会话。会话里的 /resume 做同一件事。/clear 开始一段新的上下文，上一份会话仍留在磁盘上，之后还能用 /resume 打开。'),
      ], code: [{
        label: t('Continue, resume, or branch', '继续、恢复或分出副本'),
        language: 'shell',
        code: 'lingxi --continue\nlingxi --resume\nlingxi --resume SESSION_ID --fork-session',
      }], links: [
        { label: t('First session', '第一次会话'), href: '/docs/first-session#resume' },
        { label: t('Command line', '命令行'), href: '/docs/using-cli#everyday' },
      ] },
      { id: 'fork', title: t('A copy with a new id', '新 id 的副本'), paragraphs: [
        t('--fork-session keeps the history you resumed and stores the new work under a new session id. With agent view on, which is the default, /fork copies the conversation into a background session and leaves you in the current one. /subtask sends a subagent out with this conversation and brings the result back here. It needs a task, and it waits until the first turn has happened.', '加上 --fork-session 会保留你恢复的历史，并把新的工作记在一个新的会话 id 下。Agent 视图默认开着：这时 /fork 把对话复制成一个后台会话，你仍留在当前会话。/subtask 派一个子 Agent 带走这段对话，再把结果带回来。它需要一段任务说明，而且要等第一轮对话发生之后才能用。'),
        t('Agent view turns off when settings set disableAgentView to true, or when LINGXI_DISABLE_AGENT_VIEW is 1, true, yes, or on. In that mode /fork starts the subagent inside this session, and /subtask is not the spawn command.', '设置里 disableAgentView 为 true，或 LINGXI_DISABLE_AGENT_VIEW 为 1、true、yes、on 时，Agent 视图关闭。这时 /fork 在当前会话里启动子 Agent，/subtask 不再是启动子 Agent 的命令。'),
      ], links: [
        { label: t('Subagents', '子 Agent'), href: '/docs/subagents' },
        { label: t('Environment variables', '环境变量'), href: '/docs/environment' },
      ] },
      { id: 'background', title: t('Background sessions', '后台会话'), paragraphs: [
        t('lingxi --bg starts the work as a background session and returns to the shell. It cannot be combined with --print. lingxi agents lists those sessions in a terminal; add --json when stdout is not a terminal. lingxi stop keeps the conversation. lingxi attach opens it again, and lingxi --resume works after it has stopped. lingxi rm deletes that background session and its worktree.', 'lingxi --bg 把这次工作放到后台会话并回到 shell。它不能和 --print 一起用。lingxi agents 在终端里列出这些会话；标准输出不是终端时加上 --json。lingxi stop 会停掉会话并保留对话。lingxi attach 再次打开它，停掉之后 lingxi --resume 仍然可用。lingxi rm 删除该后台会话和它的工作树。'),
      ], code: [{
        label: t('Start, stop, and reopen', '开始、停止、再打开'),
        language: 'shell',
        code: 'lingxi --bg "Run the test suite and summarize failures"\nlingxi agents\nlingxi stop SESSION_ID\nlingxi attach SESSION_ID',
      }], note: t('The program name in help text is lingxi-cli. After the npm package is installed, the same program is the lingxi command.', '帮助文字里的程序名是 lingxi-cli。安装 npm 包之后，同一个程序的命令是 lingxi。') },
    ],
  },
  {
    id: 'tasks', group: 'use',
    title: t('Common tasks', '常见任务'),
    description: t('Read a repository, change a bug, or run tests, and choose how much LingXi may do first.', '阅读仓库、修改一个 bug 或运行测试，并先决定灵犀可以做到哪一步。'),
    sourceUrl: app,
    sources: [
      { repo: 'lingxi-app', path: 'apps/cli/host/src/argv.rs' },
      { repo: 'harness-runtime', path: 'crates/permission/src/mode.rs' },
      { repo: 'lingxi-app', path: 'apps/cli/host/src/commands/mod.rs' },
    ],
    sections: [
      { id: 'read', title: t('Read the code', '看代码'), paragraphs: [
        t('Open the project and ask for an explanation of the path you care about. Plan mode keeps the turn on reading and planning. Pass it for one launch with --permission-mode plan, or switch with /plan and Shift+Tab.', '打开项目，让灵犀解释你关心的路径。计划模式把这一轮限制在阅读和计划。单次启动可以传 --permission-mode plan，会话里可以用 /plan 和 Shift+Tab 切换。'),
      ], code: [{
        label: t('Read-only launch', '只读启动'),
        language: 'shell',
        code: 'lingxi --permission-mode plan "Explain how this repository starts a session"',
      }], links: [{ label: t('Permission modes', '权限模式'), href: '/docs/permissions#modes' }] },
      { id: 'fix', title: t('Change a bug', '改一个 bug'), steps: [
        t('Say which file or failing behavior to change, and what result you want to see.', '说明要改哪个文件或哪种失败行为，以及你希望看到的结果。'),
        t('Stay in manual mode when you want a question before each unmatched tool call. acceptEdits allows file edits without a prompt for those edits.', '希望每次未匹配的工具调用都先问你时，留在 manual。acceptEdits 允许文件编辑不再为这些编辑询问。'),
        t('Use /diff to look at uncommitted changes and the edits from this turn.', '用 /diff 查看未提交的改动和这一轮的编辑。'),
      ], links: [{ label: t('Set a mode', '设置模式'), href: '/docs/permissions#set-mode' }] },
      { id: 'test', title: t('Run tests', '跑测试'), paragraphs: [
        t('Ask LingXi to run the project’s test command. In manual mode it asks before a shell command that no allow rule matches. When the run finishes, ask it to report the failing names and the files those failures point at.', '让灵犀运行项目自己的测试命令。manual 模式下，没有允许规则匹配的 shell 命令会先询问你。跑完后，让它报告失败的名字，以及这些失败指向的文件。'),
        t('lingxi doctor reads settings in the current directory without a trust prompt and does not connect MCP servers. For a checkup inside the session, use /doctor.', 'lingxi doctor 读取当前目录的设置，不弹出信任询问，也不会连接 MCP 服务器。要在会话里做检查，用 /doctor。'),
      ], links: [
        { label: t('Help', '帮助'), href: '/docs/help#doctor' },
        { label: t('Sessions', '会话'), href: '/docs/sessions#background' },
      ] },
    ],
  },
];

export const customizePages: DocPage[] = [
  {
    id: 'instructions', group: 'customize',
    title: t('Project instructions', '项目说明'),
    description: t('Tell LingXi how this repository works. The files are ordinary Markdown on disk.', '告诉灵犀这个仓库如何工作。这些文件就是磁盘上的普通 Markdown。'),
    sourceUrl: harness,
    sources: [
      { repo: 'harness-runtime', path: 'crates/memory/src/lingxi_md/agents.rs' },
      { repo: 'harness-runtime', path: 'crates/branding/src/lib.rs' },
      { repo: 'harness-runtime', path: 'crates/command-api/src/builtin_support/names.rs' },
    ],
    sections: [
      { id: 'files', title: t('Which files are read', '会读取哪些文件'), paragraphs: [
        t('A project can keep LINGXI.md at the repository root, .lingxi/LINGXI.md, and LINGXI.local.md. Your own copy for every project is ~/.lingxi/LINGXI.md. /init creates a project LINGXI.md. /memory lists the instruction files loaded for this session.', '项目可以把 LINGXI.md 放在仓库根目录，也可以使用 .lingxi/LINGXI.md 和 LINGXI.local.md。对所有项目生效的个人副本是 ~/.lingxi/LINGXI.md。/init 创建项目的 LINGXI.md。/memory 列出这次会话加载的说明文件。'),
      ], links: [{ label: t('First session', '第一次会话'), href: '/docs/first-session#instructions' }] },
      { id: 'agents-md', title: t('When AGENTS.md is included', '何时会读到 AGENTS.md'), paragraphs: [
        t('The default selection is lingxi-md-or-agents-md. If the project already has a LINGXI.md file, AGENTS.md is not also loaded. If it has none of the LINGXI.md files, LingXi reads AGENTS.md or .lingxi/AGENTS.md.', '默认选择是 lingxi-md-or-agents-md。项目里已经有 LINGXI.md 时，不会再同时加载 AGENTS.md。这些 LINGXI.md 都不存在时，灵犀读取 AGENTS.md 或 .lingxi/AGENTS.md。'),
        t('Trusted user, flag, and managed settings can change that selection. lingxi-md reads only the LINGXI.md files. lingxi-md-and-agents-md reads both. A project settings file is not the tier that selects this.', '受信任的用户设置、命令行设置和托管设置可以改变这个选择。lingxi-md 只读 LINGXI.md。lingxi-md-and-agents-md 两边都读。项目设置文件不负责做这个选择。'),
      ] },
      { id: 'outside', title: t('Imports outside the project', '引用项目以外的文件'), paragraphs: [
        t('If a LINGXI.md file imports a file outside the project, LingXi asks before reading it. --bare skips LINGXI.md auto-discovery. --safe-mode turns off the LINGXI.md hierarchy for that launch.', '如果 LINGXI.md 引用了项目以外的文件，灵犀会先询问再读取。--bare 会跳过 LINGXI.md 的自动发现。--safe-mode 会在这一次启动中关闭 LINGXI.md 层级。'),
      ], links: [{ label: t('CLI flags', '命令行参数'), href: '/docs/cli-flags#launch' }] },
    ],
  },
  {
    id: 'skills', group: 'customize',
    title: t('Skills', 'Skills'),
    description: t('A skill is a directory with a SKILL.md file. LingXi lists it under the directory name.', '一个 skill 是含有 SKILL.md 的目录。灵犀用目录名来显示它。'),
    sourceUrl: harness,
    sources: [
      { repo: 'harness-runtime', path: 'crates/skill/src/listing.rs' },
      { repo: 'harness-runtime', path: 'crates/command-api/src/builtins/skills.rs' },
      { repo: 'lingxi-app', path: 'apps/cli/host/src/argv.rs' },
      { repo: 'lingxi-app', path: 'apps/cli/host/src/commands/plugin.rs' },
    ],
    sections: [
      { id: 'layout', title: t('Where to put one', '放在哪里'), paragraphs: [
        t('Each skill is a directory that contains SKILL.md. The name you see is the directory name. The short text comes from the file’s description, and an optional when_to_use field can add a longer hint.', '每个 skill 是一个含有 SKILL.md 的目录。你看到的名字是目录名。简短说明来自文件里的 description，可选的 when_to_use 可以再加一段使用提示。'),
        t('Project skills are every existing .lingxi/skills directory from the current directory up to the nearest git root. The walk stops at your home directory. The directory closest to the current folder is listed first. Your skills for every project live in ~/.lingxi/skills. When a managed directory is configured, LingXi also reads <managed>/.lingxi/skills. Extra roots passed into the session appear after those.', '项目 skill 来自当前目录往上、直到最近的 git 根目录之间每一个存在的 .lingxi/skills。走到你的主目录就停止。离当前文件夹最近的目录排在前面。对所有项目生效的 skill 在 ~/.lingxi/skills。配置了托管目录时，灵犀还会读取 <managed>/.lingxi/skills。会话额外收到的根目录排在这些之后。'),
      ], code: [{
        label: t('A project skill', '一个项目 skill'),
        language: 'text',
        code: '.lingxi/skills/review/SKILL.md',
      }] },
      { id: 'list', title: t('List and reload', '查看和重新加载'), paragraphs: [
        t('/skills lists the skills it found. Empty sections are omitted. When nothing is found, the list tells you to create skills in .lingxi/skills/ or ~/.lingxi/skills/. The same directory is shown once, even if two roots reach it. /reload-skills walks the roots again. In safe mode that command adds that custom skills are disabled.', '/skills 列出找到的 skill，空的分组不会出现。一个都没有时，列表会提示你在 .lingxi/skills/ 或 ~/.lingxi/skills/ 里创建。两条路径指向同一目录时只显示一次。/reload-skills 会重新走一遍这些目录。安全模式下，这条命令会加上自定义 skill 已禁用的说明。'),
        t('lingxi plugin init NAME scaffolds ~/.lingxi/skills/NAME/ and loads it on the next session as NAME@skills-dir. --safe-mode turns custom skills off for that launch. --disable-slash-commands empties the slash-command and skill registry for that launch.', 'lingxi plugin init NAME 会在 ~/.lingxi/skills/NAME/ 生成一份骨架，下次会话以 NAME@skills-dir 加载。--safe-mode 会在这一次启动中关闭自定义 skill。--disable-slash-commands 会在这一次启动中清空斜杠命令和 skill 注册表。'),
      ], links: [{ label: t('Plugins', '插件'), href: '/docs/plugins#install' }] },
    ],
  },
  {
    id: 'subagents', group: 'customize',
    title: t('Subagents', '子 Agent'),
    description: t('Define a subagent with a Markdown file, or ask LingXi to write one. /agents is not a manager.', '用 Markdown 文件定义子 Agent，或让灵犀写一份。/agents 不是管理界面。'),
    sourceUrl: harness,
    sources: [
      { repo: 'harness-runtime', path: 'crates/command-api/src/builtins/agents.rs' },
      { repo: 'harness-runtime', path: 'crates/agent/src/catalog.rs' },
      { repo: 'harness-runtime', path: 'crates/core/src/host/agent_view.rs' },
      { repo: 'lingxi-app', path: 'apps/cli/host/src/argv.rs' },
      { repo: 'lingxi-app', path: 'apps/cli/host/src/commands/agents.rs' },
    ],
    sections: [
      { id: 'files', title: t('Definition files', '定义文件'), paragraphs: [
        t('Put one Markdown file for each subagent under .lingxi/agents/ in the project, or under ~/.lingxi/agents/ for every project. Nested directories are included. LingXi walks from the current directory up toward the project root and stops at your home directory. On the same agent name, the deeper project file wins over a shallower one, and a project file wins over the user file. --add-dir can add more project directories at a lower priority than the walk.', '每个子 Agent 用一份 Markdown。项目里放在 .lingxi/agents/，所有项目通用的放在 ~/.lingxi/agents/。子目录里的文件也会被读到。灵犀从当前目录向项目根目录走，到你的主目录停止。同名时，更深的项目文件胜过更浅的，项目文件胜过用户文件。--add-dir 可以再加入项目目录，优先级低于这次向上查找。'),
        t('/agents prints instructions. It tells you to ask LingXi to create or update a subagent, or to edit those directories. It does not open a manager. lingxi agents is a different command: it lists background sessions.', '/agents 只打印说明。它让你请灵犀创建或更新子 Agent，或者直接编辑这些目录。它不会打开管理界面。lingxi agents 是另一条命令，用来列后台会话。'),
      ], links: [{ label: t('Sessions', '会话'), href: '/docs/sessions#background' }] },
      { id: 'launch', title: t('Choose one for a launch', '为这一次启动指定'), paragraphs: [
        t('--agent NAME selects the agent for this launch and overrides the agent setting. --agents takes one JSON object that defines agents for this launch. Those definitions sit above the files on disk. Safe mode warns and ignores that JSON.', '--agent NAME 指定这一次启动使用的 Agent，并覆盖 agent 设置。--agents 接收一份 JSON 对象，为这一次启动定义 Agent。这些定义优先于磁盘上的文件。安全模式会警告并忽略这份 JSON。'),
        t('With agent view on, /subtask is how you send a subagent from the session. With agent view off, use /fork for that in-session spawn. Caps you can set are LINGXI_MAX_CONCURRENT_SUBAGENTS, LINGXI_MAX_SUBAGENT_SPAWN_DEPTH, and LINGXI_MAX_SUBAGENTS_PER_SESSION. LINGXI_SUBAGENT_MODEL is the default model when a call does not name one. LINGXI_SUBAGENT_MODEL_FORCE stops a call from picking a different model.', 'Agent 视图开着时，用 /subtask 从会话里派子 Agent。关掉时，用 /fork 做这次会话内的派发。可以设置的上限有 LINGXI_MAX_CONCURRENT_SUBAGENTS、LINGXI_MAX_SUBAGENT_SPAWN_DEPTH 和 LINGXI_MAX_SUBAGENTS_PER_SESSION。调用没有指定模型时，LINGXI_SUBAGENT_MODEL 是默认模型。LINGXI_SUBAGENT_MODEL_FORCE 会阻止某一次调用改用别的模型。'),
      ], links: [{ label: t('Environment variables', '环境变量'), href: '/docs/environment' }] },
    ],
  },
  {
    id: 'hooks', group: 'customize',
    title: t('Hooks', 'Hooks'),
    description: t('A hook is a command, HTTP call, or MCP tool that runs when a session event happens.', 'Hook 是在会话事件发生时运行的命令、HTTP 调用或 MCP 工具。'),
    sourceUrl: harness,
    sources: [
      { repo: 'harness-runtime', path: 'crates/hooks/src/loader.rs' },
      { repo: 'harness-runtime', path: 'crates/hooks/src/events.rs' },
      { repo: 'harness-runtime', path: 'crates/command-api/src/builtin_support/names.rs' },
    ],
    sections: [
      { id: 'where', title: t('Where hooks are configured', '配置写在哪里'), paragraphs: [
        t('Put a hooks object in settings.json. Each key is an event name. Each entry can filter with a matcher and then lists hooks. A hook type is command, http, or mcp_tool. A command hook runs a program. An HTTP hook calls a URL. An MCP hook calls a tool on a connected server. Unknown event names and entries that omit their required fields are skipped.', '在 settings.json 里写 hooks 对象。每个键是事件名。每一项可以用 matcher 过滤，然后列出 hooks。类型是 command、http 或 mcp_tool。command 运行一个程序，http 调用一个 URL，mcp_tool 调用已连接服务器上的工具。不认识的事件名，以及缺少必填字段的条目，会被跳过。'),
        t('/hooks shows the hook configuration for tool events. Config changes are reported separately for ~/.lingxi/settings.json, the checked-in .lingxi/settings.json, the gitignored .lingxi/settings.local.json, managed policy, and skills. Policy changes are not something a hook can block.', '/hooks 显示工具事件的 hook 配置。配置变更会按来源分开报告：~/.lingxi/settings.json、已纳入版本库的 .lingxi/settings.json、被 git 忽略的 .lingxi/settings.local.json、托管策略，以及 skills。策略变更不能被 hook 拦住。'),
      ], code: [{
        label: t('A command hook', '一个命令 hook'),
        language: 'json',
        code: '{\n  "hooks": {\n    "SessionStart": [\n      { "hooks": [{ "type": "command", "command": "echo session-started" }] }\n    ]\n  }\n}',
      }], links: [{ label: t('Settings files', '设置文件'), href: '/docs/settings#files' }] },
      { id: 'events', title: t('Events you can subscribe to', '可以订阅的事件'), paragraphs: [
        t('The loader accepts the event names below. A hook on PreToolUse runs before a tool call. PostToolUse runs after a successful call, PostToolUseFailure after a failed one, and PostToolBatch after the batch. UserPromptSubmit runs when you send a prompt. SessionStart and SessionEnd wrap the session. Stop and StopFailure wrap a stop. PermissionRequest and PermissionDenied wrap an ask. PreCompact and PostCompact wrap context compaction. SubagentStart and SubagentStop wrap a subagent that has already been spawned. AgentSpawn runs before that spawn and may refuse it or change the agent, model, directory, or background flag.', '加载器接受下面这些事件名。PreToolUse 在工具调用前运行。PostToolUse 在成功之后，PostToolUseFailure 在失败之后，PostToolBatch 在一批调用结束之后。UserPromptSubmit 在你发出提示时运行。SessionStart 和 SessionEnd 包住会话。Stop 和 StopFailure 包住一次停止。PermissionRequest 和 PermissionDenied 包住一次询问。PreCompact 和 PostCompact 包住上下文压缩。SubagentStart 和 SubagentStop 包住已经派发的子 Agent。AgentSpawn 在派发之前运行，可以拒绝这次派发，或改掉 Agent、模型、目录和后台标志。'),
        t('Other names you can use are InstructionsLoaded, ConfigChange, Notification, FileChanged, CwdChanged, and DirectoryAdded. --safe-mode skips settings hooks. --bare skips them too. A managed policy can limit hooks to the managed set.', '还可以使用 InstructionsLoaded、ConfigChange、Notification、FileChanged、CwdChanged 和 DirectoryAdded。--safe-mode 会跳过设置里的 hooks。--bare 也会跳过。托管策略可以把 hooks 限制为托管集合。'),
      ] },
    ],
  },
  {
    id: 'mcp', group: 'customize',
    title: t('MCP', 'MCP'),
    description: t('Connect LingXi to an MCP server with lingxi mcp. A project server waits until you approve it.', '用 lingxi mcp 把灵犀接到 MCP 服务器。项目里的服务器要等你批准后才会连接。'),
    sourceUrl: app,
    sources: [
      { repo: 'lingxi-app', path: 'apps/cli/host/src/commands/mcp.rs' },
      { repo: 'lingxi-app', path: 'apps/cli/host/src/argv.rs' },
    ],
    sections: [
      { id: 'add', title: t('Add a server', '添加服务器'), paragraphs: [
        t('lingxi mcp add writes a server into the local, user, or project scope. The default transport for a command after -- is stdio. --transport http or --transport sse points at a URL. lingxi mcp add-json accepts stdio, SSE, HTTP, or WebSocket as one JSON value. lingxi mcp list prints the configured servers, and lingxi mcp remove deletes one.', 'lingxi mcp add 把服务器写进 local、user 或 project 范围。-- 后面的命令默认使用 stdio。--transport http 或 --transport sse 指向一个 URL。lingxi mcp add-json 用一份 JSON 添加 stdio、SSE、HTTP 或 WebSocket。lingxi mcp list 打印已配置的服务器，lingxi mcp remove 删除一个。'),
      ], code: [{
        label: t('Add a local server and list them', '添加本地服务器并列出'),
        language: 'shell',
        code: 'lingxi mcp add my-server -- npx my-mcp-server\nlingxi mcp list\nlingxi mcp get my-server',
      }], note: t('On Mac and on WSL, lingxi mcp add-from-claude-desktop imports servers that desktop app already saved. The command above is the ordinary way to add a server.', '在 Mac 和 WSL 上，lingxi mcp add-from-claude-desktop 可以导入那个桌面应用已经保存的服务器。平时添加服务器用上面的命令。') },
      { id: 'project', title: t('Project servers', '项目里的服务器'), paragraphs: [
        t('A project-scoped server is stored in .mcp.json. Until you approve it, lingxi mcp get shows it as pending and LingXi does not connect. An approved server is health-checked unless this project has disabled it. lingxi mcp reset-project-choices clears the approved and rejected choices for project-scoped servers.', '项目范围的服务器写在 .mcp.json。你批准之前，lingxi mcp get 会把它显示为待批准，灵犀不会连接。批准后的服务器会做健康检查，除非这个项目把它禁用了。lingxi mcp reset-project-choices 清除项目范围服务器的批准和拒绝记录。'),
        t('--mcp-config adds servers for this launch. --strict-mcp-config keeps the launch to that set. --mcp-debug is the old switch for MCP errors; --debug replaces it. /mcp manages servers from the session, and /doctor counts how many are configured and connected. --safe-mode skips .mcp.json servers discovered in the project. Servers passed with --mcp-config still apply.', '这一次启动可以用 --mcp-config 增加服务器。--strict-mcp-config 让这一次只使用那一组。--mcp-debug 是查看 MCP 错误的旧开关，改用 --debug。会话里用 /mcp 管理服务器，/doctor 会统计已配置和已连接的数量。--safe-mode 会跳过项目里发现的 .mcp.json 服务器。用 --mcp-config 传入的服务器仍然生效。'),
      ], links: [{ label: t('CLI flags', '命令行参数'), href: '/docs/cli-flags#tools' }] },
    ],
  },
  {
    id: 'plugins', group: 'customize',
    title: t('Plugins', '插件'),
    description: t('Install a plugin from a marketplace, or keep one in your skills directory.', '从市场安装插件，或把插件放在 skills 目录里。'),
    sourceUrl: app,
    sources: [
      { repo: 'lingxi-app', path: 'apps/cli/host/src/commands/plugin.rs' },
      { repo: 'lingxi-app', path: 'apps/cli/host/src/commands/plugin_tag.rs' },
    ],
    sections: [
      { id: 'install', title: t('Install and update', '安装和更新'), paragraphs: [
        t('lingxi plugin install PLUGIN@MARKETPLACE installs from that marketplace. lingxi plugin list shows what is installed. lingxi plugin enable and lingxi plugin disable take a scope of user, project, or local. lingxi plugin update fetches the latest version and asks you to restart before it applies. lingxi plugin uninstall removes one. The plugins alias is the same command.', 'lingxi plugin install PLUGIN@MARKETPLACE 从该市场安装。lingxi plugin list 显示已安装的插件。lingxi plugin enable 和 lingxi plugin disable 的范围可以是 user、project 或 local。lingxi plugin update 获取最新版本，并要你重启后才会生效。lingxi plugin uninstall 移除一个。plugins 是同一条命令的别名。'),
        t('In a session, /plugin manages plugins and /reload-plugins activates pending plugin changes. The marketplace alias opens the same plugin commands.', '会话里用 /plugin 管理插件，/reload-plugins 让待生效的插件变更在当前会话里生效。marketplace 这个别名进入同一组插件命令。'),
      ], code: [{
        label: t('Install from a marketplace', '从市场安装'),
        language: 'shell',
        code: 'lingxi plugin install name@marketplace\nlingxi plugin list\nlingxi plugin update name',
      }] },
      { id: 'manifest', title: t('The manifest', '清单文件'), paragraphs: [
        t('A plugin root carries .lingxi-plugin/plugin.json. A marketplace carries .lingxi-plugin/marketplace.json. When those versions disagree, the version in plugin.json is the one used at install time. lingxi plugin validate checks a plugin or marketplace manifest, and it can also check the skills, agents, and commands in a directory.', '插件根目录带有 .lingxi-plugin/plugin.json。市场带有 .lingxi-plugin/marketplace.json。两边的版本不一致时，安装时采用 plugin.json 里的版本。lingxi plugin validate 检查插件或市场清单，也可以检查一个目录里的 skills、agents 和 commands。'),
        t('lingxi plugin init NAME creates a skills-directory plugin at ~/.lingxi/skills/NAME/. That form loads on the next session without a marketplace entry. --safe-mode disables plugins for that launch. --plugin-dir and --plugin-url add a plugin root or URL for one launch.', 'lingxi plugin init NAME 在 ~/.lingxi/skills/NAME/ 创建一个 skills 目录形式的插件。这种形式下次会话就会加载，不需要市场条目。--safe-mode 会在这一次启动中禁用插件。--plugin-dir 和 --plugin-url 为这一次启动增加一个插件目录或 URL。'),
      ], links: [{ label: t('Skills', 'Skills'), href: '/docs/skills#list' }] },
    ],
  },
];

export const handbookReferencePages: DocPage[] = [
  {
    id: 'settings', group: 'reference',
    title: t('Settings files', '设置文件'),
    description: t('User settings live in the config directory. Project settings live in the repository. They do not merge the same way for every key.', '用户设置在配置目录里。项目设置在仓库里。并不是每个键都按同一种方式合并。'),
    sourceUrl: app,
    sources: [
      { repo: 'lingxi-app', path: 'apps/cli/host/src/permission_settings.rs' },
      { repo: 'lingxi-app', path: 'apps/cli/host/src/run/resume.rs' },
      { repo: 'harness-runtime', path: 'crates/migrations/src/global_config.rs' },
      { repo: 'harness-runtime', path: 'crates/branding/src/lib.rs' },
      { repo: 'harness-runtime', path: 'crates/hooks/src/events.rs' },
    ],
    sections: [
      { id: 'files', title: t('The files', '这些文件'), tables: [{
        headers: [t('File', '文件'), t('What it is for', '用途')],
        rows: [
          [t('~/.lingxi/settings.json', '~/.lingxi/settings.json'), t('Your settings, including model and permissions.defaultMode.', '你的设置，包括 model 和 permissions.defaultMode。')],
          [t('.lingxi/settings.json', '.lingxi/settings.json'), t('Project settings that can be committed.', '可以提交到仓库的项目设置。')],
          [t('.lingxi/settings.local.json', '.lingxi/settings.local.json'), t('Project-local settings. Git ignores this file. The permission-mode reader below does not load it.', '项目本地设置。Git 会忽略这个文件。下面的权限模式读取器不会加载它。')],
          [t('~/.lingxi.json', '~/.lingxi.json'), t('A separate global file. It is not settings.json.', '另一份全局文件。它不是 settings.json。')],
        ],
      }], paragraphs: [
        t('When LINGXI_CONFIG_DIR is unset, the config directory is ~/.lingxi and the global file is ~/.lingxi.json. When it is set to a non-empty path, session settings are read from that directory. An empty value is still set for the command line: it is a relative path, so the CLI resolves it from the directory where you started LingXi. /doctor prints the config directory value it is using. The ~/.lingxi.json lookup treats an empty LINGXI_CONFIG_DIR as unset.', 'LINGXI_CONFIG_DIR 未设置时，配置目录是 ~/.lingxi，全局文件是 ~/.lingxi.json。设成非空路径时，会话设置从那个目录读取。空值对命令行来说仍然算已设置：它是相对路径，所以命令行会从你启动灵犀的目录来解析。/doctor 打印它正在使用的配置目录。查找 ~/.lingxi.json 时，空的 LINGXI_CONFIG_DIR 视为未设置。'),
      ], links: [{ label: t('First session', '第一次会话'), href: '/docs/first-session#config-home' }] },
      { id: 'permission-mode', title: t('How the permission mode is chosen', '权限模式如何决定'), paragraphs: [
        t('For the permission mode, the CLI reads the user settings.json and then the project .lingxi/settings.json. The project value wins for permissions.defaultMode. If either file disables bypass or auto, that disable stays on. Only the user file can grant auto or bypassPermissions. A project file that asks for those modes does not grant them.', '决定权限模式时，命令行先读用户的 settings.json，再读项目的 .lingxi/settings.json。permissions.defaultMode 以项目里的值为准。任一文件关闭了 bypass 或 auto，这个关闭就会保留。只有用户文件可以授予 auto 或 bypassPermissions。项目文件里写下这两种模式并不会授予它们。'),
        t('--settings FILE_OR_JSON is applied after those files and still applies when --restricted is set. --setting-sources user,project,local chooses which ambient files this reader opens. With no flag, it reads user and project. --restricted reads neither. This reader does not open settings.local.json. Other keys, including hooks, have their own files and are not merged by this same rule.', '--settings FILE_OR_JSON 在这些文件之后生效，加上 --restricted 时也仍然生效。--setting-sources user,project,local 选择这个读取器要打开哪些环境中的文件。不写这个参数时，它读取 user 和 project。--restricted 两者都不读。这个读取器不会打开 settings.local.json。其他键，包括 hooks，有各自的文件，并不按这同一条规则合并。'),
      ], code: [{
        label: t('Plan mode in your user settings', '在用户设置里使用计划模式'),
        language: 'json',
        code: '{\n  "permissions": { "defaultMode": "plan" }\n}',
      }], links: [{ label: t('Permissions', '权限'), href: '/docs/permissions#set-mode' }] },
    ],
  },
  {
    id: 'environment', group: 'reference',
    title: t('Environment variables', '环境变量'),
    description: t('A short list of variables you set yourself. LingXi also sets variables for a plugin or a background session; leave those alone.', '一份你自己会设置的变量清单。灵犀还会为插件或后台会话设置变量，那些不用自己导出。'),
    sourceUrl: harness,
    sources: [
      { repo: 'harness-runtime', path: 'crates/branding/src/lib.rs' },
      { repo: 'harness-runtime', path: 'crates/core/src/host/agent_view.rs' },
      { repo: 'lingxi-app', path: 'apps/cli/host/src/run/resume.rs' },
    ],
    sections: [
      { id: 'config', title: t('Config and effort', '配置和投入程度'), tables: [{
        headers: [t('Variable', '变量'), t('Effect', '作用')],
        rows: [
          [t('LINGXI_CONFIG_DIR', 'LINGXI_CONFIG_DIR'), t('Config directory for this process. See the settings page for the empty-value rule.', '这个进程的配置目录。空值规则见设置页。')],
          [t('LINGXI_EFFORT_LEVEL', 'LINGXI_EFFORT_LEVEL'), t('Effort level for the process. --effort sets it for one launch. Accepted levels are low, medium, high, xhigh, and max.', '这个进程的投入程度。--effort 只对这一次启动生效。可用级别是 low、medium、high、xhigh 和 max。')],
        ],
      }], links: [{ label: t('Settings files', '设置文件'), href: '/docs/settings#files' }] },
      { id: 'switches', title: t('Switches', '开关'), paragraphs: [
        t('For LINGXI_DISABLE_AGENT_VIEW, the values 1, true, yes, and on turn agent view off. The same setting in settings.json is disableAgentView set to true. Either one is enough. The other variables in this table are opt-outs: export them as 1 before you start LingXi.', 'LINGXI_DISABLE_AGENT_VIEW 为 1、true、yes 或 on 时关闭 Agent 视图。settings.json 里的对应项是 disableAgentView 设为 true。两边有一边即可。这张表里的其他变量是退出开关：启动灵犀前把它们导出为 1。'),
      ], tables: [{
        headers: [t('Variable', '变量'), t('Effect', '作用')],
        rows: [
          [t('LINGXI_DISABLE_AGENT_VIEW', 'LINGXI_DISABLE_AGENT_VIEW'), t('Turn off agent view, so /fork spawns a subagent in this session.', '关闭 Agent 视图，于是 /fork 在当前会话里派子 Agent。')],
          [t('LINGXI_DISABLE_BACKGROUND_TASKS', 'LINGXI_DISABLE_BACKGROUND_TASKS'), t('Turn off background task execution.', '关闭后台任务执行。')],
          [t('LINGXI_DISABLE_GIT_INSTRUCTIONS', 'LINGXI_DISABLE_GIT_INSTRUCTIONS'), t('Turn off the Git instructions supplied to shell tools.', '关闭提供给 shell 工具的 Git 说明。')],
          [t('LINGXI_DISABLE_ATTACHMENTS', 'LINGXI_DISABLE_ATTACHMENTS'), t('Turn off automatic attachment handling.', '关闭自动附件处理。')],
          [t('LINGXI_DISABLE_EXPLORE_PLAN_AGENTS', 'LINGXI_DISABLE_EXPLORE_PLAN_AGENTS'), t('Remove the Explore and Plan agents from the catalog.', '从目录里撤下 Explore 和 Plan 这两个 Agent。')],
          [t('LINGXI_ENABLE_SKILL_DOCTOR', 'LINGXI_ENABLE_SKILL_DOCTOR'), t('Enable /skill-doctor when that command is switched off in this build.', '当这个构建关掉了 /skill-doctor 时，用它重新打开。')],
        ],
      }] },
      { id: 'subagents', title: t('Subagent limits', '子 Agent 上限'), tables: [{
        headers: [t('Variable', '变量'), t('Effect', '作用')],
        rows: [
          [t('LINGXI_MAX_CONCURRENT_SUBAGENTS', 'LINGXI_MAX_CONCURRENT_SUBAGENTS'), t('How many subagents may run at once in one session.', '一个会话里同时运行的子 Agent 数量。')],
          [t('LINGXI_MAX_SUBAGENT_SPAWN_DEPTH', 'LINGXI_MAX_SUBAGENT_SPAWN_DEPTH'), t('How deep subagents may spawn further subagents.', '子 Agent 还能再派子 Agent 的层数。')],
          [t('LINGXI_MAX_SUBAGENTS_PER_SESSION', 'LINGXI_MAX_SUBAGENTS_PER_SESSION'), t('How many subagent launches one session may make in total.', '一个会话累计可以派发的子 Agent 次数。')],
          [t('LINGXI_SUBAGENT_MODEL', 'LINGXI_SUBAGENT_MODEL'), t('Default model when a subagent call does not name one.', '子 Agent 调用没有指定模型时的默认模型。')],
          [t('LINGXI_SUBAGENT_MODEL_FORCE', 'LINGXI_SUBAGENT_MODEL_FORCE'), t('Stop a subagent call from overriding the model.', '阻止子 Agent 的某一次调用改写模型。')],
        ],
      }], links: [{ label: t('Subagents', '子 Agent'), href: '/docs/subagents#launch' }] },
    ],
  },
  {
    id: 'cli-flags', group: 'reference',
    title: t('CLI flags', '命令行参数'),
    description: t('Flags for everyday launches. lingxi --help lists everything this build accepts, including flags this page leaves out.', '日常启动会用到的参数。lingxi --help 列出这个构建接受的全部参数，包括本页没有展开的那些。'),
    sourceUrl: app,
    sources: [
      { repo: 'lingxi-app', path: 'apps/cli/host/src/argv.rs' },
      { repo: 'lingxi-app', path: 'apps/cli/host/src/commands/mod.rs' },
    ],
    sections: [
      { id: 'launch', title: t('Launch', '启动'), tables: [{
        headers: [t('Flag', '参数'), t('Effect', '作用')],
        rows: [
          [t('(no flag)', '（无参数）'), t('Open an interactive session. A positional prompt runs one turn.', '打开交互会话。位置参数上的提示只跑一轮。')],
          [t('-p, --print', '-p、--print'), t('Print that turn and exit.', '打印这一轮并退出。')],
          [t('-c, --continue', '-c、--continue'), t('Continue the latest session in this directory.', '继续当前目录最近的会话。')],
          [t('-r, --resume [ID]', '-r、--resume [ID]'), t('Pick a session, or open the id you pass.', '挑选会话，或打开你传入的 id。')],
          [t('--fork-session', '--fork-session'), t('Resume into a new session id.', '把恢复的历史记到新的会话 id。')],
          [t('--model NAME', '--model NAME'), t('Model for this launch.', '这一次启动使用的模型。')],
          [t('--effort LEVEL', '--effort LEVEL'), t('low, medium, high, xhigh, or max. An unknown value is ignored.', 'low、medium、high、xhigh 或 max。无法识别的值会被忽略。')],
          [t('--cwd DIR', '--cwd DIR'), t('Working directory for this launch.', '这一次启动的工作目录。')],
          [t('-n, --name NAME', '-n、--name NAME'), t('Name for this session.', '这次会话的名字。')],
          [t('--session-id UUID', '--session-id UUID'), t('Session id to use.', '要使用的会话 id。')],
          [t('-w, --worktree [NAME]', '-w、--worktree [NAME]'), t('Create a git worktree for this session.', '为这次会话创建一个 git 工作树。')],
          [t('--bg, --background', '--bg、--background'), t('Start a background session and return. Cannot be combined with --print.', '启动后台会话并返回。不能和 --print 一起用。')],
        ],
      }], links: [{ label: t('Sessions', '会话'), href: '/docs/sessions' }] },
      { id: 'permission', title: t('Permissions and settings', '权限和设置'), tables: [{
        headers: [t('Flag', '参数'), t('Effect', '作用')],
        rows: [
          [t('--permission-mode MODE', '--permission-mode MODE'), t('manual, acceptEdits, plan, dontAsk, bypassPermissions, or auto. default is a hidden alias of manual.', 'manual、acceptEdits、plan、dontAsk、bypassPermissions 或 auto。default 是 manual 的隐藏别名。')],
          [t('--dangerously-skip-permissions', '--dangerously-skip-permissions'), t('Select bypassPermissions for this launch.', '这一次启动使用 bypassPermissions。')],
          [t('--allow-dangerously-skip-permissions', '--allow-dangerously-skip-permissions'), t('Offer bypass as a choice without selecting it now.', '把绕过权限作为一个可选项，这一次并不直接选中。')],
          [t('--settings FILE_OR_JSON', '--settings FILE_OR_JSON'), t('Settings applied after the user and project files.', '在用户和项目文件之后再应用的设置。')],
          [t('--setting-sources LIST', '--setting-sources LIST'), t('Which of user, project, and local this launch consults.', '这一次要查阅 user、project、local 中的哪些。')],
          [t('--restricted', '--restricted'), t('Do not read user or project settings files.', '不读取用户或项目设置文件。')],
          [t('--safe-mode', '--safe-mode'), t('Disable hooks, plugins, skills, custom agents, discovered .mcp.json servers, and the LINGXI.md hierarchy. --mcp-config servers remain.', '关闭 hooks、插件、skills、自定义 Agent、项目里发现的 .mcp.json 服务器，以及 LINGXI.md 层级。--mcp-config 传入的服务器仍然保留。')],
          [t('--bare', '--bare'), t('Skip hooks, plugin sync, auto-memory, background prefetches, and LINGXI.md auto-discovery.', '跳过 hooks、插件同步、自动记忆、后台预取，以及 LINGXI.md 自动发现。')],
        ],
      }], links: [{ label: t('Settings files', '设置文件'), href: '/docs/settings#permission-mode' }] },
      { id: 'tools', title: t('Tools, MCP, and output', '工具、MCP 和输出'), tables: [{
        headers: [t('Flag', '参数'), t('Effect', '作用')],
        rows: [
          [t('--tools LIST', '--tools LIST'), t('Tools available for this launch.', '这一次可用的工具。')],
          [t('--allowedTools, --allowed-tools', '--allowedTools、--allowed-tools'), t('Tools that may run without an extra ask.', '可以不再额外询问就运行的工具。')],
          [t('--disallowedTools, --disallowed-tools', '--disallowedTools、--disallowed-tools'), t('Tools this launch must not use.', '这一次不得使用的工具。')],
          [t('--add-dir DIR', '--add-dir DIR'), t('Add a working directory.', '再加一个工作目录。')],
          [t('--agent NAME', '--agent NAME'), t('Agent for this launch.', '这一次使用的 Agent。')],
          [t('--agents JSON', '--agents JSON'), t('One JSON object of agent definitions for this launch.', '一份定义这次启动所用 Agent 的 JSON 对象。')],
          [t('--mcp-config FILE', '--mcp-config FILE'), t('MCP config for this launch.', '这一次使用的 MCP 配置。')],
          [t('--strict-mcp-config', '--strict-mcp-config'), t('Use the MCP config you passed, without the discovered servers.', '只使用你传入的 MCP 配置，不加入发现的服务器。')],
          [t('--plugin-dir PATH', '--plugin-dir PATH'), t('Load a plugin directory for this launch.', '这一次加载一个插件目录。')],
          [t('--plugin-url URL', '--plugin-url URL'), t('Load a plugin URL for this launch.', '这一次加载一个插件 URL。')],
          [t('--output-format FORMAT', '--output-format FORMAT'), t('text, json, or stream-json.', 'text、json 或 stream-json。')],
          [t('--input-format FORMAT', '--input-format FORMAT'), t('text or stream-json.', 'text 或 stream-json。')],
          [t('--json', '--json'), t('JSON output.', '以 JSON 输出。')],
          [t('--no-session-persistence', '--no-session-persistence'), t('Do not save the session. Only works with --print.', '不保存会话。只能和 --print 一起用。')],
          [t('-d, --debug [FILTER]', '-d、--debug [FILTER]'), t('Debug logging. This replaces --mcp-debug.', '调试日志。用它代替 --mcp-debug。')],
        ],
      }] },
      { id: 'commands', title: t('Subcommands', '子命令'), paragraphs: [
        t('lingxi provider manages model accounts. lingxi mcp manages MCP servers. lingxi plugin manages plugins. lingxi doctor checks the installation without a trust prompt. lingxi agents lists background sessions. lingxi attach, logs, stop, and rm manage a background session: stop keeps the conversation, and rm deletes the session and its worktree. lingxi update checks for an update.', 'lingxi provider 管理模型账号。lingxi mcp 管理 MCP 服务器。lingxi plugin 管理插件。lingxi doctor 检查安装，不弹出信任询问。lingxi agents 列出后台会话。lingxi attach、logs、stop 和 rm 管理后台会话：stop 保留对话，rm 删除会话和工作树。lingxi update 检查更新。'),
        t('The signed-in remote page is how you open a session on your computer from a browser. lingxi remote-control exits unimplemented, and the --remote-control flag does not open that page.', '从浏览器打开你电脑上的会话，用已登录的远控页面。lingxi remote-control 会以未实现退出，--remote-control 参数也不会打开那个页面。'),
      ], links: [
        { label: t('Remote control', '远控'), href: '/docs/remote-control' },
        { label: t('API keys', 'API key'), href: '/docs/accounts' },
      ] },
    ],
  },
  {
    id: 'slash-commands', group: 'reference',
    title: t('Slash commands', '斜杠命令'),
    description: t('Commands you type inside a session. /help lists the ones this build will run.', '在会话里输入的命令。/help 列出这个构建会运行的那些。'),
    sourceUrl: harness,
    sources: [
      { repo: 'harness-runtime', path: 'crates/command-api/src/builtin_support/names.rs' },
      { repo: 'harness-runtime', path: 'crates/command-api/src/builtins/register.rs' },
      { repo: 'harness-runtime', path: 'crates/command-api/src/builtins/agents.rs' },
    ],
    sections: [
      { id: 'everyday', title: t('Everyday commands', '常用命令'), paragraphs: [
        t('Type / to open the list. A command that opens a screen belongs to the interactive session. lingxi -p does not open that screen. If a name below is missing from /help, this build is not offering it.', '输入 / 打开列表。会打开一个界面的命令属于交互会话。lingxi -p 不会打开那个界面。如果下面某个名字没有出现在 /help 里，说明这个构建没有提供它。'),
      ], tables: [{
        headers: [t('Command', '命令'), t('What it does', '作用')],
        rows: [
          [t('/help', '/help'), t('Show the commands available here.', '显示这里可用的命令。')],
          [t('/init', '/init'), t('Create a project LINGXI.md.', '创建项目的 LINGXI.md。')],
          [t('/memory', '/memory'), t('Show the loaded instruction files.', '显示已加载的说明文件。')],
          [t('/model', '/model'), t('Choose the model.', '选择模型。')],
          [t('/permissions', '/permissions'), t('Manage allow and deny rules.', '管理允许和拒绝规则。')],
          [t('/plan', '/plan'), t('Use plan mode, or view the plan for this session.', '使用计划模式，或查看这次会话的计划。')],
          [t('/doctor', '/doctor'), t('Check this setup from inside the session.', '在会话里检查当前环境。')],
          [t('/resume', '/resume'), t('Open a previous conversation. continue is an alias.', '打开之前的对话。continue 是别名。')],
          [t('/clear', '/clear'), t('Start a fresh context. The previous session stays on disk.', '开始新的上下文。上一份会话仍留在磁盘上。')],
          [t('/compact', '/compact'), t('Summarize the conversation so far to free context.', '总结到目前为止的对话，以腾出上下文。')],
          [t('/diff', '/diff'), t('Show uncommitted changes and this turn’s edits.', '显示未提交的改动和这一轮的编辑。')],
          [t('/config', '/config'), t('Open settings.', '打开设置。')],
          [t('/status', '/status'), t('Show version, model, account, and tool status.', '显示版本、模型、账号和工具状态。')],
          [t('/context', '/context'), t('Show how full the context is.', '显示上下文占用了多少。')],
          [t('/export', '/export'), t('Export this conversation.', '导出这次对话。')],
          [t('/rename', '/rename'), t('Rename this conversation.', '重命名这次对话。')],
          [t('/theme', '/theme'), t('Change the theme.', '更换主题。')],
          [t('/effort', '/effort'), t('Set the effort level.', '设置投入程度。')],
          [t('/add-dir', '/add-dir'), t('Add a working directory.', '增加一个工作目录。')],
        ],
      }] },
      { id: 'customize', title: t('Customization commands', '定制相关的命令'), tables: [{
        headers: [t('Command', '命令'), t('What it does', '作用')],
        rows: [
          [t('/skills', '/skills'), t('List skills.', '列出 skills。')],
          [t('/reload-skills', '/reload-skills'), t('Read skill directories again.', '重新读取 skill 目录。')],
          [t('/skill-doctor', '/skill-doctor'), t('Show loaded skills that are unused. Enable it with LINGXI_ENABLE_SKILL_DOCTOR when /help omits it.', '显示已加载但未使用的 skill。/help 里没有它时，用 LINGXI_ENABLE_SKILL_DOCTOR 打开。')],
          [t('/hooks', '/hooks'), t('View hook configuration.', '查看 hook 配置。')],
          [t('/mcp', '/mcp'), t('Manage MCP servers.', '管理 MCP 服务器。')],
          [t('/plugin', '/plugin'), t('Manage plugins. plugins and marketplace are aliases.', '管理插件。plugins 和 marketplace 是别名。')],
          [t('/reload-plugins', '/reload-plugins'), t('Apply pending plugin changes in this session.', '在这次会话里应用待生效的插件变更。')],
          [t('/agents', '/agents'), t('Print where subagent files live. It is not a manager.', '打印子 Agent 文件所在的目录。它不是管理界面。')],
          [t('/fork', '/fork'), t('Copy this conversation to a background session, or spawn a subagent when agent view is off.', '把这次对话复制到后台会话；Agent 视图关闭时，改为派一个子 Agent。')],
          [t('/subtask', '/subtask'), t('Send a subagent with this conversation. Available when agent view is on.', '带着这次对话派一个子 Agent。Agent 视图开着时可用。')],
          [t('/tasks', '/tasks'), t('Show background work.', '显示后台工作。')],
          [t('/recap', '/recap'), t('Write a one-line recap of this session.', '为这次会话写一行摘要。')],
        ],
      }], links: [
        { label: t('Skills', 'Skills'), href: '/docs/skills' },
        { label: t('Sessions', '会话'), href: '/docs/sessions#fork' },
      ], note: t('Names that install another product’s app, open an IDE bridge, or start a hosted review are not part of this guide. Use /help on your build before you rely on a name that is not in the tables.', '安装其他产品的应用、打开 IDE 桥，或发起托管审查的那些名字，不在本指南里。表格以外的名字，先在你的构建里看 /help，再决定是否使用。') },
    ],
  },
];

export const extraProductPages: DocPage[] = [
  {
    id: 'mobile-execution', group: 'product',
    title: t('On-device execution', '在手机上执行'),
    description: t('On iPhone and Android, commands run in a Linux environment on that phone.', '在 iPhone 和 Android 上，命令运行在这台手机里的 Linux 环境中。'),
    sourceUrl: mobile,
    sources: [
      { repo: 'lingxi-app', path: 'apps/web/src/data/mockData.ts' },
      { repo: 'mobile-linux-runtime', path: 'crates/mobile-linux-api/src/mobile_linux.rs' },
      { repo: 'lingxi-app', path: 'apps/README.md' },
    ],
    sections: [
      { id: 'on-the-phone', title: t('The phone runs the work', '工作在手机上运行'), paragraphs: [
        t('LingXi for iPhone and LingXi for Android are internal betas. This site does not publish an App Store, TestFlight, or public APK link. The phone app is a client on that device. A command it runs is executed there.', 'iPhone 版和 Android 版灵犀是内部测试版。本站没有公布 App Store、TestFlight 或公开 APK 链接。手机应用是这台设备上的客户端。它运行的命令就在这台设备上执行。'),
      ], links: [
        { label: t('iPhone and Android', 'iPhone 与 Android'), href: '/docs/using-mobile' },
        { label: t('Download page', '下载页'), href: '/download' },
      ] },
      { id: 'guest-paths', title: t('Paths the command can see', '命令看得到的路径'), paragraphs: [
        t('The default Linux profile uses /home/lingxi as its home. Its config directory is /home/lingxi/.lingxi, and its projects directory is /home/lingxi/.lingxi-workspaces/projects. A command’s working directory is one of these guest paths. A folder path from the phone’s own file browser is a different coordinate, and it is not automatically the path the command sees.', '默认的 Linux 配置把 /home/lingxi 当作主目录。它的配置目录是 /home/lingxi/.lingxi，项目目录是 /home/lingxi/.lingxi-workspaces/projects。命令的工作目录是这些 guest 路径之一。手机自己的文件浏览器里的文件夹路径是另一套坐标，不会自动变成命令看到的路径。'),
        t('Embedding that environment in another app, choosing mounts, and passing host paths to the runtime are covered by the Mobile Linux SDK.', '把这个环境嵌进另一个应用、选择挂载，以及把宿主路径传给运行时，见 Mobile Linux SDK。'),
      ], links: [{ label: t('Guest paths in the SDK', 'SDK 里的 guest 路径'), href: '/docs/mobile-linux#guest-identity' }] },
    ],
  },
  {
    id: 'related-products', group: 'product',
    title: t('Remote, bots, and apps', '远控、Bot 和应用'),
    description: t('A signed-in browser can open a computer that is already running LingXi, including its bots, sessions, and local apps.', '已登录的浏览器可以打开一台已经在运行灵犀的电脑，包括它的 Bot、会话和本地应用。'),
    sourceUrl: app,
    sources: [
      { repo: 'lingxi-app', path: 'apps/web/src/pages/RemotePage.tsx' },
      { repo: 'lingxi-app', path: 'apps/web/src/remote/strings.ts' },
      { repo: 'lingxi-app', path: 'apps/web/src/remote/LocalApps.tsx' },
      { repo: 'lingxi-app', path: 'apps/web/src/pages/RemoteSignInPage.tsx' },
      { repo: 'lingxi-app', path: 'apps/web/src/pages/LoginPage.tsx' },
    ],
    sections: [
      { id: 'remote', title: t('One signed-in remote page', '同一个已登录的远控页'), paragraphs: [
        t('Remote sign-in is /login/remote. After you sign in, /remote lists your computers. Pick one and confirm the pairing code on that computer the first time. The page then shows that computer’s bots and the sessions that have remote access turned on. Session controls on the open conversation change that session.', '远控登录在 /login/remote。登录之后，/remote 列出你的电脑。选一台，第一次要在那台电脑上确认配对码。页面随后显示这台电脑的 Bot，以及已经打开远程访问的会话。打开的对话里的会话控制，改的是这次会话。'),
        t('The website’s /login screen opens a console prototype. That prototype is separate from remote sign-in.', '网站的 /login 页面进入的是控制台原型。那个原型和远控登录是分开的。'),
      ], links: [
        { label: t('Remote control', '远控'), href: '/docs/remote-control' },
        { label: t('Open remote control', '打开远控'), href: '/remote' },
      ] },
      { id: 'apps', title: t('Apps on the computer', '电脑上的应用'), paragraphs: [
        t('The Apps view lists local apps that run on the connected desktop and open in this browser. An app appears there after remote access is enabled for it on the desktop. The row says it runs on the desktop. Long-running work belongs on that computer; the phone apps also run work on the phone itself.', '应用视图列出在已连接的桌面电脑上运行、并在这个浏览器里打开的本地应用。在桌面版里为某个应用打开远程访问之后，它会出现在这里。这一行会标明它运行在桌面电脑上。长时间的工作适合放在那台电脑上；手机应用也会把工作放在手机自己上面运行。'),
      ], links: [{ label: t('On-device execution', '在手机上执行'), href: '/docs/mobile-execution' }] },
    ],
  },
];
