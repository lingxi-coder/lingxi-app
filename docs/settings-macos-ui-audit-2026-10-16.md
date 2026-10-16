# Desktop 设置界面 macOS UI 检查

本轮覆盖当前设置导航的全部 18 页，按 apple-design 检查布局、层级、系统字体、控件反馈、主题和键盘操作。基于真实 Electron 渲染器及合成桥接数据完成检查；不代表真实账号、Keychain、音频设备或远端服务验收。

参考 Apple 的 [Settings](https://developer.apple.com/design/human-interface-guidelines/settings)、[Toggles](https://developer.apple.com/design/human-interface-guidelines/toggles) 和 [Accessibility](https://developer.apple.com/design/human-interface-guidelines/accessibility) 指南。

## 已修正

| 问题 | 修正 |
| --- | --- |
| 共享设置行强制横排，长控件会压缩标题并可能溢出 | 标签保持可读宽度，控件可换行、受容器宽度约束；工具模型覆盖输入独立适配 |
| 卡片第一行又绘制顶部分隔线 | 去除第一行重复边框，保留内部行分隔 |
| 搜索框移除默认轮廓，部分控件没有统一焦点反馈 | 搜索容器、按钮、字段、链接和折叠项使用主题色焦点轮廓；补充按下与悬停反馈 |
| 工具和 Skills 开关没有辅助功能名称，保存中仍可聚焦点击 | 开关必须声明名称，保存时使用真正的 disabled；保留 switch/aria-checked |
| 开关位移及其他设置动画没有统一响应减少动态效果 | 开关使用 transform；设置范围统一禁用非必要过渡和动画 |
| 范围选择没有向辅助技术声明当前状态 | 加入组名和 aria-pressed；主题选择也提供组名 |
| 原生字段和独立模型弹层没有明确主题继承 | 设置根及 Fusion 弹层声明 color-scheme，弹层具有焦点和增强对比度样式 |
| 深色扩展主按钮直接用明亮蓝色承载白色小字 | 单独调整按钮填充蓝色，保留图标和链接的原强调色；主按钮文字对比度至少 4.5:1 |
| 设置读取错误位于窗口顶部，可能与系统窗口控件重叠 | 移入内容区域，使用主题表面与错误文字，保留 alert 和重试操作 |
| 增强对比度时共享卡片、表单边框和选择状态不够明确 | 加强卡片/字段/行边界，给选中导航及开关增加可见轮廓 |

## 页面覆盖

| 分类 | 页面 | 检查内容 |
| --- | --- | --- |
| 个人 | 通用、账户、外观、通知、语音、已归档会话、项目与信任 | 快捷入口、账户登录表单、主题分段选择、通知预览和依赖禁用、语音表单、归档搜索/恢复、项目状态 |
| 模型与服务 | Provider 凭据、自定义 Provider 与路由、Fusion 多模型合议 | Provider 列表及详情、请求格式、凭据字段、自定义编辑与导入、策略表单及高级字段 |
| 编码 | 权限、工具与 Agent 行为、Skills、MCP 服务器、Hooks、插件与市场 | 权限规则、开关及模型覆盖、列表/详情、Skills 弹窗、MCP 结构化编辑、Hooks 编辑、插件配置与操作确认 |
| 高级 | 诊断、关于 | 状态与配置文件、操作禁用、版本与隐私入口 |

所有页面在 light/dark × 1000×720/760×720 下检查页面顶部及滚动到底部的横向溢出，共 72 个基础组合。对语音、工具、权限、诊断和 Provider 列表额外保存底部截图。

自定义 Provider、Provider 凭据、Fusion、MCP、Hooks 和 Skills 另在 light/dark × 760×600 下展开，检查编辑器、单行输入高度及弹窗边界，共 12 个展开组合。现有交互回归继续覆盖插件详情、凭据保存/导入/失败恢复、项目切换、未保存修改保护和嵌套焦点管理。

通过截图总览检查全部页面的明暗外观，并放大复查展开编辑器、Provider 凭据及长表单底部。

## 验证

- `npm run typecheck`：Node 和 Web 类型检查。
- `npm run build`：Electron 主进程、preload 和 renderer 生产构建。
- 设置相关 Electron 回归：25 项整轮通过，含新增的全页布局、辅助功能偏好、六类展开编辑器检查，以及错误提示避开标题栏的几何断言。
- 设置导航、共享行、客户端页、编码页、解析逻辑、Provider、Voice、Fusion 和旧代码守卫：127 项。
- `git diff --check`。
- 合并交付前在隔离 worktree 再次验证：类型检查、生产构建、129 项单测和 28 项 Electron 回归全部通过（含 desktop 菜单、终端、提及菜单和定时任务相关检查）。

交互回归命令（工作目录 `apps/electron`）：

```sh
node --import ../../packages/bridge-client/node_modules/tsx/dist/loader.mjs \
  --test --test-concurrency=1 \
  test/settings-screen.test.mjs \
  test/settings-background-interaction.test.mjs \
  test/settings-transaction-interaction.test.mjs \
  test/archived-settings-interaction.test.mjs
```

设置 `LINGXI_SETTINGS_SCREENSHOT_DIR=/tmp/lingxi-settings-audit-final` 可保留截图。该目录为本机临时证据，不随 Git 提交。

本轮还更新了旧的凭据事务测试夹具，使其提供当前设置页要求的 settings snapshot、真实全局样式和正确的浅色主题值。没有修改账户或凭据持久化逻辑。

## 验证边界

未执行签名安装包、VoiceOver 实机操作、系统弹出选择器的原生像素对照、真实 OAuth/邮件发送、Keychain 写入或语音模型下载与设备录音。页面内真实服务返回的任意错误、所有用户数据长度和全部第三方插件配置组合无法由这些固定样例穷尽。减少透明度下，这些设置主体使用已有不透明表面，无新增模糊材质。

交付范围为本轮 desktop 和设置 UI 修改；其他正在进行的移动端修改保留在原工作区。
