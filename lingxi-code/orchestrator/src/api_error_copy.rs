//! User-facing API-error copy — the `${IT}: …` family from claude-code's error
//! renderer (2.1.220 @230600350–230602900).
//!
//! Every string here was verified fragment by fragment against the binary, with
//! a control string that must NOT match: a pattern grep over a binary carrying
//! both UTF-8 and UTF-16LE text reports false positives without one.
//!
//! The separator between clauses is U+00B7 `·` (the binary's `\xB7`), not a
//! hyphen and not a bullet.
//!
//! # Scope
//!
//! This module covers the 429 family. The rest of the error-surfacing set
//! (`Credit balance is too low`, `Invalid API key · Please run /login`, the
//! PDF-page and password branches) is still unported and is NOT silently
//! absorbed here.

use serde_json::Value;

/// Oracle `IT` — the prefix every rendered API error carries.
pub(crate) const API_ERROR: &str = "API Error";

/// Oracle `\xB7` — the clause separator. A hyphen here would be wrong.
const SEP: char = '·';

/// Oracle `Gcs`: does this message mean "the 1M-context window needs paid usage
/// credits"? Two accepted phrasings, matched verbatim.
#[must_use]
pub(crate) fn is_long_context_credit_message(message: &str) -> bool {
    message.contains("Extra usage is required for long context")
        || message.contains("Usage credits are required for long context")
}

/// Oracle: `${IT}: Usage credits required for 1M context · ${hint}`.
///
/// `non_interactive` picks the hint (`_n()`): an interactive session is told to
/// run the slash commands, a non-interactive one is pointed at the settings URL
/// and `--model`, because it has no slash commands to run.
#[must_use]
pub(crate) fn usage_credits_required_for_1m_context(non_interactive: bool) -> String {
    let hint = if non_interactive {
        format!("turn on usage credits at {USAGE_SETTINGS_URL}, or use --model to switch to standard context")
    } else {
        "run /usage-credits to turn them on, or /model to switch to standard context".to_string()
    };
    format!("{API_ERROR}: Usage credits required for 1M context {SEP} {hint}")
}

/// Oracle `xYr`.
const USAGE_SETTINGS_URL: &str = "claude.ai/settings/usage?from=cc_cli_limit_message";

/// Oracle `lir` — no usable credential, so the user has to sign in.
pub(crate) const NOT_LOGGED_IN: &str = "Not logged in \u{b7} Please run /login";

/// Oracle `cir` — a credential EXISTS but the server rejected it. "External"
/// because it came from outside the app: an env var or an `apiKeyHelper`
/// script, neither of which `/login` can fix.
pub(crate) const INVALID_API_KEY: &str = "Invalid API key \u{b7} Fix external API key";

/// Oracle `UOu` — an auth failure the client believes is transient.
pub(crate) const AUTH_TRANSIENT: &str =
    "Authentication error \u{b7} This may be a temporary network issue, please try again";

/// Is this a REMOTE session? — oracle `qOu()` = `Yt(process.env.CLAUDE_CODE_REMOTE)`.
///
/// Rebranded to `LINGXI_REMOTE`, matching the existing
/// `tool_ui::push_notification::is_remote` which ports the same predicate. This
/// is LingXi's OWN runtime flag, unlike the `CLAUDE_CODE_USE_*` provider
/// variables, which keep their names.
#[must_use]
pub(crate) fn is_remote_session() -> bool {
    traits::env::is_env_truthy(std::env::var("LINGXI_REMOTE").ok().as_deref())
}

/// Does this error message name the `x-api-key` header?
///
/// The oracle's gate for the whole credential-rejection branch,
/// `message.toLowerCase().includes("x-api-key")` — it keys on the header the
/// server complained about, not on the status.
#[must_use]
pub(crate) fn mentions_api_key_header(message: &str) -> bool {
    message.to_ascii_lowercase().contains("x-api-key")
}

/// Which credential-rejection copy to show — oracle:
///
/// ```js
/// let {source:i} = e1();
/// return i==="ANTHROPIC_API_KEY" || i==="apiKeyHelper" ? cir : lir;
/// ```
///
/// `external` means the credential came from outside the app (the env var or a
/// helper script). Everything else — stored keys, OAuth, nothing at all — gets
/// the sign-in copy, because `/login` is the fix.
#[must_use]
pub(crate) fn credential_rejected_text(external: bool) -> &'static str {
    if external {
        INVALID_API_KEY
    } else {
        NOT_LOGGED_IN
    }
}

/// Oracle `GOu` / `VOu` / `zOu` / `KOu` — cloud-provider credential failures.
const AWS_CREDS_EXPIRED: &str = "AWS credentials expired or invalid";
const AWS_AUTH_FAILED: &str = "AWS authentication failed";
const GCP_CREDS_EXPIRED: &str = "Google Cloud credentials expired or invalid";
const GCP_AUTH_FAILED: &str = "Google Cloud authentication failed";
/// Appended when the AWS failure is NOT a plain 401 — a live credential that
/// still cannot reach the model is a permissions problem, not an expiry one.
const AWS_PERMISSIONS_HINT: &str =
    " \u{b7} if credentials are current, check AWS permissions and model access";
/// Oracle default for the GCP re-auth command.
const GCLOUD_ADC_LOGIN: &str = "gcloud auth application-default login";

/// Credential copy for a cloud-hosted route — oracle's AWS branch @230606080
/// and Google Cloud branch @230606640.
///
/// ```js
/// let c = e.status===401 && (l==="anthropicAws"||l==="mantle"), u = c?GOu:VOu,
///     d = c ? "" : " \xB7 if credentials are current, check AWS permissions and model access";
/// // GCP: d = e.status===401, p = d?zOu:KOu, f = ` \xB7 run \`${u}\` and retry`
/// ```
///
/// The 401-vs-other split is only decidable because the status survives into the
/// message (`llm_client::api_error_status`); before that both collapsed to one
/// variant and this could not have been ported.
///
/// `None` for routes that are not cloud-hosted — the caller falls through to the
/// credential-rejection copy.
#[must_use]
pub(crate) fn cloud_credential_text(route: &ErrorRouteTag, status: Option<u16>) -> Option<String> {
    let is_401 = status == Some(401);
    match route {
        // `anthropicAws` and `mantle` take the expiry wording on a 401; Bedrock
        // reaches this branch too but never satisfies `l==="anthropicAws"`, so it
        // always gets the generic failure plus the permissions hint.
        ErrorRouteTag::AnthropicAws => Some(if is_401 {
            AWS_CREDS_EXPIRED.to_string()
        } else {
            format!("{AWS_AUTH_FAILED}{AWS_PERMISSIONS_HINT}")
        }),
        ErrorRouteTag::Other { display } if display == "Bedrock" || display == "Mantle" => {
            let expiry = is_401 && display == "Mantle";
            Some(if expiry {
                AWS_CREDS_EXPIRED.to_string()
            } else {
                format!("{AWS_AUTH_FAILED}{AWS_PERMISSIONS_HINT}")
            })
        }
        ErrorRouteTag::AnthropicGoogleCloud => {
            let head = if is_401 {
                GCP_CREDS_EXPIRED
            } else {
                GCP_AUTH_FAILED
            };
            Some(format!("{head} \u{b7} run `{GCLOUD_ADC_LOGIN}` and retry"))
        }
        _ => None,
    }
}

/// Oracle `LYr` — the billing surface for `LlmError::QuotaExceeded`.
///
/// Rendered BARE: `yu({content:LYr,error:"billing_error"})` carries no
/// `API Error:` prefix, unlike the 429 family. Getting that wrong would be
/// invisible in review and wrong on screen.
pub(crate) const CREDIT_BALANCE_TOO_LOW: &str = "Credit balance is too low";

/// Oracle `Jq` — the prompt-too-long surface for `LlmError::ContextOverflow`.
///
/// Also bare: `yu({content:Jq,error:"invalid_request"})`.
pub(crate) const PROMPT_TOO_LONG: &str = "Prompt is too long";

/// Oracle `le_` — the first-party variant of the rejection label, used instead
/// of `Request rejected (429)` when the limit is the server's rather than the
/// account's.
///
/// Not selected yet: choosing between this and [`REQUEST_REJECTED_429`] needs
/// the provider-route signal the oracle's `i` carries, which this layer does
/// not have. Kept (and tested) so the string is already byte-verified when that
/// plumbing lands — deleting and re-deriving it later is how transcription
/// errors get in.
pub(crate) const SERVER_LIMITING: &str = "Server is temporarily limiting requests (not your usage limit)";

/// Recover the detail clause from a 429 message — oracle:
///
/// ```js
/// let c = e.message.replace(/^429\s+/,""), u;
/// try { let m = Ut(c), g = m?.error?.message ?? m?.message; if (typeof g==="string") u = g } catch {}
/// let d = u || c;
/// ```
///
/// Strip the status prefix, try to read the body as JSON and take
/// `error.message` (falling back to a top-level `message`), and use the stripped
/// remainder verbatim when it is not JSON. This only works because the decoder
/// stringifies the whole body into the message — see
/// `llm_client::providers::api_error_message`.
#[must_use]
pub(crate) fn rate_limit_detail(message: &str) -> String {
    let stripped = strip_status_prefix(message, 429);
    let parsed = serde_json::from_str::<Value>(stripped)
        .ok()
        .and_then(|v| {
            v.get("error")
                .and_then(|e| e.get("message"))
                .or_else(|| v.get("message"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .filter(|s| !s.is_empty());
    parsed.unwrap_or_else(|| stripped.to_string())
}

/// `message.replace(/^{status}\s+/, "")` — strip the status and the whitespace
/// run after it. Returns the input unchanged when the prefix is absent.
fn strip_status_prefix(message: &str, status: u16) -> &str {
    let digits = status.to_string();
    let Some(rest) = message.strip_prefix(&digits) else {
        return message;
    };
    let trimmed = rest.trim_start_matches([' ', '\t', '\n', '\r']);
    // `\s+` requires at least one whitespace character; `"429x"` keeps its text.
    if trimmed.len() == rest.len() {
        return message;
    }
    trimmed
}

/// Oracle: `${IT}: ${label} · ${detail || fallback}`.
///
/// `label` is `Request rejected (429)` normally, or [`SERVER_LIMITING`] when the
/// throttle is the server's. `fallback` is the
/// `this may be a temporary capacity issue.{suffix}` clause, used only when the
/// detail is empty — the suffix is provider-dependent (oracle `hpo()`), so the
/// caller supplies it.
#[must_use]
pub(crate) fn rate_limited_text(message: &str, label: &str, fallback: &str) -> String {
    let detail = rate_limit_detail(message);
    let clause = if detail.is_empty() { fallback } else { &detail };
    format!("{API_ERROR}: {label} {SEP} {clause}")
}

/// The default rejection label — oracle's non-first-party branch.
pub(crate) const REQUEST_REJECTED_429: &str = "Request rejected (429)";

/// The fallback clause's stem. The oracle appends [`persistence_suffix`].
pub(crate) const TEMPORARY_CAPACITY: &str = "this may be a temporary capacity issue.";

/// Oracle `jcs`.
const STATUS_PAGE: &str = "https://status.claude.com";

/// Which upstream a request is routed to — the oracle's `xn()` tags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ErrorRoute<'a> {
    /// `"firstParty"`. `default_endpoint` is the oracle's `Yd()`: false means a
    /// custom `ANTHROPIC_BASE_URL`, i.e. an inference gateway.
    FirstParty {
        /// Whether the request goes to the official endpoint.
        default_endpoint: bool,
        /// The configured base URL, named when it is a gateway.
        base_url: &'a str,
    },
    /// `"anthropicAws"`.
    AnthropicAws,
    /// `"anthropicGoogleCloud"`.
    AnthropicGoogleCloud,
    /// Anything else; `display` is the oracle's `rK[e]` provider name.
    Other {
        /// Provider display name.
        display: &'a str,
    },
}

/// Owned twin of [`ErrorRoute`], for carrying a route on
/// [`crate::OrchestratorConfig`] (which cannot hold borrowed strings).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", tag = "route")]
pub enum ErrorRouteTag {
    /// `"firstParty"`.
    FirstParty {
        /// Whether the request goes to the official endpoint (oracle `Yd()`).
        default_endpoint: bool,
        /// Configured base URL, named when it is a gateway.
        base_url: String,
    },
    /// `"anthropicAws"`.
    AnthropicAws,
    /// `"anthropicGoogleCloud"`.
    AnthropicGoogleCloud,
    /// Anything else; the oracle's `rK[e]` display name.
    Other {
        /// Provider display name.
        display: String,
    },
}

impl ErrorRouteTag {
    /// Resolve the route from the environment — oracle `xn()` @227682549:
    ///
    /// ```js
    /// return Z.CLAUDE_CODE_USE_BEDROCK?"bedrock"
    ///      : Z.CLAUDE_CODE_USE_FOUNDRY?"foundry"
    ///      : Z.CLAUDE_CODE_USE_ANTHROPIC_AWS?"anthropicAws"
    ///      : Z.CLAUDE_CODE_USE_ANTHROPIC_GOOGLE_CLOUD?"anthropicGoogleCloud"
    ///      : Z.CLAUDE_CODE_USE_MANTLE?"mantle"
    ///      : Z.CLAUDE_CODE_USE_VERTEX?"vertex"
    ///      : "firstParty";
    /// ```
    ///
    /// ORDER MATTERS — it is a chain, not a set, so `USE_BEDROCK` wins over
    /// `USE_VERTEX` when both are set.
    ///
    /// These keep their `CLAUDE_CODE_` names: they are the PROVIDER's variables,
    /// which the rebrand deliberately preserves (the same call sites already
    /// read them in `apps/cli` and `migrations`). Only LingXi's own variables
    /// take the `LINGXI_` prefix.
    ///
    /// `hpo()` only distinguishes firstParty / anthropicAws / anthropicGoogleCloud
    /// from "everything else", so the remaining tags collapse into
    /// [`Self::Other`] carrying their display name.
    #[must_use]
    pub fn from_env() -> Self {
        fn on(var: &str) -> bool {
            std::env::var(var).is_ok_and(|v| !v.is_empty() && v != "0" && v != "false")
        }
        if on("CLAUDE_CODE_USE_BEDROCK") {
            return Self::Other {
                display: "Bedrock".to_string(),
            };
        }
        if on("CLAUDE_CODE_USE_FOUNDRY") {
            return Self::Other {
                display: "Foundry".to_string(),
            };
        }
        if on("CLAUDE_CODE_USE_ANTHROPIC_AWS") {
            return Self::AnthropicAws;
        }
        if on("CLAUDE_CODE_USE_ANTHROPIC_GOOGLE_CLOUD") {
            return Self::AnthropicGoogleCloud;
        }
        if on("CLAUDE_CODE_USE_MANTLE") {
            return Self::Other {
                display: "Mantle".to_string(),
            };
        }
        if on("CLAUDE_CODE_USE_VERTEX") {
            return Self::Other {
                display: "Vertex".to_string(),
            };
        }
        // firstParty. `Yd()` picks the status page over the gateway wording;
        // modelled as "no custom base URL is configured".
        let base_url = std::env::var("ANTHROPIC_BASE_URL").unwrap_or_default();
        Self::FirstParty {
            default_endpoint: base_url.is_empty(),
            base_url,
        }
    }

    /// Borrow this tag as an [`ErrorRoute`].
    #[must_use]
    pub(crate) fn as_route(&self) -> ErrorRoute<'_> {
        match self {
            Self::FirstParty {
                default_endpoint,
                base_url,
            } => ErrorRoute::FirstParty {
                default_endpoint: *default_endpoint,
                base_url,
            },
            Self::AnthropicAws => ErrorRoute::AnthropicAws,
            Self::AnthropicGoogleCloud => ErrorRoute::AnthropicGoogleCloud,
            Self::Other { display } => ErrorRoute::Other { display },
        }
    }
}

/// The full fallback clause: [`TEMPORARY_CAPACITY`] plus [`persistence_suffix`].
///
/// `None` renders the stem ALONE. That is a documented divergence, not parity:
/// the oracle's `xn()` always resolves to some route, but LingXi's error layer
/// only knows one when a composition root supplies it, and an unattributed
/// suffix ("check your  service status.") would be worse than none.
#[must_use]
pub(crate) fn capacity_fallback(route: Option<&ErrorRouteTag>) -> String {
    match route {
        Some(tag) => format!("{TEMPORARY_CAPACITY}{}", persistence_suffix(tag.as_route())),
        None => TEMPORARY_CAPACITY.to_string(),
    }
}

/// Oracle `hpo()` — the clause appended to
/// [`TEMPORARY_CAPACITY`], naming where to look if the trouble persists.
///
/// EVERY branch starts with a leading space; the caller concatenates without
/// one. Dropping it silently joins two words.
///
/// ```js
/// if (xn()==="firstParty") {
///   if (Yd()) return ` If it persists, check ${jcs}.`;
///   return ` If it persists, check your inference gateway (${URL.parse(t)?.host||t}).`
/// }
/// if (…==="anthropicAws") return ` If it persists, check ${jcs}.`;
/// if (…==="anthropicGoogleCloud") return ` If it persists, check ${jcs} and Google Cloud's status page.`;
/// return ` If it persists, check your ${rK[e]} service status.`;
/// ```
#[must_use]
pub(crate) fn persistence_suffix(route: ErrorRoute<'_>) -> String {
    match route {
        ErrorRoute::FirstParty {
            default_endpoint: true,
            ..
        }
        | ErrorRoute::AnthropicAws => format!(" If it persists, check {STATUS_PAGE}."),
        ErrorRoute::FirstParty { base_url, .. } => {
            // `URL.parse(t)?.host || t` — the HOST when it parses, else the raw
            // string. `host` keeps the port, unlike `hostname`.
            let host = url::Url::parse(base_url)
                .ok()
                .and_then(|u| {
                    u.host_str().map(|h| match u.port() {
                        Some(p) => format!("{h}:{p}"),
                        None => h.to_string(),
                    })
                })
                .unwrap_or_else(|| base_url.to_string());
            format!(" If it persists, check your inference gateway ({host}).")
        }
        ErrorRoute::AnthropicGoogleCloud => {
            format!(" If it persists, check {STATUS_PAGE} and Google Cloud's status page.")
        }
        ErrorRoute::Other { display } => {
            format!(" If it persists, check your {display} service status.")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Both of these render BARE — no `API Error:` prefix — which is the easy
    /// thing to get wrong when every neighbouring string has one.
    /// `xn()` is a CHAIN, not a set: with both Bedrock and Vertex set, Bedrock
    /// wins. A `match`-style port that checked them in any other order would
    /// name the wrong provider in the message.
    ///
    /// Env is process-global, so this runs the whole chain in ONE test rather
    /// than racing sibling tests that set the same variables.
    #[test]
    fn the_route_chain_resolves_in_the_oracle_order() {
        const VARS: [&str; 6] = [
            "CLAUDE_CODE_USE_BEDROCK",
            "CLAUDE_CODE_USE_FOUNDRY",
            "CLAUDE_CODE_USE_ANTHROPIC_AWS",
            "CLAUDE_CODE_USE_ANTHROPIC_GOOGLE_CLOUD",
            "CLAUDE_CODE_USE_MANTLE",
            "CLAUDE_CODE_USE_VERTEX",
        ];
        let saved: Vec<Option<String>> = VARS.iter().map(|v| std::env::var(v).ok()).collect();
        let saved_base = std::env::var("ANTHROPIC_BASE_URL").ok();
        for v in VARS {
            std::env::remove_var(v);
        }
        std::env::remove_var("ANTHROPIC_BASE_URL");

        // Nothing set → firstParty on the official endpoint.
        assert_eq!(
            ErrorRouteTag::from_env(),
            ErrorRouteTag::FirstParty {
                default_endpoint: true,
                base_url: String::new()
            }
        );

        // A custom base URL keeps the route but drops the default-endpoint flag.
        std::env::set_var("ANTHROPIC_BASE_URL", "https://gw.test:9000");
        assert_eq!(
            ErrorRouteTag::from_env(),
            ErrorRouteTag::FirstParty {
                default_endpoint: false,
                base_url: "https://gw.test:9000".to_string()
            }
        );
        std::env::remove_var("ANTHROPIC_BASE_URL");

        std::env::set_var("CLAUDE_CODE_USE_VERTEX", "1");
        assert!(matches!(ErrorRouteTag::from_env(), ErrorRouteTag::Other { .. }));
        // Bedrock outranks Vertex — the chain order, not alphabetical.
        std::env::set_var("CLAUDE_CODE_USE_BEDROCK", "1");
        assert_eq!(
            ErrorRouteTag::from_env(),
            ErrorRouteTag::Other {
                display: "Bedrock".to_string()
            }
        );
        std::env::remove_var("CLAUDE_CODE_USE_BEDROCK");

        // anthropicAws outranks Vertex too, and is its OWN branch in `hpo()`.
        std::env::set_var("CLAUDE_CODE_USE_ANTHROPIC_AWS", "1");
        assert_eq!(ErrorRouteTag::from_env(), ErrorRouteTag::AnthropicAws);
        std::env::remove_var("CLAUDE_CODE_USE_ANTHROPIC_AWS");

        std::env::set_var("CLAUDE_CODE_USE_ANTHROPIC_GOOGLE_CLOUD", "1");
        assert_eq!(ErrorRouteTag::from_env(), ErrorRouteTag::AnthropicGoogleCloud);

        for (v, old) in VARS.iter().zip(saved) {
            match old {
                Some(x) => std::env::set_var(v, x),
                None => std::env::remove_var(v),
            }
        }
        match saved_base {
            Some(x) => std::env::set_var("ANTHROPIC_BASE_URL", x),
            None => std::env::remove_var("ANTHROPIC_BASE_URL"),
        }
    }

    #[test]
    fn the_capacity_fallback_joins_stem_and_suffix_without_a_gap() {
        assert_eq!(
            capacity_fallback(Some(&ErrorRouteTag::AnthropicAws)),
            "this may be a temporary capacity issue. If it persists, check https://status.claude.com."
        );
        // Unknown route → stem alone, never a dangling "check your  service".
        assert_eq!(
            capacity_fallback(None),
            "this may be a temporary capacity issue."
        );
    }

    #[test]
    fn the_persistence_suffix_is_byte_exact_on_every_branch() {
        assert_eq!(
            persistence_suffix(ErrorRoute::FirstParty {
                default_endpoint: true,
                base_url: ""
            }),
            " If it persists, check https://status.claude.com."
        );
        assert_eq!(
            persistence_suffix(ErrorRoute::AnthropicAws),
            " If it persists, check https://status.claude.com."
        );
        assert_eq!(
            persistence_suffix(ErrorRoute::AnthropicGoogleCloud),
            " If it persists, check https://status.claude.com and Google Cloud's status page."
        );
        assert_eq!(
            persistence_suffix(ErrorRoute::Other { display: "Bedrock" }),
            " If it persists, check your Bedrock service status."
        );
        // A custom base URL is named by HOST, not the whole URL.
        assert_eq!(
            persistence_suffix(ErrorRoute::FirstParty {
                default_endpoint: false,
                base_url: "https://gw.example.com/v1/messages"
            }),
            " If it persists, check your inference gateway (gw.example.com)."
        );
        // `URL.host` keeps the port (unlike `hostname`).
        assert_eq!(
            persistence_suffix(ErrorRoute::FirstParty {
                default_endpoint: false,
                base_url: "https://gw.example.com:8443/v1"
            }),
            " If it persists, check your inference gateway (gw.example.com:8443)."
        );
        // Unparseable → the raw string, per `|| t`.
        assert_eq!(
            persistence_suffix(ErrorRoute::FirstParty {
                default_endpoint: false,
                base_url: "not a url"
            }),
            " If it persists, check your inference gateway (not a url)."
        );
    }

    /// Every branch begins with a space — the caller concatenates without one,
    /// so losing it joins "issue." to "If".
    #[test]
    fn every_persistence_suffix_branch_leads_with_a_space() {
        for r in [
            ErrorRoute::FirstParty { default_endpoint: true, base_url: "" },
            ErrorRoute::FirstParty { default_endpoint: false, base_url: "https://x.test" },
            ErrorRoute::AnthropicAws,
            ErrorRoute::AnthropicGoogleCloud,
            ErrorRoute::Other { display: "X" },
        ] {
            let s = persistence_suffix(r);
            assert!(s.starts_with(' '), "missing leading space: {s:?}");
        }
    }

    /// These are BARE too, and use U+00B7 like the 429 clause separator.
    ///
    /// NOTE the binary stores the JS escape `\xB7`, not the encoded character,
    /// so verifying these needs the escaped form — grepping for the decoded `·`
    /// returns 0 and looks like the string is absent.
    #[test]
    fn the_credential_copy_is_byte_exact() {
        assert_eq!(NOT_LOGGED_IN, "Not logged in \u{b7} Please run /login");
        assert_eq!(INVALID_API_KEY, "Invalid API key \u{b7} Fix external API key");
        assert_eq!(
            AUTH_TRANSIENT,
            "Authentication error \u{b7} This may be a temporary network issue, please try again"
        );
        for s in [NOT_LOGGED_IN, INVALID_API_KEY, AUTH_TRANSIENT] {
            assert!(!s.starts_with(API_ERROR), "{s} renders bare");
            assert!(s.contains('\u{b7}'), "{s} separates with U+00B7");
        }
    }

    /// An externally supplied credential was rejected → tell the user to fix
    /// THAT, because /login cannot. Anything else → /login.
    #[test]
    fn credential_copy_splits_on_where_the_key_came_from() {
        assert_eq!(credential_rejected_text(true), INVALID_API_KEY);
        assert_eq!(credential_rejected_text(false), NOT_LOGGED_IN);
    }

    /// On a remote session a rejected key is reported as possibly transient,
    /// BEFORE the source split — the remote host may just have lost the network.
    #[test]
    fn a_remote_session_reports_the_failure_as_transient() {
        let saved = std::env::var("LINGXI_REMOTE").ok();
        std::env::remove_var("LINGXI_REMOTE");
        assert!(!is_remote_session());
        std::env::set_var("LINGXI_REMOTE", "1");
        assert!(is_remote_session());
        std::env::set_var("LINGXI_REMOTE", "0");
        assert!(!is_remote_session(), "`0` is falsy, matching the oracle's `Yt`");
        match saved {
            Some(v) => std::env::set_var("LINGXI_REMOTE", v),
            None => std::env::remove_var("LINGXI_REMOTE"),
        }
    }

    #[test]
    fn the_api_key_header_gate_is_case_insensitive() {
        assert!(mentions_api_key_header("invalid X-Api-Key header"));
        assert!(mentions_api_key_header("401 {\"error\":\"bad x-api-key\"}"));
        assert!(!mentions_api_key_header("authentication_error"));
    }

    #[test]
    fn cloud_credential_copy_splits_on_401() {
        // anthropicAws: 401 means the credential expired; anything else means it
        // is live but cannot reach the model.
        assert_eq!(
            cloud_credential_text(&ErrorRouteTag::AnthropicAws, Some(401)).unwrap(),
            "AWS credentials expired or invalid"
        );
        assert_eq!(
            cloud_credential_text(&ErrorRouteTag::AnthropicAws, Some(403)).unwrap(),
            "AWS authentication failed \u{b7} if credentials are current, check AWS \
             permissions and model access"
        );
        // Bedrock reaches the same branch but never takes the expiry wording:
        // the oracle's inner test is `l==="anthropicAws"||l==="mantle"`.
        assert_eq!(
            cloud_credential_text(
                &ErrorRouteTag::Other { display: "Bedrock".into() },
                Some(401)
            )
            .unwrap(),
            "AWS authentication failed \u{b7} if credentials are current, check AWS \
             permissions and model access"
        );
        // GCP always names the re-auth command.
        assert_eq!(
            cloud_credential_text(&ErrorRouteTag::AnthropicGoogleCloud, Some(401)).unwrap(),
            "Google Cloud credentials expired or invalid \u{b7} run \
             `gcloud auth application-default login` and retry"
        );
        assert_eq!(
            cloud_credential_text(&ErrorRouteTag::AnthropicGoogleCloud, Some(500)).unwrap(),
            "Google Cloud authentication failed \u{b7} run \
             `gcloud auth application-default login` and retry"
        );
        // A first-party route is not cloud-hosted → the caller falls through.
        assert!(cloud_credential_text(
            &ErrorRouteTag::FirstParty { default_endpoint: true, base_url: String::new() },
            Some(401)
        )
        .is_none());
    }

    #[test]
    fn the_bare_surfaces_carry_no_prefix() {
        assert_eq!(CREDIT_BALANCE_TOO_LOW, "Credit balance is too low");
        assert_eq!(PROMPT_TOO_LONG, "Prompt is too long");
        for s in [CREDIT_BALANCE_TOO_LOW, PROMPT_TOO_LONG] {
            assert!(!s.starts_with(API_ERROR), "{s} must not be prefixed");
        }
    }

    #[test]
    fn the_rendered_429_is_byte_exact() {
        // The common shape: a decoded body, so the detail comes from JSON.
        let msg = r#"429 {"type":"error","error":{"type":"rate_limit_error","message":"slow down"}}"#;
        assert_eq!(
            rate_limited_text(msg, REQUEST_REJECTED_429, TEMPORARY_CAPACITY),
            "API Error: Request rejected (429) \u{b7} slow down"
        );
        // The separator is U+00B7, not a hyphen or an ASCII middot lookalike.
        assert!(rate_limited_text(msg, REQUEST_REJECTED_429, TEMPORARY_CAPACITY)
            .contains('\u{b7}'));
    }

    #[test]
    fn the_first_party_label_swaps_in() {
        assert_eq!(
            rate_limited_text("429 {\"message\":\"x\"}", SERVER_LIMITING, TEMPORARY_CAPACITY),
            "API Error: Server is temporarily limiting requests (not your usage limit) \u{b7} x"
        );
    }

    #[test]
    fn detail_falls_back_through_json_then_text_then_the_capacity_clause() {
        // `error.message` wins.
        assert_eq!(
            rate_limit_detail(r#"429 {"error":{"message":"a"},"message":"b"}"#),
            "a"
        );
        // Top-level `message` when there is no `error.message`.
        assert_eq!(rate_limit_detail(r#"429 {"message":"b"}"#), "b");
        // Not JSON → the stripped remainder verbatim.
        assert_eq!(rate_limit_detail("429 Too Many Requests"), "Too Many Requests");
        // Nothing after the status → empty, so the caller's fallback shows.
        assert_eq!(
            rate_limited_text("429 ", REQUEST_REJECTED_429, TEMPORARY_CAPACITY),
            "API Error: Request rejected (429) \u{b7} this may be a temporary capacity issue."
        );
    }

    #[test]
    fn the_status_prefix_strip_requires_whitespace() {
        // `/^429\s+/` — no whitespace, no strip.
        assert_eq!(rate_limit_detail("429Too Many"), "429Too Many");
        // Absent prefix is left alone.
        assert_eq!(rate_limit_detail("Too Many"), "Too Many");
        // Multiple spaces are all consumed (`\s+`).
        assert_eq!(rate_limit_detail("429   spaced"), "spaced");
    }

    #[test]
    fn the_1m_context_copy_is_byte_exact_in_both_modes() {
        assert_eq!(
            usage_credits_required_for_1m_context(false),
            "API Error: Usage credits required for 1M context \u{b7} run /usage-credits to \
             turn them on, or /model to switch to standard context"
        );
        assert_eq!(
            usage_credits_required_for_1m_context(true),
            "API Error: Usage credits required for 1M context \u{b7} turn on usage credits at \
             claude.ai/settings/usage?from=cc_cli_limit_message, or use --model to switch to \
             standard context"
        );
    }

    #[test]
    fn the_long_context_credit_gate_matches_both_phrasings() {
        assert!(is_long_context_credit_message(
            "429 Extra usage is required for long context requests"
        ));
        assert!(is_long_context_credit_message(
            "Usage credits are required for long context"
        ));
        assert!(!is_long_context_credit_message("rate limited"));
    }
}
