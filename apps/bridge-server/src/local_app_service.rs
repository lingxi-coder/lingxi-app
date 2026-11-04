//! Private stdio application-library host, independent of chat and credentials.
//!
//! The parent owns this process and supplies the canonical configuration root.
//! No network listener or discovery file exposes storage operations to pages.

use local_app::v2::{
    native_source_identity, AppError, AppLibrary, AuthoringRef, ProjectRef, PublishRequest,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    io::{BufRead, Write},
    path::PathBuf,
};

const MAX_REQUEST_BYTES: usize = 4 * 1024 * 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    id: String,
    request: Value,
}

// These operations are available only on the private native-host stdio
// channel. They are intentionally absent from the shared LibraryRequest enum.
fn native_authoring_request(
    library: &AppLibrary,
    request: &Value,
) -> Option<Result<Value, AppError>> {
    let op = request.get("op")?.as_str()?;
    if !matches!(
        op,
        "ensureAuthoringSource"
            | "copyAuthoringSource"
            | "remixAuthoringSource"
            | "findAuthoringSource"
            | "prepareAuthoringSource"
            | "registerAuthoring"
            | "getAuthoringGrant"
            | "fingerprintAuthoringPublication"
            | "getPublication"
            | "publishOnce"
            | "publishAuthoring"
    ) {
        return None;
    }
    Some((|| {
        let object = request
            .as_object()
            .ok_or_else(|| AppError::Invalid("request must be an object".into()))?;
        let required = |name: &str| -> Result<&str, AppError> {
            object
                .get(name)
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| AppError::Invalid(format!("{name} is required")))
        };
        let allowed: &[&str] = match op {
            "ensureAuthoringSource" => &[
                "op",
                "requestId",
                "sourceDirectory",
                "projectRef",
                "targetAppId",
            ],
            "copyAuthoringSource" => &[
                "op",
                "requestId",
                "appId",
                "revision",
                "sourcePath",
                "projectRef",
            ],
            "remixAuthoringSource" => {
                &["op", "requestId", "appId", "revision", "sourcePath", "name"]
            }
            "findAuthoringSource" => &["op", "sourceDirectory"],
            "prepareAuthoringSource" => &["op", "sourceDirectory", "sourceId"],
            "registerAuthoring" => &[
                "op",
                "sourceId",
                "requestId",
                "setCurrent",
                "authoring",
                "sessionMode",
            ],
            "getAuthoringGrant" => &[
                "op",
                "sourceDirectory",
                "projectRef",
                "authoringSessionId",
                "sessionMode",
            ],
            "fingerprintAuthoringPublication" => &["op", "sourcePath"],
            "getPublication" => &["op", "requestId"],
            "publishOnce" => &[
                "op",
                "requestId",
                "stagedDigest",
                "sourceDirectory",
                "request",
                "replayOnly",
            ],
            "publishAuthoring" => &[
                "op",
                "requestId",
                "stagedDigest",
                "sourceId",
                "sourceDirectory",
                "projectRef",
                "authoringSessionId",
                "setCurrent",
                "sessionMode",
                "request",
                "replayOnly",
            ],
            _ => unreachable!(),
        };
        if object.keys().any(|key| !allowed.contains(&key.as_str())) {
            return Err(AppError::Invalid("unknown native authoring field".into()));
        }
        let desktop_project = |value: &Value, source: &str| -> Result<ProjectRef, AppError> {
            let project: ProjectRef = serde_json::from_value(value.clone())?;
            if !matches!(&project, ProjectRef::DesktopPath { path } if path == source) {
                return Err(AppError::Invalid(
                    "Desktop Project must match the source".into(),
                ));
            }
            Ok(project)
        };
        match op {
            "remixAuthoringSource" => {
                let source = required("sourcePath")?;
                let request_id = required("requestId")?;
                let app_id = required("appId")?;
                let revision = object
                    .get("revision")
                    .and_then(Value::as_u64)
                    .filter(|r| *r > 0)
                    .ok_or_else(|| AppError::Invalid("positive revision required".into()))?;
                let name = match object.get("name") {
                    None => None,
                    Some(Value::String(name)) if !name.trim().is_empty() => Some(name.as_str()),
                    _ => return Err(AppError::Invalid("invalid remix name".into())),
                };
                let mut payload = request.clone();
                payload
                    .as_object_mut()
                    .expect("validated request")
                    .remove("requestId");
                let (source_id, source_directory, result) = if let Some(result) =
                    library.replay_native_remix(request_id, &payload)?
                {
                    result
                } else {
                    let (digest, origin) =
                        library.native_remix_source_metadata(app_id, revision, name)?;
                    let (source_id, source_directory) = library.create_native_authoring_source(
                        request_id,
                        &payload,
                        None,
                        None,
                        || {
                            library.copy_native_remix_source(
                                app_id,
                                revision,
                                std::path::Path::new(source),
                                name,
                            )?;
                            Ok(source.to_owned())
                        },
                        native_source_identity,
                    )?;
                    let result = library.publish_native_remix_once(
                        request_id,
                        &payload,
                        &source_id,
                        &source_directory,
                        std::path::Path::new(source),
                        &digest,
                        origin,
                        native_source_identity,
                    )?;
                    (source_id, source_directory, result)
                };
                Ok(
                    json!({"requestId":request_id,"sourceId":source_id,"sourceDirectory":source_directory,
                    "sourcePath":source_directory,"record":result.record,"release":result.release}),
                )
            }
            "copyAuthoringSource" => {
                let source = required("sourcePath")?;
                let request_id = required("requestId")?;
                let app_id = required("appId")?;
                let revision = object
                    .get("revision")
                    .and_then(Value::as_u64)
                    .filter(|r| *r > 0)
                    .ok_or_else(|| AppError::Invalid("positive revision required".into()))?;
                desktop_project(
                    object
                        .get("projectRef")
                        .ok_or_else(|| AppError::Invalid("projectRef is required".into()))?,
                    source,
                )?;
                let payload = json!({"op":"copyAuthoringSource","appId":app_id,"revision":revision,"sourcePath":source,"projectRef":object["projectRef"]});
                let (id, copied) = library.create_native_authoring_source(
                    request_id,
                    &payload,
                    Some(app_id),
                    None,
                    || {
                        library.copy_native_authoring_source(
                            app_id,
                            revision,
                            std::path::Path::new(source),
                        )?;
                        Ok(source.to_owned())
                    },
                    native_source_identity,
                )?;
                Ok(
                    json!({"requestId":request_id,"sourceId":id,"sourceDirectory":copied,"sourcePath":copied,"targetAppId":app_id,"record":library.get(app_id)?}),
                )
            }
            "ensureAuthoringSource" => {
                let source = required("sourceDirectory")?;
                desktop_project(
                    object
                        .get("projectRef")
                        .ok_or_else(|| AppError::Invalid("projectRef is required".into()))?,
                    source,
                )?;
                let request_id = required("requestId")?;
                let target = match object.get("targetAppId") {
                    None | Some(Value::Null) => None,
                    Some(Value::String(id)) if !id.is_empty() => Some(id.as_str()),
                    _ => return Err(AppError::Invalid("invalid targetAppId".into())),
                };
                let payload = json!({"op":"ensureAuthoringSource","sourceDirectory":source,"projectRef":object["projectRef"],"targetAppId":target});
                let (id, _) = library.create_native_authoring_source(
                    request_id,
                    &payload,
                    target,
                    None,
                    || Ok(source.to_owned()),
                    native_source_identity,
                )?;
                let mut value = json!({"requestId":request_id,"sourceId":id,"sourceDirectory":source,"sourcePath":source});
                if let Some(target) = target {
                    value["targetAppId"] = json!(target);
                }
                Ok(value)
            }
            "findAuthoringSource" | "prepareAuthoringSource" => {
                let source = required("sourceDirectory")?;
                let result = if op == "findAuthoringSource" {
                    library.find_native_authoring_source(source, || native_source_identity(source))
                } else {
                    library
                        .prepare_native_authoring_source(
                            source,
                            Some(required("sourceId")?),
                            || native_source_identity(source),
                        )
                        .map(|id| (id, None))
                };
                match result {
                    Ok((id, owner)) => {
                        let mut value =
                            json!({"sourceId":id,"sourceDirectory":source,"sourcePath":source});
                        if let Some(owner) = owner {
                            value["ownerAppId"] = json!(owner);
                        }
                        Ok(value)
                    }
                    Err(AppError::NotFound(_)) if op == "findAuthoringSource" => Ok(Value::Null),
                    Err(error) => Err(error),
                }
            }
            "registerAuthoring" => {
                let source_id = required("sourceId")?;
                let request_id = required("requestId")?;
                if required("sessionMode")? != "code" {
                    return Err(AppError::Invalid(
                        "authoring requires a Code session".into(),
                    ));
                }
                let set_current = object
                    .get("setCurrent")
                    .and_then(Value::as_bool)
                    .ok_or_else(|| AppError::Invalid("setCurrent must be a boolean".into()))?;
                let authoring: AuthoringRef = serde_json::from_value(
                    object
                        .get("authoring")
                        .cloned()
                        .ok_or_else(|| AppError::Invalid("authoring is required".into()))?,
                )?;
                desktop_project(
                    &serde_json::to_value(&authoring.project_ref)?,
                    &authoring.source_directory,
                )?;
                let mut value = serde_json::to_value(library.register_native_authoring(
                    authoring,
                    source_id,
                    request_id,
                    set_current,
                    |authoring| native_source_identity(&authoring.source_directory),
                )?)?;
                value["sessionMode"] = json!("code");
                Ok(value)
            }
            "getAuthoringGrant" => {
                let source = required("sourceDirectory")?;
                let project = desktop_project(
                    object
                        .get("projectRef")
                        .ok_or_else(|| AppError::Invalid("projectRef is required".into()))?,
                    source,
                )?;
                let session = required("authoringSessionId")?;
                if required("sessionMode")? != "code" {
                    return Err(AppError::Invalid(
                        "authoring requires a Code session".into(),
                    ));
                }
                let (source_id, authoring, owner) = library
                    .resolve_native_authoring_with_source_id(
                        source,
                        &project,
                        session,
                        |authoring| native_source_identity(&authoring.source_directory),
                    )?;
                let mut value =
                    json!({"sourceId":source_id,"authoring":authoring,"sessionMode":"code"});
                if let Some(owner) = owner {
                    value["ownerAppId"] = json!(owner);
                }
                Ok(value)
            }
            "publishAuthoring" => {
                let source = required("sourceDirectory")?;
                let source_id = required("sourceId")?;
                let project = desktop_project(
                    object
                        .get("projectRef")
                        .ok_or_else(|| AppError::Invalid("projectRef is required".into()))?,
                    source,
                )?;
                let session = required("authoringSessionId")?;
                if required("sessionMode")? != "code" {
                    return Err(AppError::Invalid(
                        "authoring requires a Code session".into(),
                    ));
                }
                let set_current = object
                    .get("setCurrent")
                    .and_then(Value::as_bool)
                    .ok_or_else(|| AppError::Invalid("setCurrent must be a boolean".into()))?;
                let publish: PublishRequest = serde_json::from_value(
                    object
                        .get("request")
                        .cloned()
                        .ok_or_else(|| AppError::Invalid("request is required".into()))?,
                )?;
                let replay_only = match object.get("replayOnly") {
                    None => false,
                    Some(Value::Bool(value)) => *value,
                    _ => return Err(AppError::Invalid("replayOnly must be a boolean".into())),
                };
                if replay_only {
                    return Ok(serde_json::to_value(
                        library.replay_native_authoring_publication(
                            required("requestId")?,
                            required("stagedDigest")?,
                            &publish,
                            source,
                            session,
                            Some(&project),
                            Some(source_id),
                            set_current,
                        )?,
                    )?);
                }
                Ok(serde_json::to_value(
                    library.publish_native_authoring_once(
                        required("requestId")?,
                        required("stagedDigest")?,
                        publish,
                        source,
                        session,
                        Some(&project),
                        Some(source_id),
                        set_current,
                        |authoring| native_source_identity(&authoring.source_directory),
                    )?,
                )?)
            }
            "fingerprintAuthoringPublication" => Ok(
                json!({"stagedDigest":library.fingerprint_publication(std::path::Path::new(required("sourcePath")?))?}),
            ),
            "getPublication" => {
                match library.native_authoring_publication(required("requestId")?)? {
                    Some((receipt, result)) => Ok(
                        json!({"payloadSha256":receipt.payload_sha256,"publisher":receipt.publisher,"sourceId":receipt.source_id,"record":result.record,"release":result.release}),
                    ),
                    None => Ok(Value::Null),
                }
            }
            "publishOnce" => {
                let source = Some(required("sourceDirectory")?);
                let publish: PublishRequest = serde_json::from_value(
                    object
                        .get("request")
                        .cloned()
                        .ok_or_else(|| AppError::Invalid("request is required".into()))?,
                )?;
                let replay_only = match object.get("replayOnly") {
                    None => false,
                    Some(Value::Bool(value)) => *value,
                    _ => return Err(AppError::Invalid("replayOnly must be a boolean".into())),
                };
                if replay_only {
                    return Ok(serde_json::to_value(library.replay_publication(
                        required("requestId")?,
                        required("stagedDigest")?,
                        &publish,
                        source,
                    )?)?);
                }
                Ok(serde_json::to_value(library.publish_once(
                    required("requestId")?,
                    required("stagedDigest")?,
                    publish,
                    source,
                )?)?)
            }
            _ => unreachable!(),
        }
    })())
}

fn config_home(args: impl IntoIterator<Item = String>) -> anyhow::Result<PathBuf> {
    let args: Vec<_> = args.into_iter().collect();
    anyhow::ensure!(
        args.len() == 2 && args[0] == "--config-home",
        "expected --config-home <absolute directory>"
    );
    let path = PathBuf::from(&args[1]);
    anyhow::ensure!(
        path.is_absolute(),
        "configuration directory must be absolute"
    );
    Ok(path)
}

/// Enter the bounded request loop without starting an Agent or reading credentials.
pub async fn run() -> anyhow::Result<()> {
    let home = config_home(std::env::args().skip(2))?;
    tokio::task::spawn_blocking(move || {
        let library = AppLibrary::open(home)?;
        // A reader reconnect must never recover records while another runtime
        // owner is alive. This guard is held for the entire private channel.
        let _runtime_owner = library.acquire_runtime_lease()?;
        serve(&library, std::io::stdin().lock(), std::io::stdout().lock())
    })
    .await??;
    Ok(())
}

/// Read one bounded line at a time. Oversized input closes the private channel,
/// rather than allocating an unbounded line or treating a suffix as a request.
fn serve(
    library: &AppLibrary,
    mut input: impl BufRead,
    mut output: impl Write,
) -> anyhow::Result<()> {
    loop {
        let mut line = Vec::new();
        loop {
            let available = input.fill_buf()?;
            if available.is_empty() {
                break;
            }
            let take = available
                .iter()
                .position(|b| *b == b'\n')
                .map_or(available.len(), |n| n + 1);
            anyhow::ensure!(
                line.len() + take <= MAX_REQUEST_BYTES,
                "application request exceeds size limit"
            );
            line.extend_from_slice(&available[..take]);
            input.consume(take);
            if line.last() == Some(&b'\n') {
                break;
            }
        }
        if line.is_empty() {
            return Ok(());
        }
        let response = match serde_json::from_slice::<Envelope>(&line) {
            Ok(envelope) if !envelope.id.is_empty() && envelope.id.len() <= 128 => {
                match native_authoring_request(library, &envelope.request)
                    .unwrap_or_else(|| library.request_json(&envelope.request.to_string()))
                {
                    Ok(value) => json!({"id":envelope.id,"ok":true,"value":value}),
                    Err(error) => {
                        json!({"id":envelope.id,"ok":false,"error":{"code":error.code(),"message":error.to_string()}})
                    }
                }
            }
            _ => {
                json!({"id":null,"ok":false,"error":{"code":"invalidRequest","message":"Invalid application service request"}})
            }
        };
        serde_json::to_writer(&mut output, &response)?;
        output.write_all(b"\n")?;
        output.flush()?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_protocol_is_correlated_and_recovers_after_invalid_request() {
        let root = tempfile::tempdir().unwrap();
        let library = AppLibrary::open(root.path()).unwrap();
        let input = b"{\"id\":\"bad\",\"request\":{\"op\":\"unknown\"}}\n{\"id\":\"list\",\"request\":{\"op\":\"list\"}}\n";
        let mut output = Vec::new();
        serve(&library, &input[..], &mut output).unwrap();
        let replies: Vec<Value> = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(replies[0]["id"], "bad");
        assert_eq!(replies[0]["ok"], false);
        assert_eq!(replies[1]["id"], "list");
        assert_eq!(replies[1]["ok"], true);
        assert_eq!(replies[1]["value"], json!([]));
    }

    #[test]
    fn private_protocol_does_not_route_legacy_authoring_operations() {
        let root = tempfile::tempdir().unwrap();
        let library = AppLibrary::open(root.path()).unwrap();
        let source = root.path().canonicalize().unwrap().join("source");
        std::fs::create_dir(&source).unwrap();
        std::fs::write(source.join("index.html"), "source").unwrap();
        std::fs::write(source.join("lingxi-app.json"), json!({"schema":2,"name":"App",
            "entry":{"type":"static","path":"index.html"},"targets":[{"platform":"macos","environment":"web"}],
            "remix":{"allowed":true,"license":"MIT"}}).to_string()).unwrap();
        let app = library
            .publish(PublishRequest {
                app_id: None,
                expected_revision: 0,
                source_path: source.to_string_lossy().into_owned(),
                authoring: None,
                market_origin: None,
            })
            .unwrap();
        let app_id = &app.record.app_id;
        let restore = source.with_file_name("restore");
        let remix = source.with_file_name("remix");
        let authoring = |path: &std::path::Path| {
            json!({"sourceDirectory":path,"workspaceId":uuid::Uuid::new_v4().to_string(),
            "projectRef":{"kind":"desktopPath","path":path},"authoringSessionId":uuid::Uuid::new_v4().to_string(),"appId":app_id})
        };
        let input = [
            json!({"id":"bind","request":{"op":"bindAuthoring","appId":app_id,"authoring":authoring(&source)}}),
            json!({"id":"restore","request":{"op":"restoreAuthoring","appId":app_id,"revision":1,"sourcePath":restore,"authoring":authoring(&restore)}}),
            json!({"id":"remix","request":{"op":"remix","appId":app_id,"revision":1,"sourcePath":remix,"authoring":authoring(&remix)}}),
        ].into_iter().map(|request| request.to_string() + "\n").collect::<String>();
        let mut output = Vec::new();
        serve(&library, input.as_bytes(), &mut output).unwrap();
        let responses: Vec<Value> = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(responses.len(), 3);
        assert!(responses.iter().all(|response| response["ok"] == false));
        assert!(library.get(app_id).unwrap().authoring.is_none());
        assert!(!restore.exists() && !remix.exists());
        assert_eq!(library.list().unwrap().len(), 1);
    }

    #[test]
    fn native_authoring_receipts_survive_rebind_and_fence_generic_publication() {
        let root = tempfile::tempdir().unwrap();
        let library = AppLibrary::open(root.path().join("config")).unwrap();
        let source = root.path().join("project");
        std::fs::create_dir(&source).unwrap();
        std::fs::write(source.join("index.html"), "source").unwrap();
        std::fs::write(source.join("lingxi-app.json"), json!({"schema":2,"name":"App","entry":{"type":"static","path":"index.html"},"targets":[{"platform":"macos","environment":"web"}]}).to_string()).unwrap();
        let source = source
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let project = json!({"kind":"desktopPath","path":source});
        let call = |value: Value| native_authoring_request(&library, &value).unwrap();
        assert_eq!(
            call(json!({"op":"findAuthoringSource","sourceDirectory":source})).unwrap(),
            Value::Null
        );
        let request_id = uuid::Uuid::new_v4().to_string();
        let proof = call(json!({"op":"ensureAuthoringSource","requestId":request_id,"sourceDirectory":source,"projectRef":project})).unwrap();
        let source_id = proof["sourceId"].as_str().unwrap();
        let session_a = uuid::Uuid::new_v4().to_string();
        let session_b = uuid::Uuid::new_v4().to_string();
        let binding = |session: &str| json!({"sourceDirectory":source,"workspaceId":uuid::Uuid::new_v4().to_string(),"projectRef":project,"authoringSessionId":session});
        let a = binding(&session_a);
        let b = binding(&session_b);
        call(json!({"op":"registerAuthoring","sourceId":source_id,"requestId":request_id,"sessionMode":"code","setCurrent":false,"authoring":a})).unwrap();
        call(json!({"op":"registerAuthoring","sourceId":source_id,"requestId":request_id,"sessionMode":"code","setCurrent":true,"authoring":b})).unwrap();
        let staged = root.path().join("staged");
        std::fs::create_dir(&staged).unwrap();
        for name in ["index.html", "lingxi-app.json"] {
            std::fs::copy(std::path::Path::new(&source).join(name), staged.join(name)).unwrap();
        }
        let staged_digest =
            call(json!({"op":"fingerprintAuthoringPublication","sourcePath":staged})).unwrap()
                ["stagedDigest"]
                .clone();
        let publish_id = uuid::Uuid::new_v4().to_string();
        let request = json!({"op":"publishAuthoring","requestId":publish_id,"stagedDigest":staged_digest,"sourceId":source_id,"sourceDirectory":source,"projectRef":project,"authoringSessionId":session_a,"sessionMode":"code","setCurrent":true,"request":{"expectedRevision":0,"sourcePath":staged}});
        let published = call(request.clone()).unwrap();
        std::fs::remove_dir_all(&staged).unwrap();
        assert_eq!(call(request.clone()).unwrap(), published);
        let mut replay = request.clone();
        replay["replayOnly"] = json!(true);
        assert_eq!(call(replay.clone()).unwrap(), published);
        let mut missing = replay.clone();
        missing["requestId"] = json!(uuid::Uuid::new_v4().to_string());
        assert!(call(missing).is_err());
        assert_eq!(library.list().unwrap().len(), 1);
        replay["stagedDigest"] = json!("0".repeat(64));
        assert!(call(replay).is_err());
        let mut conflict = request.clone();
        conflict["stagedDigest"] = json!("0".repeat(64));
        assert!(call(conflict).is_err());
        let id = published["record"]["appId"].as_str().unwrap();
        assert_eq!(
            published["record"]["authoring"]["authoringSessionId"],
            session_b
        );
        let outcome = call(json!({"op":"getPublication","requestId":publish_id})).unwrap();
        assert_eq!(outcome["publisher"]["authoringSessionId"], session_a);
        assert_eq!(outcome["sourceId"], source_id);
        let grant = call(json!({"op":"getAuthoringGrant","sourceDirectory":source,"projectRef":project,"authoringSessionId":session_b,"sessionMode":"code"})).unwrap();
        assert_eq!(grant["ownerAppId"], id);
        assert_eq!(grant["sourceId"], source_id);
        let mut wrong_owner = request;
        wrong_owner["authoringSessionId"] = json!(session_b);
        assert!(call(wrong_owner).is_err());
        let restored = root
            .path()
            .canonicalize()
            .unwrap()
            .join("restored")
            .to_string_lossy()
            .into_owned();
        let copy_request = json!({"op":"copyAuthoringSource","requestId":uuid::Uuid::new_v4().to_string(),"appId":id,"revision":1,"sourcePath":restored,"projectRef":{"kind":"desktopPath","path":restored}});
        let copied = call(copy_request.clone()).unwrap();
        assert_eq!(call(copy_request).unwrap(), copied);
        assert_eq!(
            library
                .get(id)
                .unwrap()
                .authoring
                .unwrap()
                .authoring_session_id
                .as_deref(),
            Some(session_b.as_str())
        );
        assert!(library
            .request_json(
                &json!({"op":"publish","appId":id,"expectedRevision":1,"sourcePath":source})
                    .to_string()
            )
            .is_err());
        assert!(library.request_json(&json!({"op":"getAuthoringGrant","sourceDirectory":source,"projectRef":project,"authoringSessionId":session_a}).to_string()).is_err());
    }

    #[test]
    fn native_desktop_wire_fixture_uses_actual_private_stdio() {
        let root = tempfile::tempdir().unwrap();
        let library = AppLibrary::open(root.path().join("config")).unwrap();
        let source = root.path().join("project");
        std::fs::create_dir(&source).unwrap();
        std::fs::write(source.join("index.html"), "source").unwrap();
        std::fs::write(
            source.join("lingxi-app.json"),
            json!({"schema":2,"name":"Desktop fixture","entry":{"type":"static","path":"index.html"},"targets":[{"platform":"macos","environment":"web"}]}).to_string(),
        )
        .unwrap();
        let source = source
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let project = json!({"kind":"desktopPath","path":source});
        let invoke = |id: &str, request: Value| -> Value {
            let envelope = json!({"id":id,"request":request});
            let mut input = serde_json::to_vec(&envelope).unwrap();
            input.push(b'\n');
            let mut output = Vec::new();
            serve(&library, input.as_slice(), &mut output).unwrap();
            let response: Value = serde_json::from_slice(&output).unwrap();
            assert_eq!(response["id"], id);
            assert_eq!(response["ok"], true, "{response}");
            json!({"request":envelope,"response":response})
        };
        let creation = uuid::Uuid::new_v4().to_string();
        let ensured = invoke(
            "ensure",
            json!({"op":"ensureAuthoringSource","requestId":creation,
            "sourceDirectory":source,"projectRef":project}),
        );
        let source_id = ensured["response"]["value"]["sourceId"].as_str().unwrap();
        let session = uuid::Uuid::new_v4().to_string();
        let registered = invoke(
            "register",
            json!({"op":"registerAuthoring","requestId":creation,
            "sourceId":source_id,"setCurrent":true,"sessionMode":"code","authoring":{
                "sourceDirectory":source,"workspaceId":uuid::Uuid::new_v4().to_string(),
                "projectRef":project,"authoringSessionId":session}}),
        );
        let grant_request = json!({"op":"getAuthoringGrant","sourceDirectory":source,
            "projectRef":project,"authoringSessionId":session,"sessionMode":"code"});
        let grant = invoke("get", grant_request.clone());
        assert_eq!(grant["response"]["value"]["sourceId"], source_id);
        assert_eq!(
            grant["response"]["value"]["authoring"]["projectRef"],
            project
        );

        let staged = root.path().join("stage");
        std::fs::create_dir(&staged).unwrap();
        for name in ["index.html", "lingxi-app.json"] {
            std::fs::copy(PathBuf::from(&source).join(name), staged.join(name)).unwrap();
        }
        let fingerprint = invoke(
            "fingerprint",
            json!({"op":"fingerprintAuthoringPublication","sourcePath":staged}),
        );
        let digest = &fingerprint["response"]["value"]["stagedDigest"];
        let publish_id = uuid::Uuid::new_v4().to_string();
        let publish_request = json!({"op":"publishAuthoring","requestId":publish_id,
            "stagedDigest":digest,"sourceId":source_id,"sourceDirectory":source,"projectRef":project,
            "authoringSessionId":session,"sessionMode":"code","setCurrent":true,
            "request":{"expectedRevision":0,"sourcePath":staged}});
        let publication = invoke("publish", publish_request.clone());
        let app_id = publication["response"]["value"]["record"]["appId"]
            .as_str()
            .unwrap();
        let granted = invoke("getPublished", grant_request);
        assert_eq!(granted["response"]["value"]["ownerAppId"], app_id);
        let lookup = invoke(
            "publication",
            json!({"op":"getPublication","requestId":publish_id}),
        );
        assert_eq!(lookup["response"]["value"]["sourceId"], source_id);
        std::fs::remove_dir_all(&staged).unwrap();
        let mut replay_request = publish_request;
        replay_request["replayOnly"] = json!(true);
        let replay = invoke("replay", replay_request);
        assert_eq!(
            replay["response"]["value"],
            publication["response"]["value"]
        );
        let fixture = json!({"schema":1,"ensure":ensured,"register":registered,"get":grant,
            "fingerprint":fingerprint,"publish":publication,"getPublished":granted,
            "publication":lookup,"replay":replay});
        if let Some(path) = std::env::var_os("LINGXI_DESKTOP_AUTHORING_FIXTURE_OUT") {
            std::fs::write(path, serde_json::to_vec_pretty(&fixture).unwrap()).unwrap();
        }
    }

    #[test]
    fn native_desktop_remix_creates_new_source_and_replays_after_source_removal() {
        let root = tempfile::tempdir().unwrap();
        let library = AppLibrary::open(root.path().join("config")).unwrap();
        let source = root.path().join("source");
        std::fs::create_dir(&source).unwrap();
        std::fs::write(source.join("index.html"), "source").unwrap();
        std::fs::write(source.join("lingxi-app.json"), json!({"schema":2,"name":"Original",
            "entry":{"type":"static","path":"index.html"},"targets":[{"platform":"macos","environment":"web"}],
            "remix":{"allowed":true,"license":"MIT"}}).to_string()).unwrap();
        let original = library
            .publish(PublishRequest {
                app_id: None,
                expected_revision: 0,
                source_path: source.to_string_lossy().into_owned(),
                authoring: None,
                market_origin: None,
            })
            .unwrap();
        let target = root.path().canonicalize().unwrap().join("remix");
        let request = json!({"op":"remixAuthoringSource","requestId":uuid::Uuid::new_v4().to_string(),
            "appId":original.record.app_id,"revision":1,"sourcePath":target});
        let invoke = || native_authoring_request(&library, &request).unwrap();
        let result = invoke().unwrap();
        assert_ne!(result["record"]["appId"], original.record.app_id);
        assert!(result["record"]["authoring"].is_null());
        assert_eq!(
            result["record"]["authoringSourceIds"][0],
            result["sourceId"]
        );
        assert!(library.request_json(&request.to_string()).is_err());
        std::fs::remove_dir_all(&target).unwrap();
        assert_eq!(invoke().unwrap(), result);
        assert_eq!(
            library
                .get(&original.record.app_id)
                .unwrap()
                .latest_revision,
            1
        );
    }

    #[test]
    fn ordinary_publication_replay_only_requires_exact_committed_receipt() {
        let root = tempfile::tempdir().unwrap();
        let library = AppLibrary::open(root.path().join("config")).unwrap();
        let source = root.path().join("ordinary");
        std::fs::create_dir(&source).unwrap();
        std::fs::write(source.join("index.html"), "source").unwrap();
        std::fs::write(source.join("lingxi-app.json"), json!({"schema":2,"name":"App","entry":{"type":"static","path":"index.html"},"targets":[{"platform":"macos","environment":"web"}]}).to_string()).unwrap();
        let call = |value: Value| native_authoring_request(&library, &value).unwrap();
        let digest = call(json!({"op":"fingerprintAuthoringPublication","sourcePath":source}))
            .unwrap()["stagedDigest"]
            .clone();
        let id = uuid::Uuid::new_v4().to_string();
        let request = json!({"op":"publishOnce","requestId":id,"stagedDigest":digest,
            "sourceDirectory":source,"request":{"expectedRevision":0,"sourcePath":source}});
        let mut no_source = request.clone();
        no_source.as_object_mut().unwrap().remove("sourceDirectory");
        assert!(call(no_source).is_err());
        let mut empty_source = request.clone();
        empty_source["sourceDirectory"] = json!("");
        assert!(call(empty_source).is_err());
        let mut replay = request.clone();
        replay["replayOnly"] = json!(true);
        assert!(call(replay.clone()).is_err());
        assert!(library.list().unwrap().is_empty());
        let committed = call(request).unwrap();
        std::fs::remove_dir_all(&source).unwrap();
        assert_eq!(call(replay.clone()).unwrap(), committed);
        replay["request"]["expectedRevision"] = json!(1);
        assert!(call(replay).is_err());
        assert_eq!(library.list().unwrap().len(), 1);
    }

    #[test]
    fn rejects_unbounded_request_before_dispatch() {
        let root = tempfile::tempdir().unwrap();
        let library = AppLibrary::open(root.path()).unwrap();
        let input = vec![b' '; MAX_REQUEST_BYTES + 1];
        let mut output = Vec::new();
        assert!(serve(&library, &input[..], &mut output).is_err());
        assert!(output.is_empty());
    }

    #[test]
    fn requires_explicit_absolute_configuration_owner() {
        assert!(config_home(["--config-home".into(), "relative".into()]).is_err());
        assert!(config_home([
            "--config-home".into(),
            "/tmp/fixture".into(),
            "extra".into()
        ])
        .is_err());
        assert_eq!(
            config_home(["--config-home".into(), "/tmp/fixture".into()]).unwrap(),
            PathBuf::from("/tmp/fixture")
        );
    }
}
