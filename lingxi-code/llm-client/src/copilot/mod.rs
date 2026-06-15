//! GitHub Copilot provider support: device-flow login + request authenticator.

pub mod auth;
pub mod login;

pub use auth::{CopilotAuthenticator, CopilotSecret, COPILOT_API_VERSION, COPILOT_USER_AGENT};
pub use login::{CopilotHttp, CopilotLogin, DeviceCodeResponse, PollOutcome, COPILOT_CLIENT_ID};
