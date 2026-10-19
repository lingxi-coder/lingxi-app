# Deeplink 使用说明

同一目标支持两种链接：`lingxi://settings/general` 和 `https://lingxios.net/open/settings/general`。HTTPS 地址提供“打开、复制、下载”网页入口；已安装并完成系统关联的应用可以直接接收链接。

链接打开本机已有的数据。它不会上传会话，也不会让另一台设备自动获得对应项目、消息或终端。

## 常用目标

| 目标 | 路径 |
| --- | --- |
| 会话、指定消息 | `session/<session-id>`，可附加 `?message=<persisted-uuid>` |
| 项目、新会话 | `project/<workspace-key>`、`new` |
| 设置、服务商 | `settings/<section>`、`provider/<id>` |
| 扩展详情 | `extensions/mcp/<name>`、`extensions/skill/<name>`、`extensions/plugin/<id>` |
| 定时任务、运行记录 | `cron/job/<id>`、`cron/run/<id>` |
| 终端、本地应用、市场 | `terminal/<id>`、`apps/<id>`、`market/<id>` |
| 运行详情 | `runtime/agent/<id>`，以及 `task`、`plan`、`review`、`activity` 对应路径 |
| 导入预览 | `import/mcp`、`import/skill`、`import/plugin`、`import/provider` |

优先使用应用内的复制链接操作生成资源链接。`workspace` 用于限定本机项目，`scope` 用于限定配置范围；资源不存在、范围不明确或当前平台不支持时，应用会给出错误。资源 ID 区分大小写，消息 UUID 统一为小写。

消息链接定位会话主记录中已经持久化的 UUID。子代理记录使用 `runtime/agent/<id>`，不能把子代理消息 UUID 放进父会话链接。

## CLI

```sh
lingxi-cli link settings/general
lingxi-cli link new --prompt '请帮我检查这个项目'
lingxi-cli link 'session/<session-id>' --project /path/to/project --message '<persisted-uuid>'
lingxi-cli link settings/general --format scheme
lingxi-cli open 'https://lingxios.net/open/settings/general'
lingxi-cli open --cli 'lingxi://new'
```

`link` 默认生成 HTTPS 地址。`--project` 将项目规范路径转换为工作区标识，避免在链接中暴露本地路径。`open` 默认交给桌面应用；`--cli` 在交互终端内支持会话、消息、项目和新会话，不允许和提交提示词、打印、流式输出或恢复参数混用。

新会话文本和终端命令放在 URL fragment 的 `payload` JSON 中，生成器负责转义。新会话只填写可编辑草稿，终端命令需要用户确认，导入需要预览并确认。不要在导入链接中放入 API key、密码、token、认证 headers 或环境变量；凭据应在应用中填写。Fragment 仍可能被复制或留在浏览器历史中。

当前实现和验收记录见 [implementation-status.md](implementation-status.md)，线上关联与发布证据见 [distribution-verification.md](distribution-verification.md)。Play 正式关联还需要真实 Play App Signing 证书；本地生成的 direct 证书不代表 Play 签名。
