//! The project settings command edits an unopened directory, without a conversation or runtime.
use serde_json::{json, Value};
use std::{
    fs,
    io::Write,
    path::Path,
    process::{Command, Stdio},
};

fn settings(home: &Path, project: &Path, command: Value) -> Result<Vec<Value>, String> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_bridge-server"))
        .arg("--project-settings-command")
        .env("LINGXI_CONFIG_DIR", home)
        .current_dir(project)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(command.to_string().as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned());
    }
    serde_json::from_slice(&output.stdout).map_err(|e| e.to_string())
}

fn payload(event: &Value, field: &str) -> Value {
    serde_json::from_str(event[field].as_str().unwrap()).unwrap()
}

fn transcripts(root: &Path) -> Vec<std::path::PathBuf> {
    let mut found = Vec::new();
    for item in fs::read_dir(root).unwrap() {
        let path = item.unwrap().path();
        if path.is_dir() {
            found.extend(transcripts(&path));
        } else if path.extension().is_some_and(|ext| ext == "jsonl") {
            found.push(path);
        }
    }
    found.sort();
    found
}

#[test]
fn config_uses_selected_project_without_creating_or_touching_a_conversation() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("config");
    let a = tmp.path().join("active");
    let b = tmp.path().join("without-chats");
    for dir in [&a, &b] {
        fs::create_dir_all(dir.join(".lingxi")).unwrap();
        fs::write(
            dir.join(".lingxi/settings.json"),
            json!({"enabledTools":[dir.file_name().unwrap().to_str().unwrap()]}).to_string(),
        )
        .unwrap();
    }
    let chat = home.join("projects/active/existing.jsonl");
    fs::create_dir_all(chat.parent().unwrap()).unwrap();
    fs::write(&chat, b"existing conversation\n").unwrap();
    let a_before = fs::read(a.join(".lingxi/settings.json")).unwrap();
    let run = |cmd| settings(&home, &b, cmd).unwrap();
    let read = || json!({"type":"refresh_listings","which":[{"type":"settings"}]});
    assert_eq!(
        payload(&run(read())[0], "layers_json")["project"]["enabledTools"],
        json!(["without-chats"])
    );
    run(
        json!({"type":"update_settings","destination":"project","patch_json":"{\"syncClaudeAiSkills\":true}"}),
    );
    let changed: Value =
        serde_json::from_slice(&fs::read(b.join(".lingxi/settings.json")).unwrap()).unwrap();
    assert_eq!(changed["syncClaudeAiSkills"], true);
    run(
        json!({"type":"update_permission_rules","destination":"project","behavior":"allow","add":["Read"],"remove":[]}),
    );
    assert_eq!(
        payload(&run(read())[0], "layers_json")["project"]["permissions"]["allow"],
        json!(["Read"])
    );

    let mcp = run(json!({"type":"mcp_admin","command":{"action":"get_snapshot"}}));
    let scopes = payload(&mcp[0], "snapshot_json");
    let revision = &scopes["scopes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["scope"] == "project")
        .unwrap()["revision_sha256"];
    let command = json!({"type":"mcp_admin","command":{"action":"save_server","operation_id":1,"scope":"project","revision":revision,
        "payload_json":json!({"scope":"project","name":"docs","config":{"type":"http","url":"https://example.org/mcp"}}).to_string()}});
    run(command.clone());
    assert!(
        settings(&home, &b, command).is_err(),
        "stale revisions must be rejected"
    );
    assert!(b.join(".mcp.json").is_file());
    assert!(!a.join(".mcp.json").exists());

    run(
        json!({"type":"skill_admin","command":{"action":"create_skill","operation_id":2,"scope":"project",
        "revision":"e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        "payload_json":json!({"name":"directory-check","markdown":"---\nname: directory-check\ndescription: Verify the directory.\n---\nRead settings."}).to_string()}}),
    );
    let skills = payload(
        &run(json!({"type":"skill_admin","command":{"action":"get_catalog"}}))[0],
        "catalog_json",
    );
    assert!(skills["entries"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["name"] == "directory-check" && entry["source"] == "project"));
    assert!(
        run(json!({"type":"plugin_admin","command":{"action":"get_catalog"}}))
            .iter()
            .any(|e| e["type"] == "plugin_catalog")
    );
    assert!(run(
        json!({"type":"hook_admin","command":{"action":"get_document","scope":"project"}})
    )
    .iter()
    .any(|e| e["domain"] == "hook"));
    assert!(settings(&home, &b, json!({"type":"new_session"})).is_err());
    assert_eq!(fs::read(a.join(".lingxi/settings.json")).unwrap(), a_before);
    assert_eq!(transcripts(tmp.path()), vec![chat.clone()]);
    assert_eq!(fs::read(chat).unwrap(), b"existing conversation\n");
}
