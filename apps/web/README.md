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

## 文档内容

`src/data/docs/` 存放双语结构化文档，覆盖 Harness Runtime、LLM Client、Mobile Linux Rust / Kotlin / Swift 与产品内部 Bridge / 原生宿主集成。修改文档后运行测试，检查路由、标题、章节锚点、代码和搜索。

示例来自 SDK 文档与源码。安装示例使用经核对的 Git 提交；依赖升级时需同时更新文档中的版本与源码链接。没有声称代码摘录已完成真实模型服务或移动设备验收。Bridge 为内部 Node.js 包；原下载、价格和控制台页面仍保留各自的原型或发布状态说明。

## 更新 API 源码索引

```sh
npm run docs:sync
npm run docs:check
# SDK 工作区不在相邻目录时：
python3 scripts/generate-api-index.py --workspace /path/to/workspace
```

脚本默认读取 `~/lingxi/` 这类相邻工作区中的 `harness-runtime`、`llm-client`、`mobile-linux-runtime` 与 `lingxi-app`。读取每个仓库的已提交 `HEAD` 内容，输出 `public/api-index.json`；每条声明附有固定到完整提交 SHA 的源码行号链接。

索引覆盖选定 SDK 接口模块中的公开声明、trait 方法和原生 facade。它是可搜索的源码声明索引，不执行宏展开、重导出解析或平台 cfg 求值，也不代替 Rust 编译器生成的完整 rustdoc。大型索引仅在打开 API 参考页时加载。

不需要网络服务或模型凭据即可浏览网站和文档。构建与发布官网不会部署原控制台的示例 API 服务。
