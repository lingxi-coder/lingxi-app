//! 本地应用的 LLM 接缝（v3）：只剩运行中应用的自由文本 `chat` 调用。
//!
//! 设计器/生成管线（出题 / 出方案 / 写码）已整体移除——应用源码现在由
//! 会话中的 agent 直接编辑，构建走 MCP `build` 工具。模型经
//! [`LocalAppsModel`] trait 注入，使桥接逻辑能脱离真实 `ApiService` 单测。
//!
//! `AppError::LlmUnavailable` 用于模型够不到（离线/鉴权失败/超时）；
//! `AppError::LlmOutputRejected` 用于模型答了但答案不合格。两者在客户端
//! 渲染不同文案、提供不同操作，绝不能混用。

use async_trait::async_trait;
use llm_client::{ApiService, ContentBlock};
use local_apps::AppError;
use protocol::{ConversationMessage, MessageId};
use std::sync::{Arc, RwLock};

/// Who wrote one turn of an app-initiated chat.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatRole {
    /// The app's own user-side prompt.
    User,
    /// A previous answer the app is replaying for context.
    Assistant,
}

/// One piece of a chat turn. Media arrives already decoded and
/// size-checked by the bridge; this layer only shapes it for the provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatPart {
    /// Plain text.
    Text(String),
    /// An image the model should look at (vision).
    Image {
        /// MIME type, e.g. `image/jpeg`.
        media_type: String,
        /// Base64-encoded bytes, no `data:` prefix.
        base64: String,
    },
    /// A document (PDF) the model should read.
    Document {
        /// MIME type, e.g. `application/pdf`.
        media_type: String,
        /// Base64-encoded bytes, no `data:` prefix.
        base64: String,
    },
}

/// One turn of an app-initiated chat.
///
/// Multi-part so a photo the app just captured can be asked about directly.
/// There is deliberately no audio part: the conversation protocol has no
/// audio content block and no provider on this stack accepts raw audio in a
/// messages call, so an app that wants speech input transcribes it first
/// (`device.transcribeSpeech`) and sends text — a silently dropped audio
/// attachment would be far worse than a typed refusal.
#[derive(Debug, Clone)]
pub struct ChatMessage {
    /// Who wrote it.
    pub role: ChatRole,
    /// Ordered parts. Apps send no tool results.
    pub content: Vec<ChatPart>,
}

/// A free-text model call a RUNNING app asked for (`window.lingxi.v2.llm`).
///
/// Deliberately smaller than the provider surface: the model and profile are
/// NOT part of it — an app always rides whatever the user currently has
/// selected (`ApiServiceModel`'s live selection), so an app can neither pin
/// an expensive model nor route around the user's `/model` choice. Tools are
/// absent for the same reason a running app cannot reach the orchestrator: a
/// page's prompt is untrusted input, and giving it tool calls would hand
/// prompt-injected text an execution surface.
#[derive(Debug, Clone)]
pub struct ChatRequest {
    /// Optional system prompt written by the app's own code.
    pub system: Option<String>,
    /// Conversation so far, oldest first.
    pub messages: Vec<ChatMessage>,
    /// Output budget. An app-initiated call spends the USER's quota on the
    /// app's behalf, so it always carries an explicit cap.
    pub max_tokens: u32,
    /// Optional sampling temperature.
    pub temperature: Option<f32>,
}

/// What a [`ChatRequest`] produced.
#[derive(Debug, Clone)]
pub struct ChatOutcome {
    /// The answer's text blocks, concatenated (thinking excluded).
    pub text: String,
    /// Provider stop reason, when reported.
    pub stop_reason: Option<String>,
}

/// The injectable model seam. The implementation owns auth, routing, retry
/// and timeout — callers only see a request in, an answer or a typed error
/// out.
#[async_trait]
pub trait LocalAppsModel: Send + Sync {
    /// One free-text call on behalf of a RUNNING app. No tool, no schema —
    /// see [`ChatRequest`] for what an app may and may not control.
    ///
    /// Required rather than defaulted: a double that silently answered with
    /// canned text would make a broken wiring look green, so every
    /// implementation states its behaviour.
    async fn chat(&self, request: ChatRequest) -> Result<ChatOutcome, AppError>;

    /// Update the default model/profile future calls route through —
    /// `ClientCommand::SetModel` calls this so app-initiated `llm.chat`
    /// follows a `/model` switch instead of staying pinned to whatever was
    /// live at engine build time. Default is a no-op: only
    /// [`ApiServiceModel`] (the production implementation) has a live
    /// selection to update; test doubles ignore it.
    fn set_model(&self, _model: String, _profile: Option<String>) {}
}

/// Lower one app-supplied part to the conversation vocabulary.
fn chat_part_block(part: ChatPart) -> protocol::ContentBlock {
    match part {
        ChatPart::Text(text) => protocol::ContentBlock::Text { text },
        ChatPart::Image { media_type, base64 } => protocol::ContentBlock::Image {
            source: protocol::ImageSource::Base64 {
                media_type,
                data: base64,
            },
        },
        ChatPart::Document { media_type, base64 } => protocol::ContentBlock::Document {
            source: protocol::DocumentSource::Base64 {
                media_type,
                data: base64,
            },
        },
    }
}

/// Project an assistant reply onto its text, matching the repo's existing
/// fold (`orchestrator::hook_prompt_runner::extract_text`): `Text` AND
/// `ConnectorText` contribute, tool-use / reasoning blocks do not, and the
/// pieces are joined bare. Dropping `ConnectorText` would hand the page
/// `ok: true` with an empty answer whenever a reply arrives on that block.
fn extract_chat_text(content: &[ContentBlock]) -> String {
    content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text, .. } => Some(text.as_str()),
            ContentBlock::ConnectorText { connector_text, .. } => Some(connector_text.as_str()),
            _ => None,
        })
        .collect()
}

/// Real [`LocalAppsModel`] over the shared `ApiService`.
pub struct ApiServiceModel {
    service: Arc<ApiService>,
    /// `(model, profile)` behind ONE `RwLock`, NOT two independent locks:
    /// `ClientCommand::SetModel` updates this in place (via
    /// [`LocalAppsModel::set_model`]) so app-initiated calls follow a live
    /// `/model` switch instead of staying pinned to whatever
    /// `default_model_id`/`default_model_profile` were at engine build time
    /// — the same class of "silently stale after the user changed
    /// something" bug `SharedLlm` closes for a reconnect. Two separate locks
    /// (an earlier version of this code) let a read land BETWEEN `set_model`'s
    /// two writes and see a new model id paired with the OLD provider
    /// profile, routing the new model through the wrong provider — a single
    /// lock over the pair makes that torn read structurally impossible.
    selection: RwLock<(String, Option<String>)>,
}

impl ApiServiceModel {
    #[must_use]
    pub fn new(
        service: Arc<ApiService>,
        model: impl Into<String>,
        profile: Option<String>,
    ) -> Self {
        Self {
            service,
            selection: RwLock::new((model.into(), profile)),
        }
    }
}

#[async_trait]
impl LocalAppsModel for ApiServiceModel {
    /// An app-initiated free-text call.
    ///
    /// Non-streaming on purpose: `messages_create_side_query` is the only
    /// free-text entry point on `ApiService` that accepts `max_tokens` and
    /// `temperature`, and an app-initiated call spending the user's quota
    /// must carry an explicit budget. There is also nothing to stream INTO —
    /// the bridge is strictly request/response, so a token stream would have
    /// no channel to the page (see the module docs on the push channel that
    /// would be needed first).
    async fn chat(&self, request: ChatRequest) -> Result<ChatOutcome, AppError> {
        // One acquisition for the pair, never held across the await — a
        // concurrent `set_model` must never block, or be blocked by, an
        // in-flight call, but a reader must also never see a model id paired
        // with a profile from a DIFFERENT `set_model` call.
        let (model, profile) = self
            .selection
            .read()
            .expect("selection lock poisoned")
            .clone();
        let messages = request
            .messages
            .into_iter()
            .map(|message| {
                // `protocol::ContentBlock`, NOT the `llm_client` one this
                // module otherwise names: `ConversationMessage` is the
                // conversation vocabulary, and the two types are distinct.
                let content: Vec<protocol::ContentBlock> =
                    message.content.into_iter().map(chat_part_block).collect();
                match message.role {
                    ChatRole::User => ConversationMessage::User {
                        id: MessageId::new(),
                        content,
                        is_meta: false,
                        is_compact_summary: false,
                        is_visible_in_transcript_only: false,
                    },
                    ChatRole::Assistant => ConversationMessage::Assistant {
                        id: MessageId::new(),
                        content,
                        stop_reason: None,
                    },
                }
            })
            .collect();
        let response = self
            .service
            .messages_create_side_query(
                &model,
                profile.as_deref(),
                request.system.as_deref(),
                messages,
                vec![],
                Some(request.max_tokens),
                None,
                vec![],
                request.temperature,
            )
            .await
            .map_err(|error| AppError::LlmUnavailable(format!("{error}")))?;
        Ok(ChatOutcome {
            text: extract_chat_text(&response.content),
            stop_reason: response.stop_reason,
        })
    }

    fn set_model(&self, model: String, profile: Option<String>) {
        *self.selection.write().expect("selection lock poisoned") = (model, profile);
    }
}

/// The engine-side handle over the injected model — what the broker's
/// `llm.chat` bridge operation calls through (`SharedLlm` holds one).
pub struct LocalAppsLlm {
    model: Arc<dyn LocalAppsModel>,
}

impl LocalAppsLlm {
    #[must_use]
    pub fn new(model: Arc<dyn LocalAppsModel>) -> Self {
        Self { model }
    }

    /// Follow a live `/model` switch: see [`LocalAppsModel::set_model`].
    pub fn set_model(&self, model: String, profile: Option<String>) {
        self.model.set_model(model, profile);
    }

    /// One free-text call on behalf of a running app — see [`ChatRequest`].
    ///
    /// A passthrough: there is no prompt to compose and no validator to run,
    /// because the answer is prose the app renders itself. Everything
    /// policy-shaped (declared capability, budget clamp, concurrency,
    /// truncation) lives at the bridge, where the app id is known.
    pub async fn chat(&self, request: ChatRequest) -> Result<ChatOutcome, AppError> {
        self.model.chat(request).await
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::{AppError, ChatOutcome, ChatRequest, LocalAppsModel};
    use async_trait::async_trait;
    use std::sync::{Arc, Mutex};

    /// 按顺序吐出预置 chat 响应的假模型。
    pub(crate) struct ScriptedModel {
        chat_responses: Mutex<Vec<Result<ChatOutcome, AppError>>>,
        chat_requests: Mutex<Vec<ChatRequest>>,
    }

    impl ScriptedModel {
        /// A model with NO scripted answers — any call fails loudly with
        /// "ran out of responses" rather than silently returning something
        /// plausible. The no-op double for wiring tests.
        pub(crate) fn new() -> Arc<Self> {
            Self::with_chat(Vec::new())
        }

        /// The double scripted for [`LocalAppsModel::chat`].
        pub(crate) fn with_chat(responses: Vec<Result<ChatOutcome, AppError>>) -> Arc<Self> {
            Arc::new(Self {
                chat_responses: Mutex::new(responses),
                chat_requests: Mutex::new(Vec::new()),
            })
        }

        /// The `index`-th `chat` call's request (0-based, call order).
        pub(crate) fn chat_request_at(&self, index: usize) -> ChatRequest {
            self.chat_requests.lock().expect("lock")[index].clone()
        }

        /// How many times `chat` has been called so far.
        #[allow(dead_code)]
        pub(crate) fn chat_call_count(&self) -> usize {
            self.chat_requests.lock().expect("lock").len()
        }
    }

    #[async_trait]
    impl LocalAppsModel for ScriptedModel {
        async fn chat(&self, request: ChatRequest) -> Result<ChatOutcome, AppError> {
            self.chat_requests.lock().expect("lock").push(request);
            let mut responses = self.chat_responses.lock().expect("lock");
            if responses.is_empty() {
                return Err(AppError::Io(
                    "the scripted model ran out of chat responses".into(),
                ));
            }
            responses.remove(0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::ScriptedModel;
    use super::*;

    /// The free-text seam's answer is whatever prose the model wrote.
    /// Pinning the extraction against hand-built blocks keeps a reasoning
    /// model's thinking out of the app's answer with no `ApiService`
    /// involved.
    #[test]
    fn chat_text_concatenates_text_blocks_and_drops_thinking() {
        let content = vec![
            ContentBlock::Reasoning {
                text: "先想一下".into(),
                signature: None,
            },
            ContentBlock::Text {
                text: "答案第一段".into(),
                cache_control: None,
            },
            ContentBlock::Text {
                text: "，第二段".into(),
                cache_control: None,
            },
        ];
        assert_eq!(extract_chat_text(&content), "答案第一段，第二段");
    }

    #[tokio::test]
    async fn chat_hands_the_whole_request_to_the_model_and_returns_its_answer() {
        let model = ScriptedModel::with_chat(vec![Ok(ChatOutcome {
            text: "好的".into(),
            stop_reason: Some("end_turn".into()),
        })]);
        let llm = LocalAppsLlm::new(model.clone());

        let outcome = llm
            .chat(ChatRequest {
                system: Some("你是这个应用的助手".into()),
                messages: vec![
                    ChatMessage {
                        role: ChatRole::User,
                        content: vec![ChatPart::Text("第一句".into())],
                    },
                    ChatMessage {
                        role: ChatRole::Assistant,
                        content: vec![ChatPart::Text("上一轮回答".into())],
                    },
                    ChatMessage {
                        role: ChatRole::User,
                        content: vec![ChatPart::Text("第二句".into())],
                    },
                ],
                max_tokens: 512,
                temperature: Some(0.3),
            })
            .await
            .expect("chat");

        assert_eq!(outcome.text, "好的");
        assert_eq!(outcome.stop_reason.as_deref(), Some("end_turn"));
        let seen = model.chat_request_at(0);
        assert_eq!(seen.max_tokens, 512);
        assert_eq!(seen.temperature, Some(0.3));
        assert_eq!(seen.system.as_deref(), Some("你是这个应用的助手"));
        assert_eq!(
            seen.messages.len(),
            3,
            "multi-turn context must reach the model verbatim, not be flattened"
        );
    }

    /// A photo the app just captured has to reach the provider as a real
    /// vision block — not as a base64 string glued into the prompt text,
    /// which is what an app would be forced into if this mapping were
    /// missing (and which no provider can actually look at).
    #[test]
    fn an_image_part_lowers_to_a_provider_vision_block() {
        let block = chat_part_block(ChatPart::Image {
            media_type: "image/jpeg".into(),
            base64: "AQID".into(),
        });
        assert!(matches!(
            block,
            protocol::ContentBlock::Image {
                source: protocol::ImageSource::Base64 { media_type, data }
            } if media_type == "image/jpeg" && data == "AQID"
        ));

        let block = chat_part_block(ChatPart::Document {
            media_type: "application/pdf".into(),
            base64: "JVBER".into(),
        });
        assert!(matches!(
            block,
            protocol::ContentBlock::Document {
                source: protocol::DocumentSource::Base64 { media_type, .. }
            } if media_type == "application/pdf"
        ));
    }

    #[tokio::test]
    async fn chat_surfaces_an_unreachable_model_as_llm_unavailable() {
        let model = ScriptedModel::with_chat(vec![Err(AppError::LlmUnavailable("offline".into()))]);
        let llm = LocalAppsLlm::new(model);
        let error = llm
            .chat(ChatRequest {
                system: None,
                messages: vec![ChatMessage {
                    role: ChatRole::User,
                    content: vec![ChatPart::Text("你好".into())],
                }],
                max_tokens: 128,
                temperature: None,
            })
            .await
            .expect_err("an unreachable model must not read as a refusal");
        assert!(matches!(error, AppError::LlmUnavailable(_)), "{error:?}");
    }

    /// A [`LocalAppsModel`] double that only records `set_model` calls —
    /// proves [`LocalAppsLlm::set_model`] actually delegates to the
    /// underlying model instead of silently no-op'ing (the DEFAULT trait
    /// method every OTHER test double relies on).
    struct RecordingModel {
        calls: std::sync::Mutex<Vec<(String, Option<String>)>>,
    }

    #[async_trait]
    impl LocalAppsModel for RecordingModel {
        async fn chat(&self, _request: ChatRequest) -> Result<ChatOutcome, AppError> {
            unreachable!("not exercised by this test")
        }

        fn set_model(&self, model: String, profile: Option<String>) {
            self.calls.lock().expect("lock").push((model, profile));
        }
    }

    /// PINS the Important-3 fix from the Task 11 review: `ClientCommand::
    /// SetModel` must reach the local-apps LLM, not just the orchestrator's
    /// own model selection — otherwise app-initiated `llm.chat` stays
    /// silently pinned to whatever was live at engine build time forever.
    #[test]
    fn set_model_delegates_to_the_underlying_model() {
        let model = Arc::new(RecordingModel {
            calls: std::sync::Mutex::new(Vec::new()),
        });
        let llm = LocalAppsLlm::new(model.clone());
        llm.set_model("claude-opus-5".into(), Some("anthropic".into()));
        llm.set_model("gpt-5.5".into(), None);
        assert_eq!(
            model.calls.lock().expect("lock").as_slice(),
            &[
                ("claude-opus-5".to_string(), Some("anthropic".to_string())),
                ("gpt-5.5".to_string(), None),
            ]
        );
    }
}
