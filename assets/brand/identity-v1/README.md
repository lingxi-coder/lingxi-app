# 灵犀 LingXi · 品牌视觉系统 v1

本目录是一套覆盖灵犀产品系列的设计提案。打开 `index.html` 可查看品牌展示、切换产品、中英文 label 与深浅主题。资源均保存在当前目录，预览不依赖外网。

## 设计

**灵犀结 / Resonance Link** 延续现有 Web 双环的连接意象：将双环调整为较紧凑的圆弧，并向右上方倾斜 35°。两个相连的环对应人与智能、理解与行动；使用单色即可成立。母品牌使用灵犀蓝 `#4167ED`，搭配墨色 `#202632`、纸白 `#F6F5F0`。

应用图标使用相同母标志。48 px 及以上的系列图标加入一个功能符号：Code 的尖括号、Chat 的对话框、Control 的指针、Apps 的网格、Remote 的双向箭头、Harness 的层叠、SDK 的代码括号。16–32 px 使用简化母标志，并与可见的产品名称一起出现，避免只依靠颜色区分产品。

## 名称与 label

英文母品牌固定写作 **LingXi**，L 与 X 大写；中文为 **灵犀**，繁体为 **靈犀**。双语组合使用「灵犀 LingXi」。`brand.json` 为结构化设计清单，`brand-data.js` 是它的预览副本，两者由 `build.py` 同步生成。

| 原项目 / 用途 | 英文完整名称 | 中文完整名称 | 中文启动标签 | 英文启动标签 |
|---|---|---|---|---|
| lingxi-app | LingXi Code | 灵犀 Code | 灵犀 | LingXi |
| chat | LingXi Chat | 灵犀聊天 | 灵犀聊天 | LingXi Chat |
| computer-use | LingXi Control | 灵犀操控 | 灵犀操控 | LingXi Control |
| local-app-market | LingXi Apps | 灵犀应用 | 灵犀应用 | LingXi Apps |
| Remote Control 功能 | LingXi Remote | 灵犀远控 | — | — |
| harness-runtime 组件 | LingXi Harness | 灵犀 Harness | — | — |
| SDK 系列 | LingXi SDK | 灵犀 SDK | — | — |

`Control / 操控` 是 Computer Use 的对外命名提案。主产品保留 `LingXi Code` 完整名称，简短启动标签与移动端现有「灵犀」用法对齐。Remote 是产品内功能，Harness / SDK 是开发者组件；它们的图标用于入口、文档和模块标识，不代表增加独立应用。

SDK 子项使用 `LingXi WebRTC`、`LingXi Mobile Linux`、`LingXi LLM Client`，沿用 SDK 图标。其他语言界面可使用英文完整名称作为默认 label，避免随意翻译产品后缀。品牌名使用文本显示时保留空格：`LingXi Chat`，不写成 `LingxiChat`。

可访问名称使用完整产品名，例如 `aria-label="LingXi Chat"`。图标与同名可见文字相邻时图标设为空 alt；独立图标按钮按动作描述，例如「打开灵犀聊天」。环境标识独立放置为 `Beta`、`Dev` 等徽标，不拼入字标图形。

## 文件

- `svg/mark-{blue,ink,white}.svg`：透明背景主标志，SVG viewBox 为 128 × 128。
- `svg/lockup-{en,zh,bilingual}-{ink,white}.svg`：英文、中文、双语组合字标；字体已转路径。
- `svg/wordmark-en-{ink,white}.svg`：英文文字独立版。
- `svg/app-master.svg` 与 `png/lingxi-app-1024.png`：不预切圆角的 1024 px 不透明应用图标母版。
- `svg/app-preview.svg`：带圆角的展示版。
- `svg/maskable-icon.svg`：额外留安全区的图标；`svg/adaptive-foreground.svg` 为同一几何的透明前景。
- `svg/favicon.svg`、`png/lingxi-{16,32,48,64,128,256,512}.png`：小图标与网页素材。
- `products/<id>/`：各产品的彩色图标、简化图标、无圆角母版、深浅底字标，以及 16–1024 px PNG。
- `brand.json`：名称、角色、颜色和几何清单；`tokens.css`：CSS 色值。
- `index.html`、`preview.css`、`preview.js`、`brand-data.js`：交互式品牌预览。
- `lingxi-brand-kit.zip`：完整资源包。

所有正式 SVG 字标只含路径，不要求接收方安装字体。字标采用本机 Avenir Next Demi Bold 与 Hiragino Sans GB W6 的字形转曲；本包不分发字体文件。预览正文使用本机字体回退。

## 使用约定

1. 主标志最小尺寸 16 px；水平组合字标建议宽度不小于 96 px，中文或双语组合建议不小于 160 px。
2. 主标志周围保留至少 20/128 图形宽度的空间；不拉伸、不改变倾角，不在双环内放文字。
3. 浅底使用蓝色或墨色，深底使用白色；系列色用于产品图标与辅助强调。
4. 母品牌保持蓝色。系列色与产品名称固定对应，避免同一产品在不同平台换色。
5. 系列 `icon-1024.png` 为不透明、无预切圆角的输入母版；较小 PNG 为展示图标。平台集成时选择对应源文件并由各平台打包流程生成最终资源。
6. `maskable-icon.svg` / `adaptive-foreground.svg` 提供几何安全区素材，尚未接入具体平台资源目录或完成安装验证。不要把预览 PNG 当作平台集成完成的证据。

## 生成

需要 Python 3、fontTools、librsvg 的 `rsvg-convert`，以及本机对应字体。生成不需要网络。可传入 `--latin-font` / `--cjk-font` 改变字体路径；默认集合索引分别为 Avenir Next 的 2、5 与 Hiragino Sans GB 的 2。

```sh
python3 build.py --png
```

`build.py` 是矢量和 PNG 的生成源，预览页面单独维护。已有导出可直接使用，不需要运行生成脚本。

## 接入范围

设计素材统一放在产品仓库 `assets/brand/identity-v1/`。后续接入点包括主应用 Desktop / iOS / Android / Web、Computer Use、Chat、应用市场以及 SDK 文档。当前交付没有替换线上图标或应用名称，也没有改动 bundle ID、包名、URL scheme、仓库名、签名配置或发布渠道。
