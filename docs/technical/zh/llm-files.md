# 附件与供应商文件

区分宿主持久附件、账户限定的供应商上传文件、模型输入与生成产物。

<a id="identities"></a>

## 两类身份与独立生命周期

AttachmentRef 是宿主存储中的不可变引用，包含 attachment_id、revision、filename、media_type 与声明大小。对话历史保留它，其他设备通过宿主附件服务进行认证后显示。供应商文件 ID 与 URI 是请求时资源，受账户、端点、协议、用途与过期约束，不能替代宿主附件记录。

| 对象 | 归属与用途 | 失败边界 |
| --- | --- | --- |
| AttachmentRef | 宿主：持久原件与不可变修订 | 修订缺失或权限拒绝由 resolver 返回 |
| ProviderFileRef / ProviderFileSource | SDK 限定作用域的供应商资源与 codec 输入 | 账户、端点、用途不符或文件过期会校验失败 |
| ProviderFileContent | 支持下载操作时的原始字节 | 下载支持独立于上传或文本提取 |

<a id="resolve"></a>

## 解析宿主附件

1. 以不可变宿主 ID/修订存储原始字节，并在 AttachmentResolver 中校验调用者权限。
2. 配置 LlmClientBuilder::with_attachment_resolver，在消息中加入 ImageSource::Attachment、DocumentSource::Attachment 或 VideoSource::Attachment。
3. SDK 在 resolver I/O 前检查全部声明元数据与每次请求合计 64 MiB 上限，并发解析独立修订，检查返回字节数完全一致，再按所选连接/模型规划内联或原生上传输入。
4. 原始 ChatRequest 保持不变。持久化宿主引用，请求级准备字节与供应商引用不作为持久附件身份。

### 使用现有已授权宿主 resolver 创建客户端

```rust
use std::sync::Arc;
use lingxi_llm_client::{builtin_providers, AttachmentResolver, LlmClient, LlmClientBuilder};
use lingxi_llm_client::protocol::{AttachmentRef, ChatRequest, ContentBlock, ConversationMessage, ImageSource, Region};

pub fn attachment_client(
    resolver: Arc<dyn AttachmentResolver>,
    attachment: AttachmentRef,
) -> Result<(LlmClient, ChatRequest), Box<dyn std::error::Error>> {
    let profiles = builtin_providers()?.into_iter()
        .filter(|profile| profile.profile_name == "openai").collect::<Vec<_>>();
    let mut builder = LlmClientBuilder::new(&profiles)?.with_region(Region::International);
    builder.with_attachment_resolver(resolver);
    let client = builder.build()?;
    let mut message = ConversationMessage::user_text("Describe the attached image.");
    message.content.push(ContentBlock::Image {
        source: ImageSource::Attachment { attachment },
    });
    let mut request = ChatRequest::new("gpt-4.1-mini");
    request.messages.push(message);
    Ok((client, request))
}
```

验证方式: 编译检查 (llm-attachment-client)

宿主 resolver 必须读取精确且已授权的修订。validate_content_reuse() 需要显式启用；SDK 在使用缓存字节或共享读取前调用它。默认禁止跨请求字节复用，resolve() 因而仍承担每次请求的权限检查。

<a id="provider-management"></a>

## 显式管理供应商文件

先绑定类型化供应商客户端，再使用 files() 资源。每次操作捕获一个配置快照，并要求非空 file_account_scope（或 account_scope）。宿主已有 Transport、profile 和 Authenticator 时，可使用底层 FileService。FileCapabilities 按供应商身份、实际端点/协议、媒体类型、用途与模型判断；兼容 OpenAI Chat 的端点不代表有 Files API。

- upload_stream 只消费 UploadFileStream 一次。声明大小必须与精确 EOF 一致；不支持的路由在读取输入前拒绝。自定义传输需实现 send_stream 才能增量上传。
- 下载、元数据、列表、list_for_purpose、删除与 extract_text 分别检查能力。分页游标保持不透明。Foundry Files 需要显式 Anthropic hosting 与规范资源/账户作用域。

### 上传一个宿主持有的文件并保留作用域

```rust
use lingxi_llm_client::{FilePurpose, LlmClient, ProviderFileRef, RequestOptions, UploadFile};
use lingxi_llm_client::providers::OpenAiClient;

pub async fn upload_model_file(
    client: &LlmClient,
    file: &UploadFile,
    options: &RequestOptions,
) -> Result<ProviderFileRef, Box<dyn std::error::Error>> {
    // options must contain this account's credential and nonempty file_account_scope.
    let provider = client.provider::<OpenAiClient>("openai")?;
    let files = provider.files();
    let reference = files.upload(file, FilePurpose::ModelInput, options).await?;
    let metadata = files.get(&reference, options).await?;
    Ok(metadata.file)
}
```

验证方式: 编译检查 (llm-provider-upload)

<a id="retention-recovery"></a>

## 留存、不确定结果与恢复

- 稳定 file_account_scope 允许符合条件的自动上传按账户隔离复用。缺少时上传限定于单次尝试。Qwen 自动上传仍限定于请求，并显式清理，因为服务器文件不会自动过期。
- 符合条件的 OpenAI、Anthropic 与 xAI 自动上传采用 24 小时过期，23 小时后停止缓存复用。能定位到该文件的 404 仅在允许重放时触发一次重新自动上传；托管副作用与续接会限制重放。
- Gemini 处理仍未完成或无法确认就绪时，ProviderFileProcessing 携带作用域限定的引用。保留并调用 resume_gemini_processing，不要为了重新轮询而再次上传。
- FileUploadError::OutcomeUnknown 表示发送后无法确定供应商是否接受。可用时通过元数据/列表核对。with_timeout 为每个底层文件操作设置总预算；with_deadline 在多次调用间共享绝对截止时间。两者都不增加重试。
- 显式上传由宿主管理。自动 Qwen 文件在响应完成后尝试删除，中断或删除失败会后台重试；进程退出仍可能遗留孤立文件，宿主应定期核对。

实现证据：provider_files、provider_file_clients、file_stream_upload 与 file_deadlines 测试使用模拟传输覆盖作用域、字节上限与不确定结果。真实账户留存、上传权限与跨设备附件存储需单独验收。

## 源码与文档基线

[源码仓库](https://github.com/lingxi-coder/llm-client)

- [llm-client/src/client/mod.rs](https://github.com/lingxi-coder/llm-client/blob/ce32fd07402c13fbfcfa185942162a87907ea0a1/src/client/mod.rs) — 已提交源码 (ce32fd07402c13fbfcfa185942162a87907ea0a1)
- [llm-client/src/client/builder.rs](https://github.com/lingxi-coder/llm-client/blob/ce32fd07402c13fbfcfa185942162a87907ea0a1/src/client/builder.rs) — 已提交源码 (ce32fd07402c13fbfcfa185942162a87907ea0a1)
- [llm-client/src/client/attachments/resolve.rs](https://github.com/lingxi-coder/llm-client/blob/ce32fd07402c13fbfcfa185942162a87907ea0a1/src/client/attachments/resolve.rs) — 已提交源码 (ce32fd07402c13fbfcfa185942162a87907ea0a1)
- [llm-client/src/client/attachments/prepare.rs](https://github.com/lingxi-coder/llm-client/blob/ce32fd07402c13fbfcfa185942162a87907ea0a1/src/client/attachments/prepare.rs) — 已提交源码 (ce32fd07402c13fbfcfa185942162a87907ea0a1)
- [llm-client/src/client/attachments/cache.rs](https://github.com/lingxi-coder/llm-client/blob/ce32fd07402c13fbfcfa185942162a87907ea0a1/src/client/attachments/cache.rs) — 已提交源码 (ce32fd07402c13fbfcfa185942162a87907ea0a1)
- [llm-client/src/files/service.rs](https://github.com/lingxi-coder/llm-client/blob/ce32fd07402c13fbfcfa185942162a87907ea0a1/src/files/service.rs) — 已提交源码 (ce32fd07402c13fbfcfa185942162a87907ea0a1)
- [llm-client/src/files/policy.rs](https://github.com/lingxi-coder/llm-client/blob/ce32fd07402c13fbfcfa185942162a87907ea0a1/src/files/policy.rs) — 已提交源码 (ce32fd07402c13fbfcfa185942162a87907ea0a1)
- [llm-client/src/files/types.rs](https://github.com/lingxi-coder/llm-client/blob/ce32fd07402c13fbfcfa185942162a87907ea0a1/src/files/types.rs) — 已提交源码 (ce32fd07402c13fbfcfa185942162a87907ea0a1)
- [llm-client/src/providers/file_resources.rs](https://github.com/lingxi-coder/llm-client/blob/ce32fd07402c13fbfcfa185942162a87907ea0a1/src/providers/file_resources.rs) — 已提交源码 (ce32fd07402c13fbfcfa185942162a87907ea0a1)
- [llm-client/tests/provider_files.rs](https://github.com/lingxi-coder/llm-client/blob/ce32fd07402c13fbfcfa185942162a87907ea0a1/tests/provider_files.rs) — 已提交源码 (ce32fd07402c13fbfcfa185942162a87907ea0a1)
- [llm-client/tests/file_stream_upload.rs](https://github.com/lingxi-coder/llm-client/blob/ce32fd07402c13fbfcfa185942162a87907ea0a1/tests/file_stream_upload.rs) — 已提交源码 (ce32fd07402c13fbfcfa185942162a87907ea0a1)
- [llm-client/tests/file_deadlines.rs](https://github.com/lingxi-coder/llm-client/blob/ce32fd07402c13fbfcfa185942162a87907ea0a1/tests/file_deadlines.rs) — 已提交源码 (ce32fd07402c13fbfcfa185942162a87907ea0a1)

- llm-client: HEAD ce32fd07402c13fbfcfa185942162a87907ea0a1; 所据文件指纹 sha256:a78ca8575ddb70589a8cc04ff4f043825bd52db40d751181584ea10118bfe2b4; 产品固定版本 87db6e234e32cbc251a128d6b370cdda36c4d0c0

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
