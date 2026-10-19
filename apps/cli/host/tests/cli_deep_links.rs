//! URL commands stay outside the model-turn path, including piped invocations.
use assert_cmd::Command;
use lingxi_deep_links::parse;
use predicates::prelude::*;
use sha2::{Digest, Sha256};

const SESSION: &str = "11111111-2222-3333-4444-555555555555";
const MESSAGE: &str = "83f67a72-a806-49b7-9a18-e57307177a86";

fn generated_link(args: &[&str]) -> String {
    let home = tempfile::tempdir().unwrap();
    let output = Command::cargo_bin("lingxi-cli")
        .unwrap()
        .timeout(std::time::Duration::from_secs(10))
        .env("LINGXI_CONFIG_DIR", home.path())
        .args(args)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    String::from_utf8(output).unwrap().trim().to_owned()
}

#[test]
fn link_session_message_roundtrips_in_both_formats() {
    let route = format!("session/{SESSION}");
    let input_message = MESSAGE.to_ascii_uppercase();
    for format in ["https", "scheme"] {
        let url = generated_link(&[
            "link",
            &route,
            "--workspace",
            "project.local",
            "--message",
            &input_message,
            "--format",
            format,
        ]);
        assert!(url.starts_with(if format == "https" {
            "https://lingxios.net/open/"
        } else {
            "lingxi://"
        }));
        let target = parse(&url).unwrap();
        assert_eq!(target.route, route);
        assert_eq!(target.params["workspace"], "project.local");
        assert_eq!(target.params["message"], MESSAGE);
        assert!(
            url.contains(&format!("message={MESSAGE}")),
            "generated UUID must be canonical lowercase"
        );
        assert!(target.payload.is_none());
    }
}

#[test]
fn link_new_roundtrips_unicode_multiline_draft_in_fragment() {
    let draft = "继续检查 👩🏽‍💻\n  保留缩进 & query=? #片段";
    let url = generated_link(&["link", "new", "--prompt", draft]);
    let target = parse(&url).unwrap();
    assert_eq!(target.route, "new");
    assert_eq!(target.payload.unwrap()["prompt"], draft);
    assert!(url.contains("#payload="));
    assert!(!url.split('#').next().unwrap().contains("prompt"));
}

#[test]
fn link_project_uses_canonical_hash_without_disclosing_path() {
    let project = tempfile::tempdir().unwrap();
    let canonical = project.path().canonicalize().unwrap();
    let project_arg = project.path().join(".");
    let url = generated_link(&["link", "new", "--project", project_arg.to_str().unwrap()]);
    let target = parse(&url).unwrap();
    assert_eq!(
        target.params["workspace"],
        format!(
            "project.{:x}",
            Sha256::digest(canonical.to_string_lossy().as_bytes())
        )
    );
    assert!(!url.contains(canonical.to_str().unwrap()));
    assert!(!url.contains(project.path().file_name().unwrap().to_str().unwrap()));
}

#[test]
fn link_rejects_draft_on_existing_session_and_invalid_message_identity() {
    let route = format!("session/{SESSION}");
    for args in [
        vec!["link", route.as_str(), "--prompt", "must stay a draft"],
        vec!["link", route.as_str(), "--message", "not-a-uuid"],
    ] {
        let home = tempfile::tempdir().unwrap();
        Command::cargo_bin("lingxi-cli")
            .unwrap()
            .timeout(std::time::Duration::from_secs(10))
            .env("LINGXI_CONFIG_DIR", home.path())
            .args(args)
            .assert()
            .code(1)
            .stdout(predicate::eq(""));
    }
}

#[test]
fn open_cli_without_terminal_exits_before_runtime_or_draft_submission() {
    let home = tempfile::tempdir().unwrap();
    // A runtime attempting to initialize this store would fail. The exact
    // terminal error therefore also verifies the guard precedes runtime init.
    let blocked_store = home.path().join("config-is-a-file");
    std::fs::write(&blocked_store, "untouched").unwrap();
    let draft_url = generated_link(&["link", "new", "--prompt", "never submit this draft"]);
    for url in [
        draft_url.as_str(),
        "lingxi://session/11111111-2222-3333-4444-555555555555",
        "lingxi://settings/general",
        "not a link",
    ] {
        Command::cargo_bin("lingxi-cli")
            .unwrap()
            .timeout(std::time::Duration::from_secs(10))
            .env("LINGXI_CONFIG_DIR", &blocked_store)
            .env("ANTHROPIC_API_KEY", "sk-test-not-a-real-key")
            .args(["open", url, "--cli"])
            .write_stdin("this stdin must not become a prompt\n")
            .assert()
            .code(1)
            .stdout(predicate::eq(""))
            .stderr(predicate::eq("--cli requires an interactive terminal; use the default desktop handoff otherwise\n"));
        assert_eq!(
            std::fs::read_to_string(&blocked_store).unwrap(),
            "untouched"
        );
        assert_eq!(std::fs::read_dir(home.path()).unwrap().count(), 1);
    }
}

#[test]
fn malformed_open_urls_fail_validation_without_becoming_prompts() {
    for (url, expected) in [
        ("https://example.com/open/new", "Unsupported origin"),
        (
            "lingxi://session/x?message=invalid",
            "Invalid message identity",
        ),
        ("lingxi://new?api_key=secret", "Invalid parameter"),
    ] {
        let home = tempfile::tempdir().unwrap();
        Command::cargo_bin("lingxi-cli")
            .unwrap()
            .timeout(std::time::Duration::from_secs(10))
            .env("LINGXI_CONFIG_DIR", home.path())
            .env("ANTHROPIC_API_KEY", "sk-test-not-a-real-key")
            .args(["open", url])
            .write_stdin("must not be read as a prompt\n")
            .assert()
            .code(1)
            .stdout(predicate::eq(""))
            .stderr(predicate::str::contains(expected));
        assert!(
            !home.path().join("projects").exists(),
            "invalid URL created a session"
        );
    }
}

#[cfg(unix)]
#[test]
fn desktop_handoff_from_custom_store_rejects_local_targets_before_os_opener() {
    use std::os::unix::fs::PermissionsExt;

    let home = tempfile::tempdir().unwrap();
    let bin = tempfile::tempdir().unwrap();
    let marker = home.path().join("opener-was-started");
    for name in ["open", "xdg-open"] {
        let opener = bin.path().join(name);
        std::fs::write(
            &opener,
            "#!/bin/sh\n: > \"$LINGXI_TEST_OPENER_MARKER\"\nexit 0\n",
        )
        .unwrap();
        std::fs::set_permissions(&opener, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let inherited_path = std::env::var_os("PATH").unwrap_or_default();
    let path = std::env::join_paths(
        std::iter::once(bin.path().to_path_buf()).chain(std::env::split_paths(&inherited_path)),
    )
    .unwrap();
    let guard = "Desktop cannot verify this custom LINGXI_CONFIG_DIR; use --cli for this local destination\n";
    for (url, expected) in [
        (format!("lingxi://session/{SESSION}"), guard),
        ("lingxi://project/project.local".into(), guard),
        ("lingxi://new?workspace=project.local".into(), guard),
        // Validation takes precedence over the custom-store guard even when
        // the malformed URL otherwise points at a local session.
        (
            format!("lingxi://session/{SESSION}?message=invalid"),
            "Invalid message identity\n",
        ),
    ] {
        Command::cargo_bin("lingxi-cli")
            .unwrap()
            .timeout(std::time::Duration::from_secs(10))
            .env("LINGXI_CONFIG_DIR", home.path())
            .env("LINGXI_TEST_OPENER_MARKER", &marker)
            .env("PATH", &path)
            .args(["open", &url])
            .assert()
            .code(1)
            .stdout(predicate::eq(""))
            .stderr(predicate::eq(expected));
        assert!(
            !marker.exists(),
            "custom-store refusal must precede OS handoff"
        );
    }
}

#[test]
fn cli_resolution_requires_existing_local_identity_and_keeps_new_text_as_draft() {
    use cli::argv::Argv;
    use cli::commands::deep_links::{prepare_cli, Open};
    use lingxi_deep_links::{build, Target};
    use std::collections::BTreeMap;

    struct RestoreConfig(Option<std::ffi::OsString>);
    impl Drop for RestoreConfig {
        fn drop(&mut self) {
            match &self.0 {
                Some(value) => std::env::set_var("LINGXI_CONFIG_DIR", value),
                None => std::env::remove_var("LINGXI_CONFIG_DIR"),
            }
        }
    }
    // This is the only test in this binary mutating the parent environment;
    // every spawned command above supplies its own explicit config directory.
    let _restore = RestoreConfig(std::env::var_os("LINGXI_CONFIG_DIR"));
    let home = tempfile::tempdir().unwrap();
    std::env::set_var("LINGXI_CONFIG_DIR", home.path());
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    let first_cwd = first.path().canonicalize().unwrap();
    let workspace = format!(
        "project.{:x}",
        Sha256::digest(first_cwd.to_string_lossy().as_bytes())
    );
    for project in [&first, &second] {
        let cwd = project.path().canonicalize().unwrap();
        let path = session::session_path(home.path(), &cwd.to_string_lossy(), SESSION);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let row = serde_json::json!({"cwd": cwd, "uuid": MESSAGE, "type": "user"});
        std::fs::write(path, format!("{row}\n")).unwrap();
    }
    let resolve = |route: &str, params: BTreeMap<String, String>, payload| {
        let url = build(
            &Target {
                route: route.into(),
                params,
                payload,
            },
            true,
        )
        .unwrap();
        let mut argv = Argv::from_iter(["lingxi-cli", "open", &url, "--cli"]).unwrap();
        let result = prepare_cli(&Open { url, cli: true }, &mut argv);
        (result, argv)
    };
    let route = format!("session/{SESSION}");
    let (result, argv) = resolve(
        &route,
        BTreeMap::from([
            ("workspace".into(), workspace.clone()),
            ("message".into(), MESSAGE.to_ascii_uppercase()),
        ]),
        None,
    );
    result.unwrap();
    assert_eq!(argv.cwd.as_ref(), Some(&first_cwd));
    assert_eq!(argv.resume.as_deref(), Some(SESSION));
    assert_eq!(argv.deep_link_message.as_deref(), Some(MESSAGE));
    assert!(argv.command.is_none());
    assert!(argv.prompt.is_none());
    assert!(argv.deep_link_origin);
    assert!(!argv.brief);

    let first_transcript =
        session::session_path(home.path(), &first_cwd.to_string_lossy(), SESSION);
    for hidden in [
        serde_json::json!({"type": "user", "isSidechain": true}),
        serde_json::json!({"type": "assistant", "isVisibleInTranscriptOnly": true}),
        serde_json::json!({"type": "user", "isCompactSummary": true}),
        serde_json::json!({"type": "progress"}),
        serde_json::json!({"type": "file-history-snapshot"}),
        serde_json::json!({"type": "system"}),
    ] {
        let mut row = hidden;
        row["uuid"] = MESSAGE.into();
        row["cwd"] = first_cwd.to_string_lossy().as_ref().into();
        std::fs::write(&first_transcript, format!("{row}\n")).unwrap();
        assert!(
            resolve(
                &route,
                BTreeMap::from([
                    ("workspace".into(), workspace.clone()),
                    ("message".into(), MESSAGE.into())
                ]),
                None
            )
            .0
            .unwrap_err()
            .contains("Message is unavailable"),
            "hidden row must not create a successful anchor: {row}"
        );
    }
    for visible in [
        serde_json::json!({"type": "assistant"}),
        serde_json::json!({"type": "system", "subtype": "compact_boundary"}),
        serde_json::json!({"type": "user"}),
    ] {
        let mut row = visible;
        row["uuid"] = MESSAGE.into();
        row["cwd"] = first_cwd.to_string_lossy().as_ref().into();
        std::fs::write(&first_transcript, format!("{row}\n")).unwrap();
        resolve(
            &route,
            BTreeMap::from([
                ("workspace".into(), workspace.clone()),
                ("message".into(), MESSAGE.into()),
            ]),
            None,
        )
        .0
        .unwrap();
    }

    assert!(resolve(&route, BTreeMap::new(), None)
        .0
        .unwrap_err()
        .contains("ambiguous"));
    assert!(resolve(
        &route,
        BTreeMap::from([
            ("workspace".into(), workspace.clone()),
            (
                "message".into(),
                "00000000-0000-0000-0000-000000000099".into()
            )
        ]),
        None
    )
    .0
    .unwrap_err()
    .contains("Message is unavailable"));
    assert!(resolve(
        &route,
        BTreeMap::from([("workspace".into(), "project.unknown".into())]),
        None
    )
    .0
    .is_err());

    let draft = "先审阅，不自动发送 👩🏽‍💻";
    let (result, argv) = resolve(
        "new",
        BTreeMap::from([("workspace".into(), workspace)]),
        Some(serde_json::json!({"prompt": draft})),
    );
    result.unwrap();
    assert_eq!(argv.cwd.as_ref(), Some(&first_cwd));
    assert_eq!(argv.prefill.as_deref(), Some(draft));
    assert!(argv.prompt.is_none());
    assert!(argv.resume.is_none());
}

#[test]
fn cli_link_rejects_options_that_could_submit_or_resume_another_turn() {
    use cli::argv::Argv;
    use cli::commands::deep_links::{prepare_cli, Open};

    for option in [
        "print",
        "input_format",
        "output_format",
        "continue",
        "resume",
        "prompt",
        "resume_session_at",
        "resume_drops_turn",
        "prefill",
        "prefill_b64",
    ] {
        let open = Open {
            url: "lingxi://new".into(),
            cli: true,
        };
        let mut argv = Argv::from_iter(["lingxi-cli", "open", &open.url, "--cli"]).unwrap();
        match option {
            "print" => argv.print = true,
            "input_format" => argv.input_format = Some("stream-json".into()),
            "output_format" => argv.output_format = Some("json".into()),
            "continue" => argv.continue_session = true,
            "resume" => argv.resume = Some(SESSION.into()),
            "prompt" => argv.prompt = Some("never submit this".into()),
            "resume_session_at" => argv.resume_session_at = Some(MESSAGE.into()),
            "resume_drops_turn" => argv.resume_drops_turn = Some(MESSAGE.into()),
            "prefill" => argv.prefill = Some("unrelated existing draft".into()),
            "prefill_b64" => argv.prefill_b64 = Some("ZHJhZnQ=".into()),
            _ => unreachable!(),
        }
        let original_prefill = argv.prefill.clone();
        assert_eq!(
            prepare_cli(&open, &mut argv).unwrap_err(),
            "--cli links cannot be combined with prompt, print, streaming, or resume options",
            "conflicting {option} must fail before resolving a local project"
        );
        assert!(
            argv.command.is_some(),
            "rejection must not enter the TUI path"
        );
        assert!(!argv.deep_link_origin);
        assert_eq!(argv.prefill, original_prefill);
    }
}
