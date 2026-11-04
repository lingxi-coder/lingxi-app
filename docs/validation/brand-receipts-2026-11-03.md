# 品牌门历史证据收据复核（2026-11-03）

本记录仅覆盖已提交的历史验收结果、测试探针和工件身份。复核人逐条核对了提案的 73 条精确行与命中跨度；这些记录不能证明当前构建、签名、设备或在线验收通过。

复核提案 JSON SHA-256：`a69e14b6a49af4d8fe7915860cb7f28250c0e4edc58af683f09ac79e2f914fab`。提案逐行文本 SHA-256：`8db42b581a2d72fa3e125ace10b7520c61f52222f7f33a44370d5c53ba24cab9`。模拟检查 JSON SHA-256：`b75d50e375c2c0d8789e83a39edf5a42b5c3ae7f752bf24709bed8f49c4ac87c`。提案时产品提交：`b128c402960eba90fbaa25741f774a608f4333ce`；登记前逐文件重新核对了当前 HEAD 对应 Git blob 与工作区 SHA-256。

每条冻结只匹配指定文件中的完整行、精确出现次数和标记跨度；同一行的其他标记或新增行不受豁免。删除的两条旧收据对应已删除的源码文件和改用共享常量的测试夹具。仅移除扫描已不再发现的 8 条旧基线，没有新增基线。

| 历史文件 | Git blob | SHA-256 | 复核判定 |
| --- | --- | --- | --- |
| `acceptance/desktop-bot-takeover-20261028/android-device-bots-ui-unlocked.log.process.json` | `a94061146f6d433053240f6c6e94a545b479a375` | `936e849360ac0e0768cc4ae512fca203660481e960448a3eb46cd2692c0d7e68` | 进程回执，保留历史命令和路径 |
| `acceptance/desktop-bot-takeover-20261028/android-device-bots-ui.log.process.json` | `afa99ed6cddbdb8b7f71cd4e96d78cd0659f92f3` | `13799fb9d9a672c40ee7c61c6cc591a426c2d27a7b7b34962beb00f53e77c389` | 进程回执，保留历史命令和路径 |
| `acceptance/desktop-bot-takeover-20261028/android-terra-live.json` | `9dd9a471e9a068823245e5f525c041096fc3715f` | `588b0d19b546b154607245f8b705307df991c6a3b073d7084f48cc9cdda9dfa3` | 验收回执，保留工件或运行身份 |
| `acceptance/desktop-bot-takeover-20261028/cleanup-after-ios-20261029.json` | `3eb65a2e632e9e7ab2eaed8cd534e06a41e95b07` | `9aa93de0630dc61ab7b6c701240c3dda50f97d6c93ab64e978979c46ba0e8bdf` | 验收回执，保留工件或运行身份 |
| `acceptance/desktop-bot-takeover-20261028/cleanup-ios-precompile-20261029.json` | `732be96b3ef8d3fd37da01b3033e959952cb95c7` | `ce00bd0fe638175b69071d18acde03b50739095962fbfe892d185b4864d65525` | 验收回执，保留工件或运行身份 |
| `acceptance/desktop-bot-takeover-20261028/desktop-signed-package.json` | `80713fbef1dbb3e3610309982f32294849212422` | `3c7d003a9028f4cff17db57c5be50d7263196d767c081d566df9ae4cde663d20` | 验收回执，保留工件或运行身份 |
| `acceptance/desktop-bot-takeover-20261028/ios-device-binding-library-pair.json` | `4e9f0b893303bbdc2e8205dfb33763bac7aea3e3` | `c8982be6dccb1e43220eec94d7390541b2073b387f4bd83fd5026e89d5183862` | 验收回执，保留工件或运行身份 |
| `acceptance/desktop-bot-takeover-20261028/ios-device-bot-live-usb.log.process.json` | `006100e9d26ef01f7f8c8881cbc398431fbfa8f1` | `20887aae7a9c78722cd894d62f6858c62ce3537fff2f0cdb97125f357d33519c` | 进程回执，保留历史命令和路径 |
| `acceptance/desktop-bot-takeover-20261028/ios-device-remote-unit.log.process.json` | `cd2a40194a140ff2b67bf2a19e24878d70020654` | `e7656b69c8f82d653c14b4410676ca614925adba1756fc898bf65b336a9d0407` | 进程回执，保留历史命令和路径 |
| `acceptance/desktop-bot-takeover-20261028/ios-fulldebug-build-tests.log.process.json` | `6b808ad9574fb2b9b1b1c3d86eb5da747c1e3061` | `a5f4fa64e89bb94f6fcea77e054a3f46a367fef348c5bae9dad9ad639cbf6003` | 进程回执，保留历史命令和路径 |
| `acceptance/desktop-bot-takeover-20261028/ios-input-dedup-20261029.json` | `2d9fdec6e00a53d1d1270bb2b407f9a01ad70616` | `4ba7d8a1fef716d71250da430d58b44a46d6703f9f6aa7d025f4d451e6bc44be` | 验收回执，保留工件或运行身份 |
| `acceptance/desktop-bot-takeover-20261028/ios-runtime-driver-diagnostic.json` | `2bfb7b60c9dba11936500442b76a1cee01e79af2` | `ba25e08b604c9e2b4f0e0159dd601b21e88d48f3c087130bcdfc45dfa22f975c` | 验收回执，保留工件或运行身份 |
| `acceptance/desktop-bot-takeover-20261028/ios-terra-live.json` | `f16b997da1be1e980c1e122e83b74a82f972f321` | `b0849a6ecf9b900f88650a245d8983f0fd33a867ae9a41bdca373767a53c0bf8` | 验收回执，保留工件或运行身份 |
| `acceptance/desktop-bot-takeover-20261028/iphone-install.json` | `0cb65455375d3b7474eb521ce80aa9f52f0d197f` | `61ddef0362e91224ba07ba770d50203e6e0c187a227b102e2dcb96bfb33d207f` | 验收回执，保留工件或运行身份 |
| `acceptance/desktop-bot-takeover-20261028/results.json` | `8fbcb43c4bb798f7e58707a0b066fabda15f1505` | `c4c1c8f88a0d3b39696601e10c81665d35d3cf2301f5f848b98a9e51087864b3` | 验收回执，保留工件或运行身份 |
| `acceptance/teams-firestore-cache-20261030/checks.json` | `24337a49c81f8a6c16e2c0fd8c3c2c3e0357458a` | `4160f6d6771976062c715be8d53d502a08da44ef7fdfef1c25b54d1f29f3bbd4` | 验收回执，保留工件或运行身份 |
| `acceptance/teams-t0-20261029/probes/android/build.gradle.kts` | `3d9c2a178ac12518610cd1b026a5381a9264ab40` | `f63647775a813d341c63a72672c16b0d968bbb548fff5ae78ebc45f120dd2557` | 测试探针，保留历史输入身份 |
| `acceptance/teams-t0-20261029/probes/android/src/main/java/com/lingxi/teamsspike/MainActivity.java` | `ef2397c62820e52fb17bf5a5729a11b0ddf4067e` | `9a2278edc34e0016fd90b6775fb84aa05cf409b64a69021b290d732a260a86e4` | 测试探针，保留历史输入身份 |
| `acceptance/teams-t1-20261029/probes/android/build.gradle.kts` | `dbce52e970a9bd93a446dc4a3bbf3b43f351ec11` | `bf89c0a7b95d2c808cc6aa84e64025bc79b77e3fb18e79989a314a926b2644af` | 测试探针，保留历史输入身份 |
| `acceptance/teams-t1-20261029/probes/android/src/main/AndroidManifest.xml` | `8b33434068438a8027d455d7292cb506c431485c` | `8a16ede8156f48f0f7178fdf6a6f53bf8841b1999f24170db8cb5593481019f7` | 测试探针，保留历史输入身份 |
| `acceptance/teams-t1-20261029/results.json` | `5ff3ee7b42cdfa5b2f048c47bdc0d67129256460` | `4da83f9ff5ef6987d7b9002fe1ca392827010ea66f4e20c7b4099e89995c27ba` | 验收回执，保留工件或运行身份 |
| `acceptance/teams-t2-20261029/results.json` | `2637b1ab8fd2757dfe2abefc5f9fa7ab846e33ba` | `12af6db8910a4b6518921d661e8b204cd5604a082846fd66a7421002704eca42` | 验收回执，保留工件或运行身份 |
| `acceptance/teams-t3-complete-20261029/binary-hashes.json` | `80b74d089fbef34376aa7a0b7610096028747923` | `877bf009072e5bacf2edff5a9c81d0359ce2e2413c2dc0d0dfb2a10c969f3943` | 验收回执，保留工件或运行身份 |
| `acceptance/teams-t3-complete-20261029/metadata/ios-simulator-signing.json` | `4bd08fb5223359016ea15fe4543f61f404010a91` | `c746261f8370aff1b5fdb4471857c15f6fb1387367d17c09346a03fcaf6de345` | 验收回执，保留工件或运行身份 |
| `acceptance/teams-t3-complete-20261029/metadata/signed-runtime.json` | `47ab64f2c450f56df5de9f4a5bb5f5dcb70d3e93` | `e95e4fb4f4ffa0313d1f0d6e9550cc2cdc4ab5f422983ab15a9196f3f361ff85` | 验收回执，保留工件或运行身份 |
| `acceptance/teams-t3-complete-20261029/probes/android/build.gradle.kts` | `b447060e1754d90c1df69750d40beec67ac280c8` | `6ad928ba2a33b61d092e24acf395362da8413e8f393afe00864f315271530aa8` | 测试探针，保留历史输入身份 |
| `acceptance/teams-t3-complete-20261029/probes/android/src/main/AndroidManifest.xml` | `78cab7dc037cf5f62ffa5454569427e1759c967d` | `ced0fdbba39a0915dae61a52301dd30b619a30bf0d289e089e1e7395c76c0382` | 测试探针，保留历史输入身份 |
| `acceptance/teams-t3-complete-20261029/probes/ios/UITests/TeamsT3ProbeUITests.swift` | `254e2cfcc08fdcd09db54f16c6220083abda29df` | `2252c34421ee25b06323b85ece3a2336c9f951323dbaaf7fe4b857b546089f1f` | 测试探针，保留历史输入身份 |
| `acceptance/teams-t3-complete-20261029/probes/runtime-fixture/README.md` | `419a18a9aa7fe3ac13f95b4a07c8d2556a4a086a` | `72e0ad34b2e09910240b93e3c147c0305b8ef4075b07a87e2d8f3bcd83fd35c2` | 测试探针，保留历史输入身份 |
| `acceptance/teams-t3-complete-20261029/probes/web/journey.js` | `5deb4d986cb97c29c41abcbcc611cb63b896459f` | `63bf5c9277531a19b7bd1b9c73719d53549523da1a004d7e810e0a52e44c8f4d` | 测试探针，保留历史输入身份 |
| `acceptance/teams-t3-process-20261029/results.json` | `b4b7ee7e4742d60c90469268b2c610ce157c85c6` | `2ad0d74c44dcb659532ab1cbbb6ded2fc140afc9b60bef51891522d5b84bc97f` | 验收回执，保留工件或运行身份 |
| `acceptance/teams-t3-provider-20261030/metadata/android-full-product.json` | `b08157277260bdaceeb936725c77112376a54636` | `8bef80200880565a5e9f39f01ea1c7df37c2d6e5a89a12a22e67aadff24aa6e7` | 验收回执，保留工件或运行身份 |
| `acceptance/teams-t3-provider-20261030/metadata/ios-full-product.json` | `4b4698a2301aa4c03e441f82f81f2b2bc5e398fe` | `f56158fbaac5f82df1a86a1979eb40c3e3d48fcee1521925acda18a117becc16` | 验收回执，保留工件或运行身份 |
| `acceptance/teams-t3-profile-20261029/report.md` | `25bb1baae3567ebaba126c60cfb0deddc9f43caa` | `2256154b797497ee1d241b7d9f946356c7dea5f417955b34ab2d9b316e9f5157` | 历史报告，保留被测环境 |

## 独立测试身份

另逐项复核 9 条精确收据，限定用于独立测试中核对真实配置键、辅助程序入口、进程环境、角色文件及缓存标记；这些预期保持独立，以便检测运行时接口的意外变化。

测试提案 JSON SHA-256：`01ae460a97c51197167903dbede52dfbda1dfcd252662ddddc2e8aeb9d684e58`。登记前核对以下文件的当前 HEAD Git blob 与工作区 SHA-256 一致。

| 测试文件 | Git blob | SHA-256 | 复核判定 |
| --- | --- | --- | --- |
| `apps/electron/test/team-runtime-build.test.mjs` | `73a2207eb7fc23d2161de79ccdbbba5341967189` | `624db2805972fccbab663fb0b265f368ce0591b9f54fe59bf010f9200cebcf3a` | 独立断言，保留实际接口或标记 |
| `apps/electron/test/teams-host-runtime.integration.ts` | `b90924cb6592f01ad06cb56f2f21703b9afed6c6` | `aa778302fca4914efabb4f386cdcf59724a4c5082dde36312ff83976e53c0dc6` | 独立断言，保留实际接口或标记 |
| `apps/electron/test/teams-host.test.ts` | `54d8074b8230f52185a9495172de12dd383000b9` | `65e2e5f308ee0f719dfc704e4bb93fc39676f1d0f7b1e861029877cfaf31d6f8` | 独立断言，保留实际接口或标记 |
| `scripts/tests/test_gstreamer_sdk.py` | `18808771dc07e7d1ba1c71e02dfad0e0b0a70c54` | `9017d6595ebed6635e0fc33a8ad96cac2f7b8d818d5ea737a421453ba36f0fc3` | 独立断言，保留实际接口或标记 |

## Cargo 图中的包身份

以下条目是锁定的 LLM Git 源码包身份；它们保留供独立源码审计使用，不是产品命名空间，也不代表审计通过。包名、版本和来源由离线锁图核验，提案 JSON SHA-256：`60e4929e813d67ce1769546943bde9f755c7b99e164f9f60d2f8dadb6b1abf60`。

策略文件 `supply-chain/config.toml` 的 Git blob 为 `f0ddc133cb584d59cf025657ed3dc33471528a5c`，SHA-256 为 `4d5f49a4464f34f1a849ac49f62b91062080cfc30466aa708d3d3ae7ed77009d`。

| 包名 | 锁定版本 | LLM Git 提交 |
| --- | --- | --- |
| `lingxi-hyper` | `1.9.0` | `e59c8fb33c73461fd5cd6865ebde623eca517a7e` |
| `lingxi-hyper-rustls` | `0.27.5` | `e59c8fb33c73461fd5cd6865ebde623eca517a7e` |
| `lingxi-hyper-util` | `0.1.21` | `e59c8fb33c73461fd5cd6865ebde623eca517a7e` |
| `lingxi-reqwest` | `0.12.29` | `e59c8fb33c73461fd5cd6865ebde623eca517a7e` |
