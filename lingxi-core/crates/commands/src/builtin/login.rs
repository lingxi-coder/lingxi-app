//! `/login` — interactive OAuth (PKCE) login flow.
//!
//! Locked display templates (`LingXi` UX, M5-11 T0 step 2 L9):
//!   - Success: `"Logged in as {email} (org: {org_id})."`
//!   - Failure prefix: `"Could not log in: "`

use crate::builtin::names::core_description;
use crate::model::{BuiltinCommandHandler, CommandResult};
use crate::parser::ParsedSlashCommand;
use async_trait::async_trait;
use lingxi_telemetry::tengu::command as cmd_evt;
use lingxi_traits::AuthHandle;
use std::sync::Arc;

/// `/login` handler — drives the [`AuthHandle::login`] flow.
#[derive(Clone)]
pub struct LoginHandler {
    auth: Arc<dyn AuthHandle>,
}

impl LoginHandler {
    /// Construct a `LoginHandler` bound to the given auth handle.
    #[must_use]
    pub fn new(auth: Arc<dyn AuthHandle>) -> Self {
        Self { auth }
    }
}

#[async_trait]
impl BuiltinCommandHandler for LoginHandler {
    async fn handle(&self, _args: &ParsedSlashCommand) -> CommandResult {
        lingxi_telemetry::emit_command_started(cmd_evt::LOGIN_STARTED);
        match self.auth.login().await {
            Ok(info) => {
                lingxi_telemetry::emit_command_completed(cmd_evt::LOGIN_COMPLETED, &info.org_id);
                CommandResult::Done {
                    display: Some(format!(
                        "Logged in as {} (org: {}).",
                        info.email, info.org_id
                    )),
                }
            }
            Err(e) => {
                let msg = e.to_string();
                lingxi_telemetry::emit_command_failed(cmd_evt::LOGIN_FAILED, &msg);
                CommandResult::Done {
                    display: Some(format!("Could not log in: {msg}")),
                }
            }
        }
    }
    fn name(&self) -> &str {
        "login"
    }
    fn description(&self) -> &str {
        core_description("login")
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use lingxi_traits::{AuthError, LoginInfo};
    use std::sync::Mutex as StdMutex;

    /// Test double for [`AuthHandle`]. Exposes the same `Ok` / `Err`
    /// constructors as the test helpers in M5-11 T9.
    pub struct MockAuth {
        result: StdMutex<Result<LoginInfo, AuthError>>,
    }
    impl MockAuth {
        pub fn ok(info: LoginInfo) -> Self {
            Self {
                result: StdMutex::new(Ok(info)),
            }
        }
        pub fn err(e: AuthError) -> Self {
            Self {
                result: StdMutex::new(Err(e)),
            }
        }
    }
    #[async_trait]
    impl AuthHandle for MockAuth {
        async fn login(&self) -> Result<LoginInfo, AuthError> {
            self.result.lock().unwrap().clone()
        }
        async fn logout(&self) -> Result<(), AuthError> {
            Ok(())
        }
        async fn current_user(&self) -> Option<LoginInfo> {
            self.result.lock().unwrap().as_ref().ok().cloned()
        }
    }

    fn args() -> ParsedSlashCommand {
        ParsedSlashCommand {
            name: "login".into(),
            raw_args: String::new(),
            positional_args: vec![],
        }
    }

    #[tokio::test]
    async fn success() {
        let m = Arc::new(MockAuth::ok(LoginInfo {
            email: "u@x.com".into(),
            org_id: "org_42".into(),
        }));
        let h = LoginHandler::new(m);
        if let CommandResult::Done { display: Some(s) } = h.handle(&args()).await {
            assert_eq!(s, "Logged in as u@x.com (org: org_42).");
        } else {
            panic!();
        }
    }

    #[tokio::test]
    async fn timeout_error() {
        let m = Arc::new(MockAuth::err(AuthError::Timeout));
        let h = LoginHandler::new(m);
        if let CommandResult::Done { display: Some(s) } = h.handle(&args()).await {
            assert_eq!(s, "Could not log in: login flow timed out");
        } else {
            panic!();
        }
    }

    #[tokio::test]
    async fn network_error_propagates() {
        let m = Arc::new(MockAuth::err(AuthError::Network("dns failure".into())));
        let h = LoginHandler::new(m);
        if let CommandResult::Done { display: Some(s) } = h.handle(&args()).await {
            assert_eq!(s, "Could not log in: network error: dns failure");
        } else {
            panic!();
        }
    }
}
