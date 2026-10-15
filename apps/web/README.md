# LingXi 官网与 SDK 文档

主页采用居中品牌与双入口布局；开发者文档采用顶部 SDK 导航、左侧目录、正文和右侧页内导航。界面提供中英文、文档深色模式、移动端目录、键盘搜索和代码复制。

```sh
npm ci
npm run dev -- --host 127.0.0.1
npm run typecheck
npm test
npm run build
```

入口为 `/`、`/docs` 和 `/docs/api-reference`。每个 SDK 文档页有独立 `/docs/<page-id>` 路由；部署 `dist/` 时须将页面请求回退到 `index.html`，同时正常提供静态资源与 `api-index.json`。

## 发布到 Cloudflare

`wrangler.jsonc` 将 `dist/` 作为静态资源发布到现有的 `lingxios-site` Worker，使用 `lingxios.net/*` 路由，并为文档路由启用单页应用回退。Cloudflare 管理该域名的 HTTPS 证书。

```sh
npm run docs:check
npm run typecheck
npm test
npm run build
npx wrangler@latest deploy
```

首次发布前需要运行 `npx wrangler@latest login` 登录有权管理该 Worker 的 Cloudflare 账户。若 `docs:check` 提示索引过期，先运行 `npm run docs:sync` 并检查生成的源码提交链接，再重新构建发布。

## 唯一正文与双语手册

`src/data/docs/` 是人工维护的双语技术手册正文，涵盖整体架构、Harness、模型服务、存储、扩展、安全、后台协作、Mobile Linux、产品协议、多端与运维。官网渲染、搜索、页面复制与仓库 Markdown 使用同一份数据；表格、步骤、图示、代码标签和来源均参与同步。

仓库入口为 [中文手册](../../docs/technical/zh/README.md) 和 [English handbook](../../docs/technical/en/README.md)。`docs/technical/` 是生成目录，不能直接编辑；`.generated-files.json` 记录工具拥有的产物，旧页仅在拥有列表内清理。历史设计与原生验收报告按原日期保留，不代表当前能力已验收。

```sh
npm run docs:sync
npm run docs:check
# 仅使用保存的正文、基线和索引，无需相邻 SDK checkout：
npm run docs:check:content
# 四仓工作区位于其他目录：
npm run docs:sync -- --workspace /path/to/workspace
npm run docs:check -- --workspace /path/to/workspace
```

完整同步更新手册源码基线、API 声明索引、读者消费者清单与中英文 Markdown。手册基线覆盖各章明确引用的源码/配置；API 索引指纹只覆盖选择的接口模块。两者排除各自生成输出，连续同步不因时间或本次产物变化而过期。文档 CI 使用保存的来源信息检查导出一致性，四仓源码新鲜度由完整工作区检查。

## 工作树与已提交 API 索引

```sh
# 正常 docs:sync/docs:check 显式使用 working-tree 模式
python3 scripts/generate-api-index.py --source-mode working-tree --workspace /path/to/workspace
# 独立审计已提交版本，不覆盖手册当前索引
python3 scripts/generate-api-index.py --source-mode committed --output /tmp/api-index.committed.json
python3 scripts/generate-api-index.py --source-mode committed --output /tmp/api-index.committed.json --check
```

直接调用 Python 默认 `committed`，使用各仓库 HEAD Git blobs；`working-tree` 读取符合范围的当前磁盘源码，包含已暂存、未暂存和非忽略新增文件，排除已删除或非 UTF-8 文件。只有与 HEAD 完全一致的文件生成固定提交行号链接；改动文件显示路径、行号及未提交标记。

每仓记录 base HEAD、所据输入是否改动、排序路径/内容指纹和产品当前声明的固定 revision。SDK 工作树、SDK HEAD 与产品固定依赖可以不同，页面并列展示；指纹不使未提交实现变成可通过 Git SHA 获取的发行版。源码位置和外部引用不嵌入用户本机绝对路径。

索引是选定公开声明、trait 方法及原生 facade 的检索目录，不执行宏展开、重导出解析或平台 cfg 求值；原始源码注释保留原语言，不能代替编译器 Rustdoc。大型 JSON 仅在 API 页面加载；Markdown 链接保存的 JSON 并说明本地检索入口。

## 编译示例与 Rustdoc

```sh
# 默认离线；消费者与依赖源码置于隔离临时目录，不改生产清单/锁文件
npm run docs:examples
# 完整类型/方法参考；仅成功的 profile 进入官网可点击列表
npm run docs:rustdoc
# 如仅需指定工作区或部分配置，运行对应脚本 --help
python3 scripts/check-doc-examples.py --help
python3 scripts/build-rustdoc.py --help
```

工作树示例使用正文中完整可复制的本地依赖 overlay；验证器检查实际 Cargo 来源身份，并编译正文代码。固定 Git 安装示例单独检查对应提交。SDK 既有 downstream-docs 继续验证专项指南。编译、模拟测试、真实服务和实体设备证据分别报告，不把接口摘录当作运行验收。

Rustdoc 输出位于被忽略的 `public/rustdoc/`，可用配置由 `public/rustdoc-manifest.json` 记录。只有成功生成且在当前文档包实际存在的产物有链接；开发服务器及静态构建过滤不存在的链接，缺失依赖或工具链记录为不可用；准备发布静态官网前应运行 Rustdoc 构建，避免将本机产物可用性带到缺少对应文件的部署环境。

网站浏览不需要模型凭据。Bridge 是产品内部 Node.js 包，下载、价格与控制台中的原型或发布状态继续明确标注。构建官网不部署模型/Agent API；本次文档交付不执行 Cloudflare 发布。
