# 附件、音频与实时宿主

经显式供应商能力与原生资源负责人路由多模态输入和音频。

<a id="attachment-ingress"></a>

## 附件输入与模型能力

RunInput.images 是宿主路径数组。ClientCommand SendPrompt 使用内联 {media_type, base64} 图像记录；宿主适配器先将字节写入临时宿主文件，再调用面向路径的引擎接口。恢复后的 image DTO 携带稳定来源，不按提示文本匹配。

模型／供应商能力决定原生视觉输入或已配置的视觉委托路径。文件可读性、大小／类型处理与供应商编码是独立检查。RunInput.images 并不隐含视频、PDF 或任意附件支持，应使用明确支持处理的宿主／工具接口。

<a id="audio-routing"></a>

## 设备、离线与云端音频

harness_runtime::audio 重新导出独立 audio-runtime 组合与共享 AudioService 契约。UnifiedAudioService 管理逐操作路由、可用设备／离线服务和可选 cloud host。Automatic 先选择支持的系统后端，再选已安装兼容离线后端，不隐式选云端。采集、录音与播放仍属于原生资源。

显式 Cloud 路由绑定可信的精确会话 profile／账户或显式 profile，并选择音频专用模型。模型提供的工具参数不能选择凭据身份。update_settings 原子替换路由，已执行调用保留选定快照。声明的工具动作遵循当前 owner 专属能力与就绪状态。

### Rust · 组合本地自动音频

```rust
use std::sync::Arc;
use harness_runtime::audio::{AudioRoutingSettings, AudioService, UnifiedAudioService};

fn local_audio(device: Arc<dyn AudioService>) -> UnifiedAudioService {
    UnifiedAudioService::new(
        device,
        Vec::new(),
        None,
        AudioRoutingSettings::default(),
    )
}
```

验证方式: 编译检查 (harness-audio-composition)

<a id="host-wiring"></a>

## 音频可用性由宿主提供

DesktopConfig.audio 由 Bridge 组合通过连接注入。CLI、TUI 与离线桌面 factory 不提供，因此没有音频工具。Android／iOS 宿主实现原生回调和能力快照；原生麦克风权限、已安装模型及资源就绪状态决定会话可声明哪些操作。

AudioOwner 与 operation ID 将资源关联到活动会话。会话切换和引擎销毁以有界清理结束所属音频及 recording handle。供应商 profile／账户选择在边界重新检查，过期 owner 不能复用新会话音频凭据。

<a id="realtime-owner"></a>

## 实时会话归属

实时能力将 SDK session／control／event driver 与宿主音频 Agent run 集成。原生采集与播放属于宿主；供应商事件解码和实时传输属于 llm-client。Runtime 命令经共享客户端契约提供上下文及 start／input／stop 操作。

移动 realtime owner 约束准入与清理：所属 run 活动时拒绝冲突的 NewSession；权限响应使用同一绑定 gate。Stop／cancel 关闭流并释放原生资源后才能替换会话。传输关闭、音频失败与取消需明确结果，不能静默结束采集。

<a id="audio-acceptance"></a>

## 验证边界

- 现有测试用路由 fixture 与模拟实时 driver 验证 owner 准入、能力选择、权限路由和清理。仅编译示例证明接口可用，不证明声音输出。
- 采集／播放延迟、电话中断、蓝牙路由、权限、离线模型质量及真实云端音频／实时请求需设备／服务验收。保留模型／profile、系统版本和资源清理证据。

### 音频路由与注入宿主检查

```bash
cargo test --locked -p audio-runtime --lib
cargo test --locked -p harness-runtime --features desktop --test audio_injection
cargo test --locked -p harness-runtime --features mobile --test realtime_owner
```

验证方式: 需环境验证

## 源码与文档基线

[源码仓库](https://github.com/lingxi-coder/harness-runtime)

- [harness-runtime/crates/runtime/src/api.rs](https://github.com/lingxi-coder/harness-runtime/blob/1b202fef29353f48026c59712207b89d7d877273/crates/runtime/src/api.rs) — 已提交源码 (1b202fef29353f48026c59712207b89d7d877273)
- [harness-runtime/crates/client/src/protocol/commands.rs](https://github.com/lingxi-coder/harness-runtime/blob/1b202fef29353f48026c59712207b89d7d877273/crates/client/src/protocol/commands.rs) — 已提交源码 (1b202fef29353f48026c59712207b89d7d877273)
- [harness-runtime/crates/orchestrator/src/image_input.rs](https://github.com/lingxi-coder/harness-runtime/blob/1b202fef29353f48026c59712207b89d7d877273/crates/orchestrator/src/image_input.rs) — 已提交源码 (1b202fef29353f48026c59712207b89d7d877273)
- [harness-runtime/crates/audio-runtime/src/lib.rs](https://github.com/lingxi-coder/harness-runtime/blob/1b202fef29353f48026c59712207b89d7d877273/crates/audio-runtime/src/lib.rs) — 已提交源码 (1b202fef29353f48026c59712207b89d7d877273)
- [harness-runtime/crates/audio-runtime/src/cloud.rs](https://github.com/lingxi-coder/harness-runtime/blob/1b202fef29353f48026c59712207b89d7d877273/crates/audio-runtime/src/cloud.rs) — 已提交源码 (1b202fef29353f48026c59712207b89d7d877273)
- [harness-runtime/crates/runtime/src/desktop/configuration.rs](https://github.com/lingxi-coder/harness-runtime/blob/1b202fef29353f48026c59712207b89d7d877273/crates/runtime/src/desktop/configuration.rs) — 已提交源码 (1b202fef29353f48026c59712207b89d7d877273)
- [harness-runtime/crates/runtime/src/mobile/audio_service.rs](https://github.com/lingxi-coder/harness-runtime/blob/1b202fef29353f48026c59712207b89d7d877273/crates/runtime/src/mobile/audio_service.rs) — 已提交源码 (1b202fef29353f48026c59712207b89d7d877273)
- [harness-runtime/crates/runtime/src/mobile/host/audio_context.rs](https://github.com/lingxi-coder/harness-runtime/blob/1b202fef29353f48026c59712207b89d7d877273/crates/runtime/src/mobile/host/audio_context.rs) — 已提交源码 (1b202fef29353f48026c59712207b89d7d877273)
- [harness-runtime/crates/runtime/src/mobile/host/audio_realtime_owner.rs](https://github.com/lingxi-coder/harness-runtime/blob/1b202fef29353f48026c59712207b89d7d877273/crates/runtime/src/mobile/host/audio_realtime_owner.rs) — 已提交源码 (1b202fef29353f48026c59712207b89d7d877273)
- [harness-runtime/crates/runtime/tests/mobile/realtime_owner.rs](https://github.com/lingxi-coder/harness-runtime/blob/1b202fef29353f48026c59712207b89d7d877273/crates/runtime/tests/mobile/realtime_owner.rs) — 已提交源码 (1b202fef29353f48026c59712207b89d7d877273)
- [harness-runtime/crates/runtime/tests/desktop/audio_injection.rs](https://github.com/lingxi-coder/harness-runtime/blob/1b202fef29353f48026c59712207b89d7d877273/crates/runtime/tests/desktop/audio_injection.rs) — 已提交源码 (1b202fef29353f48026c59712207b89d7d877273)

- harness-runtime: HEAD 1b202fef29353f48026c59712207b89d7d877273; 所据文件指纹 sha256:e9079905b280000c8013dd7f16954f3709e464fdcf67fca25de7e73176e1230a; 产品固定版本 1b202fef29353f48026c59712207b89d7d877273

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
