//! Side-effect-free product URL validation and construction.
use std::collections::BTreeMap;
use url::Url;
#[derive(Debug, Clone, PartialEq)]
pub struct Target {
    pub route: String,
    pub params: BTreeMap<String, String>,
    pub payload: Option<serde_json::Value>,
}
fn valid_route(route: &str) -> bool {
    let s: Vec<_> = route.split('/').collect();
    if s.iter().any(|s| {
        s.is_empty()
            || *s == "."
            || *s == ".."
            || s.contains('\\')
            || s.chars().any(char::is_control)
    }) {
        return false;
    }
    matches!(
        s.as_slice(),
        ["new"]
            | [
                "session" | "project" | "settings" | "provider" | "terminal" | "apps" | "market",
                _
            ]
            | ["market", "bot", _]
            | ["extensions", "mcp" | "skill" | "plugin", _]
            | ["cron", "job" | "run", _]
            | [
                "runtime",
                "agent" | "task" | "plan" | "review" | "activity",
                _
            ]
            | ["import", "mcp" | "plugin" | "skill" | "provider"]
            | ["remote", _, _]
            | ["team", "join" | "inbox"]
            | ["friend", "add" | "requests" | "chat"]
    )
}
pub fn parse(raw: &str) -> Result<Target, &'static str> {
    if raw.len() > 32768 || raw.chars().any(|c| c.is_control() || c == ' ') {
        return Err("Invalid link");
    }
    let bytes = raw.as_bytes();
    for (index, byte) in bytes.iter().enumerate() {
        if *byte == b'%'
            && (index + 2 >= bytes.len()
                || !bytes[index + 1].is_ascii_hexdigit()
                || !bytes[index + 2].is_ascii_hexdigit())
        {
            return Err("Invalid encoding");
        }
    }
    // URL parsers normalize dot segments, so check before that normalization.
    for segment in raw.split(['?', '#']).next().unwrap_or("").split('/') {
        let decoded = percent_encoding::percent_decode_str(segment)
            .decode_utf8()
            .map_err(|_| "Invalid encoding")?;
        if decoded == "." || decoded == ".." {
            return Err("Invalid route");
        }
    }
    let u = Url::parse(raw).map_err(|_| "Invalid link")?;
    if !u.username().is_empty() || u.password().is_some() || u.port().is_some() {
        return Err("Invalid authority");
    }
    let route = match u.scheme() {
        "lingxi" => format!("{}{}", u.host_str().ok_or("Missing destination")?, u.path()),
        "https" if u.host_str() == Some("lingxios.net") => u
            .path()
            .strip_prefix("/open/")
            .ok_or("Invalid path")?
            .into(),
        _ => return Err("Unsupported origin"),
    };
    let route = route
        .split('/')
        .map(|part| {
            let decoded = percent_encoding::percent_decode_str(part)
                .decode_utf8()
                .map_err(|_| "Invalid encoding")?;
            if decoded.contains('/') {
                return Err("Invalid segment");
            }
            Ok(decoded.into_owned())
        })
        .collect::<Result<Vec<_>, _>>()?
        .join("/");
    if !valid_route(&route) {
        return Err("Unsupported destination");
    }
    let mut params = BTreeMap::new();
    for (key, value) in u.query_pairs() {
        if ![
            "workspace",
            "message",
            "mode",
            "scope",
            "code",
            "versionId",
            "intentId",
            "uid",
        ]
        .contains(&key.as_ref())
            || value.is_empty()
            || value.len() > 512
            || value.chars().any(char::is_control)
            || params
                .insert(key.into_owned(), value.into_owned())
                .is_some()
        {
            return Err("Invalid parameter");
        }
    }
    if params
        .get("mode")
        .is_some_and(|m| m != "chat" && m != "code")
    {
        return Err("Invalid mode");
    }
    if route.starts_with("session/") && !params.contains_key("mode") {
        return Err("Session mode is required");
    }
    if (route == "team/inbox" || route == "friend/requests") && !params.is_empty() {
        return Err("Unexpected inbox parameter");
    }
    // A team invitation and a friend link each carry exactly one 32-character code, which no other route accepts.
    let url_safe = |value: &str| {
        value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    };
    if route == "team/join" || route == "friend/add" {
        let valid_code = params
            .get("code")
            .is_some_and(|code| code.len() == 32 && url_safe(code));
        if params.len() != 1 || !valid_code {
            return Err("Invalid invitation code");
        }
    } else if params.contains_key("code") {
        return Err("Invalid invitation code");
    }
    if route == "friend/chat" {
        let valid_uid = params
            .get("uid")
            .is_some_and(|uid| (1..=128).contains(&uid.len()) && url_safe(uid));
        if params.len() != 1 || !valid_uid {
            return Err("Invalid friend identity");
        }
    } else if params.contains_key("uid") {
        return Err("Invalid friend identity");
    }
    if let Some(id) = params.get_mut("message") {
        if !route.starts_with("session/")
            || id.len() != 36
            || !id.bytes().enumerate().all(|(i, b)| {
                if [8, 13, 18, 23].contains(&i) {
                    b == b'-'
                } else {
                    b.is_ascii_hexdigit()
                }
            })
        {
            return Err("Invalid message identity");
        }
        id.make_ascii_lowercase();
    }
    let valid_id = |id: &str| {
        !id.is_empty()
            && id.len() <= 128
            && id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    };
    if params
        .get("intentId")
        .is_some_and(|id| !route.starts_with("market/bot/") || !valid_id(id))
    {
        return Err("Invalid Bot handoff identity");
    }
    if params
        .get("versionId")
        .is_some_and(|id| !route.starts_with("market/bot/") || !valid_id(id))
    {
        return Err("Invalid Bot release identity");
    }
    if route.starts_with("market/bot/")
        && (!valid_id(&route[11..]) || params.keys().any(|k| k != "versionId" && k != "intentId"))
    {
        return Err("Invalid Bot market link");
    }
    let payload = if let Some(fragment) = u.fragment() {
        if route != "new" && !route.starts_with("import/") && !route.starts_with("terminal/") {
            return Err("Unexpected payload");
        }
        let fields: Vec<_> = url::form_urlencoded::parse(fragment.as_bytes()).collect();
        if fields.len() != 1 || fields[0].0 != "payload" {
            return Err("Invalid payload");
        }
        let value: serde_json::Value =
            serde_json::from_str(&fields[0].1).map_err(|_| "Invalid payload")?;
        if !value.is_object() {
            return Err("Invalid payload");
        }
        if route.starts_with("import/") {
            validate_import(&value, 0)?;
        } else {
            let expected = if route == "new" { "prompt" } else { "command" };
            if value
                .as_object()
                .unwrap()
                .iter()
                .any(|(k, v)| k != expected || !v.is_string())
            {
                return Err("Invalid draft");
            }
        }
        Some(value)
    } else {
        None
    };
    Ok(Target {
        route,
        params,
        payload,
    })
}
fn validate_import(value: &serde_json::Value, depth: usize) -> Result<(), &'static str> {
    if depth > 16 {
        return Err("Payload too deeply nested");
    }
    match value {
        serde_json::Value::Object(fields) => {
            for (key, value) in fields {
                let key = key.to_ascii_lowercase().replace(['-', '_'], "");
                if [
                    "apikey",
                    "token",
                    "accesstoken",
                    "refreshtoken",
                    "authorization",
                    "password",
                    "secret",
                    "headers",
                    "env",
                ]
                .contains(&key.as_str())
                {
                    return Err("Enter credentials in the application");
                }
                validate_import(value, depth + 1)?;
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                validate_import(value, depth + 1)?;
            }
        }
        serde_json::Value::String(text)
            if text.to_ascii_lowercase().starts_with("https://")
                || text.to_ascii_lowercase().starts_with("http://") =>
        {
            let url = Url::parse(text).map_err(|_| "Invalid import URL")?;
            if !url.username().is_empty() || url.password().is_some() || url.query().is_some() {
                return Err("Credential-bearing import URL");
            }
        }
        _ => (),
    }
    Ok(())
}
pub fn build(target: &Target, scheme: bool) -> Result<String, &'static str> {
    if !valid_route(&target.route) {
        return Err("Unsupported destination");
    }
    let encoded = target
        .route
        .split('/')
        .map(|s| {
            percent_encoding::utf8_percent_encode(s, percent_encoding::NON_ALPHANUMERIC).to_string()
        })
        .collect::<Vec<_>>()
        .join("/");
    let mut u = Url::parse(&format!(
        "{}{}",
        if scheme {
            "lingxi://"
        } else {
            "https://lingxios.net/open/"
        },
        encoded
    ))
    .map_err(|_| "Invalid route")?;
    if !target.params.is_empty() {
        let mut params = target.params.clone();
        if let Some(id) = params.get_mut("message") {
            id.make_ascii_lowercase();
        }
        u.query_pairs_mut().extend_pairs(&params);
    }
    if let Some(payload) = &target.payload {
        u.set_fragment(Some(
            &url::form_urlencoded::Serializer::new(String::new())
                .append_pair("payload", &payload.to_string())
                .finish(),
        ));
    }
    let raw = u.to_string();
    parse(&raw)?;
    Ok(raw)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn roundtrip() {
        for raw in [
            "lingxi://session/abc?workspace=project.a&mode=code",
            "https://lingxios.net/open/settings/general",
        ] {
            let target = parse(raw).unwrap();
            assert_eq!(parse(&build(&target, true).unwrap()).unwrap(), target);
            assert_eq!(parse(&build(&target, false).unwrap()).unwrap(), target);
        }
    }
    #[test]
    fn shared_corpus() {
        let corpus: serde_json::Value =
            serde_json::from_str(include_str!("../../deep-links/fixtures.json")).unwrap();
        for item in corpus["valid"].as_array().unwrap() {
            let parsed = parse(item["url"].as_str().unwrap()).unwrap();
            assert_eq!(parsed.route, item["route"].as_str().unwrap());
            for scheme in [false, true] {
                assert_eq!(parse(&build(&parsed, scheme).unwrap()).unwrap(), parsed);
            }
            if let Some(params) = item.get("params") {
                assert_eq!(serde_json::to_value(&parsed.params).unwrap(), *params);
            }
        }
        for raw in corpus["invalid"].as_array().unwrap() {
            assert!(parse(raw.as_str().unwrap()).is_err(), "{}", raw);
        }
    }
    #[test]
    fn team_invitation_requires_only_a_valid_code() {
        let code = "aB0_aB0_aB0_aB0_aB0_aB0_aB0_aB0-";
        for scheme in ["lingxi://", "https://lingxios.net/open/"] {
            let raw = format!("{scheme}team/join?code={code}");
            let parsed = parse(&raw).unwrap();
            assert_eq!(parsed.route, "team/join");
            assert_eq!(parsed.params["code"], code);
        }
        for raw in [
            "lingxi://team/join".to_owned(),
            "lingxi://team/join?code=short".to_owned(),
            format!("lingxi://team/join?code={code}&workspace=secret"),
            format!("lingxi://team/join?code={code}&code={code}"),
            format!("lingxi://team/join?code={code}#payload=%7B%7D"),
            format!("lingxi://new?code={code}"),
            format!("lingxi://team/join?code={}!", &code[..31]),
        ] {
            assert!(parse(&raw).is_err(), "{raw}");
        }
    }
    #[test]
    fn friend_links_carry_one_code_or_one_uid_and_nothing_else() {
        let code = "aB0_aB0_aB0_aB0_aB0_aB0_aB0_aB0-";
        let uid = "Hk3xQ9vR2mT7wL1pZ8cN4aB0cD2e";
        for (scheme, custom) in [("lingxi://", true), ("https://lingxios.net/open/", false)] {
            let add = parse(&format!("{scheme}friend/add?code={code}")).unwrap();
            assert_eq!(add.route, "friend/add");
            assert_eq!(add.params["code"], code);
            assert_eq!(parse(&build(&add, custom).unwrap()).unwrap(), add);
            let chat = parse(&format!("{scheme}friend/chat?uid={uid}")).unwrap();
            assert_eq!(chat.route, "friend/chat");
            assert_eq!(chat.params["uid"], uid);
            assert_eq!(parse(&build(&chat, custom).unwrap()).unwrap(), chat);
            let requests = parse(&format!("{scheme}friend/requests")).unwrap();
            assert_eq!(requests.route, "friend/requests");
        }
        let long = "u".repeat(129);
        for raw in [
            "lingxi://friend".to_owned(),
            "lingxi://friend/other".to_owned(),
            "lingxi://friend/add".to_owned(),
            "lingxi://friend/add?code=short".to_owned(),
            format!("lingxi://friend/add?code={}!", &code[..31]),
            format!("lingxi://friend/add?code={code}&workspace=secret"),
            format!("lingxi://friend/add?code={code}&uid={uid}"),
            format!("lingxi://friend/add?code={code}&code={code}"),
            format!("lingxi://friend/add?code={code}#payload=%7B%7D"),
            format!("lingxi://friend/requests?code={code}"),
            format!("lingxi://friend/requests?uid={uid}"),
            "lingxi://friend/chat".to_owned(),
            "lingxi://friend/chat?uid=".to_owned(),
            "lingxi://friend/chat?uid=a%2Fb".to_owned(),
            "lingxi://friend/chat?uid=a%20b".to_owned(),
            format!("lingxi://friend/chat?uid={long}"),
            format!("lingxi://friend/chat?uid={uid}&uid={uid}"),
            format!("lingxi://friend/chat?uid={uid}&code={code}"),
            format!("lingxi://friend/chat?uid={uid}#payload=%7B%7D"),
            format!("lingxi://team/inbox?uid={uid}"),
            format!("lingxi://new?uid={uid}"),
            format!("lingxi://team/join?code={code}&uid={uid}"),
        ] {
            assert!(parse(&raw).is_err(), "{raw}");
        }
    }

    #[test]
    fn message_uuid_is_canonical_without_folding_other_identities() {
        let target = Target {
            route: "session/CasePreserved".into(),
            params: BTreeMap::from([
                ("mode".into(), "code".into()),
                ("workspace".into(), "project.CasePreserved".into()),
                (
                    "message".into(),
                    "123E4567-E89B-12D3-A456-426614174000".into(),
                ),
            ]),
            payload: None,
        };
        for scheme in [false, true] {
            let raw = build(&target, scheme).unwrap();
            assert!(raw.contains("message=123e4567-e89b-12d3-a456-426614174000"));
            let parsed = parse(&raw).unwrap();
            assert_eq!(parsed.route, target.route);
            assert_eq!(parsed.params["workspace"], "project.CasePreserved");
            assert_eq!(
                parsed.params["message"],
                "123e4567-e89b-12d3-a456-426614174000"
            );
        }
    }

    #[test]
    fn rejects_bad_inputs() {
        for raw in [
            "https://evil/open/new",
            "lingxi://new?mode=chat&mode=code",
            "lingxi://new?x=1",
            "lingxi://session/%2f",
            "lingxi://user:password@new",
        ] {
            assert!(parse(raw).is_err());
        }
    }
}
