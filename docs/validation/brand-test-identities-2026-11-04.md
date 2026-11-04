# 品牌门独立测试和包身份复核（2026-11-04）

复用先前已逐项批准的 3 条独立测试精确行及标记：两条核对 Team runtime 构建的真实环境键，一条核对 GStreamer 缓存标记。它们不豁免同一行中其他命中，也不代表程序运行验收。原独立测试提案 SHA-256：`01ae460a97c51197167903dbede52dfbda1dfcd252662ddddc2e8aeb9d684e58`。登记前重新核对以下 Git blob 与工作区 SHA-256。

| 文件 | Git blob | SHA-256 |
| --- | --- | --- |
| `apps/electron/test/team-runtime-build.test.mjs` | `73a2207eb7fc23d2161de79ccdbbba5341967189` | `624db2805972fccbab663fb0b265f368ce0591b9f54fe59bf010f9200cebcf3a` |
| `scripts/tests/test_gstreamer_sdk.py` | `18808771dc07e7d1ba1c71e02dfad0e0b0a70c54` | `9017d6595ebed6635e0fc33a8ad96cac2f7b8d818d5ea737a421453ba36f0fc3` |

另复用先前批准的 4 条 Cargo policy 包名精确行及完整匹配跨度。它们是当前锁定 Git 依赖的真实包身份，不能作为产品命名空间，也不代表源码审计通过。当前 `supply-chain/config.toml` Git blob 为 `649738d30e28bea189433654a9a63ed47bb2b784`、SHA-256 为 `97426da02113c0ea1a2ce1e1f92b0653a54bc652a56a06d93c995a2bcfe479bf`；`Cargo.lock` SHA-256 为 `8107432e39037dbd710de6b78bdb706d8aebdb63ab0d4175a94d487ee3e153c6`。锁定离线 `cargo metadata --all-features` 及对应 Git vendor manifest 均确认以下名称、版本和来源：

| 包 | 版本 | llm-client 完整提交 |
| --- | --- | --- |
| `lingxi-hyper` | `1.9.0` | `3cb37e3a3b840b54a4e92994fdc8b1edf7f592d3` |
| `lingxi-hyper-rustls` | `0.27.5` | `3cb37e3a3b840b54a4e92994fdc8b1edf7f592d3` |
| `lingxi-hyper-util` | `0.1.21` | `3cb37e3a3b840b54a4e92994fdc8b1edf7f592d3` |
| `lingxi-reqwest` | `0.12.29` | `3cb37e3a3b840b54a4e92994fdc8b1edf7f592d3` |
