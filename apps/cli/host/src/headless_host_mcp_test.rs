//! Consumer acceptance through the actual CLI host and the production pinned
//! Harness dependency. No replacement host, local runtime path or broker bypass.
use super::*;
use harness_runtime::desktop::{DesktopConfig, DesktopSessionComposition};
use harness_runtime::headless::{
    HeadlessConfig, HeadlessDeliveryStatus, HeadlessIo, HeadlessOptions, HeadlessShutdownStatus,
    HeadlessSignals, InputFormat, OutputFormat, SessionStart,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::io::Write as _;
use std::path::PathBuf;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, DuplexStream};

const CHILD_ROOT: &str = "LINGXI_MCP_CONSUMER_TEST_ROOT";
const RECEIPTS: &str = "LINGXI_MCP_CONSUMER_RECEIPTS";
const INPUT_BYTES: &[u8] = b"CLI_CONSUMER_INPUT\0caf\xc3\xa9 + \xce\xa9/\xf0\x9f\x98\x80";
const OUTPUT_SUFFIX: &[u8] = b"\0CLI_HOST_OUTPUT\xff";

/// Use a fresh test process so the real host's process-global config/identity
/// lookups cannot read user state or race other tests' environment. HOME is
/// inherited unchanged; only the existing config-dir and temp-dir authorities
/// point at this disposable fixture. This does not change production auth.
#[test]
fn cli_headless_mcp_staging_uses_real_host_and_rejects_stale_etags() {
    let root = tempfile::tempdir().unwrap();
    let root_path = std::fs::canonicalize(root.path()).unwrap();
    let config_dir = root_path.join("config");
    let temp_dir = root_path.join("tmp");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::create_dir_all(&temp_dir).unwrap();
    let module = module_path!().split_once("::").unwrap().1;
    let mut child = std::process::Command::new(std::env::current_exe().unwrap());
    child
        .args([
            "--exact",
            &format!("{module}::isolated_cli_mcp_staging_child"),
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .current_dir(&root_path)
        .env_clear();
    for name in ["HOME", "PATH", "TERM", "LANG", "LC_ALL", "RUST_MIN_STACK"] {
        if let Some(value) = std::env::var_os(name) {
            child.env(name, value);
        }
    }
    child
        .env(CHILD_ROOT, &root_path)
        .env(branding::CONFIG_DIR_ENV, &config_dir)
        .env("TMPDIR", &temp_dir);
    let result = child.output().unwrap();
    std::fs::write(root_path.join("child.stdout.log"), &result.stdout).unwrap();
    std::fs::write(root_path.join("child.stderr.log"), &result.stderr).unwrap();
    if let Some(directory) = std::env::var_os(RECEIPTS) {
        export_receipts(&root_path, Path::new(&directory));
    }
    assert!(
        result.status.success(),
        "isolated CLI consumer failed: {}\nstdout:\n{}\nstderr:\n{}",
        result.status,
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr),
    );
    // The report is emitted only after the public runner completed its real
    // shutdown and output-delivery barriers, not after an isolated path check.
    let report: Value =
        serde_json::from_slice(&std::fs::read(root_path.join("acceptance.json")).unwrap()).unwrap();
    assert_eq!(report["runnerExitCode"], 0);
    assert_eq!(report["inputExact"], true);
    assert_eq!(report["outputExact"], true);
    assert_eq!(report["staleEtagRejected"], true);
    assert_eq!(report["outsideCanaryUnchanged"], true);
    println!("CLI MCP consumer acceptance: {report}");
}

fn etag(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Explicit cfg(test)-only fake-data export. Preserve partial wire receipts on
/// failure as well as successful acceptance before the temporary root drops.
fn export_receipts(root: &Path, destination: &Path) {
    std::fs::create_dir_all(destination).unwrap();
    let mut files = Vec::new();
    for name in [
        "acceptance.json",
        "stdin.jsonl",
        "stdout.jsonl",
        "stderr.log",
        "peer.stdin.jsonl",
        "peer.py",
        "mcp-config.json",
        "child.stdout.log",
        "child.stderr.log",
    ] {
        let source = root.join(name);
        if !source.is_file() {
            continue;
        }
        let bytes = std::fs::read(&source).unwrap();
        std::fs::write(destination.join(name), &bytes).unwrap();
        files.push(json!({"name":name,"bytes":bytes.len(),"sha256":etag(&bytes)}));
    }
    std::fs::write(
        destination.join("receipt-manifest.json"),
        serde_json::to_vec_pretty(&json!({"fakeDataOnly":true,"files":files})).unwrap(),
    )
    .unwrap();
}

#[derive(Debug)]
struct ControlCapture {
    frames: Vec<Value>,
    stdin: std::fs::File,
    stdout: std::fs::File,
}
impl ControlCapture {
    fn new(root: &Path) -> Self {
        Self {
            frames: Vec::new(),
            stdin: std::fs::File::create(root.join("stdin.jsonl")).unwrap(),
            stdout: std::fs::File::create(root.join("stdout.jsonl")).unwrap(),
        }
    }
}

async fn control(
    input: &mut DuplexStream,
    output: &mut BufReader<DuplexStream>,
    id: &str,
    request: Value,
    captured: &mut ControlCapture,
) -> Value {
    let line = json!({"type":"control_request","request_id":id,"request":request}).to_string();
    captured.stdin.write_all(line.as_bytes()).unwrap();
    captured.stdin.write_all(b"\n").unwrap();
    captured.stdin.flush().unwrap();
    input.write_all(line.as_bytes()).await.unwrap();
    input.write_all(b"\n").await.unwrap();
    input.flush().await.unwrap();
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let mut line = String::new();
            assert_ne!(
                output.read_line(&mut line).await.unwrap(),
                0,
                "headless output closed before {id}"
            );
            captured.stdout.write_all(line.as_bytes()).unwrap();
            captured.stdout.flush().unwrap();
            let frame: Value = serde_json::from_str(line.trim_end()).unwrap();
            let matched =
                frame["type"] == "control_response" && frame["response"]["request_id"] == id;
            captured.frames.push(frame.clone());
            if matched {
                return frame["response"].clone();
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("headless control {id} timed out; captured={captured:?}"))
}

#[tokio::test]
#[ignore = "launched by isolated consumer supervisor with disposable config/temp authority"]
async fn isolated_cli_mcp_staging_child() {
    // An explicit fixture root is required before any real host construction.
    // Running this ignored helper directly must never fall back to user config.
    let root = PathBuf::from(std::env::var_os(CHILD_ROOT).expect("run via consumer supervisor"));
    let original_home = std::env::var_os("HOME");
    let project = root.join("project");
    let home = root.join("config");
    std::fs::create_dir_all(&project).unwrap();
    assert_eq!(
        std::env::var_os(branding::CONFIG_DIR_ENV),
        Some(home.clone().into_os_string())
    );
    let source = project.join("source.txt");
    let output = project.join("result.txt");
    let outside = root.join("outside-canary.txt");
    let canary = b"OUTSIDE_PROJECT_MUST_NOT_CHANGE";
    let old_bytes = b"old CLI output row";
    let expected_output = [INPUT_BYTES, OUTPUT_SUFFIX].concat();
    std::fs::write(&source, INPUT_BYTES).unwrap();
    std::fs::write(&output, old_bytes).unwrap();
    std::fs::write(&outside, canary).unwrap();
    std::os::unix::fs::symlink(&outside, project.join("escape.txt")).unwrap();

    let peer = root.join("peer.py");
    let peer_log = root.join("peer.stdin.jsonl");
    std::fs::write(&peer, r#"
import json,sys
from pathlib import Path
expected=bytes.fromhex(sys.argv[2]);suffix=bytes.fromhex(sys.argv[3])
for line in sys.stdin:
 with open(sys.argv[1],'a') as capture:capture.write(line)
 frame=json.loads(line)
 if 'id' not in frame:continue
 method=frame.get('method');params=frame.get('params',{})
 if method=='initialize':result={'protocolVersion':params['protocolVersion'],'capabilities':{'tools':{'listChanged':True}},'serverInfo':{'name':'actual-cli-consumer-peer','version':'1'}}
 elif method=='tools/list':result={'tools':[{'name':'files','inputSchema':{'type':'object','properties':{'input':{'type':'string'},'out':{'type':'string'}},'required':['input','out']}}]}
 elif method=='tools/call':
  assert params['name']=='files'
  args=params['arguments'];data=Path(args['input']).read_bytes()
  assert data==expected,'the real host must stage the exact input bytes'
  Path(args['out']).write_bytes(data+suffix)
  result={'content':[{'type':'text','text':'peer completed'}],'structuredContent':{'ok':True}}
 elif method=='ping':result={}
 else:
  print(json.dumps({'jsonrpc':'2.0','id':frame['id'],'error':{'code':-32601,'message':'Method not found'}},separators=(',',':')),flush=True);continue
 print(json.dumps({'jsonrpc':'2.0','id':frame['id'],'result':result},separators=(',',':')),flush=True)
"#).unwrap();
    let hex = |bytes: &[u8]| {
        bytes
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    };
    let entries = vec![json!({"mcpServers":{"operator":{"command":"/usr/bin/python3","args":[peer,peer_log,hex(INPUT_BYTES),hex(OUTPUT_SUFFIX)]}}}).to_string()];
    std::fs::write(root.join("mcp-config.json"), &entries[0]).unwrap();
    let configs = crate::init::parse_cli_mcp_servers(Some(&entries));
    assert_eq!(configs.len(), 1);
    assert_eq!(configs[0].scope, mcp::ConfigScope::Dynamic);
    assert!(
        configs[0].metadata.cli_owned,
        "use the actual CLI config intake"
    );

    let background_forker = Arc::new(crate::bg_session_forker::CliBgSessionForker::new(
        home.clone(),
        home.clone(),
        crate::background_launch::BackgroundLaunchOptions::default(),
        permission::PermissionMode::Default,
    ));
    let services = Arc::new(CliHeadlessServices {
        background_forker: background_forker.clone(),
        auto_connect_ide: false,
    });
    let mut desktop = DesktopConfig::default();
    desktop.cwd = project.clone();
    desktop.lingxi_home = home;
    desktop.isolated_credential_storage = true;
    desktop.setting_source_scope = (false, false);
    desktop.strict_mcp_config = true;
    desktop.composition = Some(DesktopSessionComposition::HeadlessCli);
    desktop.cli_mcp_servers = configs;
    desktop.bg_session_forker = Some(background_forker);

    let (mut input, runner_input) = tokio::io::duplex(64 * 1024);
    let (runner_output, output_reader) = tokio::io::duplex(256 * 1024);
    let runner_errors = tokio::fs::File::create(root.join("stderr.log"))
        .await
        .unwrap();
    let (_signal_tx, signals) = HeadlessSignals::channel();
    // This public production entry is the same one called by headless_host::run.
    // Only the existing DesktopConfig isolated-store test seam is selected;
    // the linked Harness dependency itself is compiled without cfg(test).
    let run = tokio::spawn(harness_runtime::headless::run(
        HeadlessConfig {
            desktop,
            options: HeadlessOptions {
                input_format: InputFormat::StreamJson,
                output_format: OutputFormat::StreamJson,
                verbose: true,
                ..Default::default()
            },
            session_start: SessionStart::New,
        },
        services,
        HeadlessIo::new(runner_input, runner_output, runner_errors),
        signals,
    ));
    let mut stdout = BufReader::new(output_reader);
    let mut captured = ControlCapture::new(&root);
    let ready = control(
        &mut input,
        &mut stdout,
        "ready",
        json!({"subtype":"initialize"}),
        &mut captured,
    )
    .await;
    assert_eq!(ready["subtype"], "success");
    let empty = control(
        &mut input,
        &mut stdout,
        "empty",
        json!({"subtype":"mcp_set_servers","servers":{}}),
        &mut captured,
    )
    .await;
    assert_eq!(
        empty["response"],
        json!({"added":[],"removed":[],"errors":{}})
    );
    let status = control(
        &mut input,
        &mut stdout,
        "status",
        json!({"subtype":"mcp_status"}),
        &mut captured,
    )
    .await;
    assert_eq!(status["response"]["mcpServers"][0]["name"], "operator");
    assert_eq!(status["response"]["mcpServers"][0]["status"], "connected");
    assert_eq!(status["response"]["mcpServers"][0]["source"], "dynamic");

    let request = json!({"subtype":"mcp_call","tool":"mcp__operator__files","arguments":{"input":"{{in:source}}","out":"{{out:result}}"},"input_files":[{"name":"source","lane_path":"/working/source.txt"}],"output_files":[{"name":"result","lane_path":"/working/result.txt","if_match":etag(old_bytes)}],"expires_at":"2099-01-01T00:00:00Z","timeout_ms":5000});
    let staged = control(
        &mut input,
        &mut stdout,
        "staged",
        request.clone(),
        &mut captured,
    )
    .await;
    assert_eq!(staged["subtype"], "success");
    assert_eq!(staged["response"]["staging"]["ok"], true, "{staged}");
    assert_eq!(
        staged["response"]["staging"]["outputs"],
        json!([{"lane_path":"/working/result.txt","etag":etag(&expected_output),"bytes":expected_output.len()}])
    );
    assert_eq!(
        staged["response"]["staging"]["inputs_used"],
        json!([{"lane_path":"/working/source.txt","etag":etag(INPUT_BYTES)}])
    );
    assert_eq!(std::fs::read(&source).unwrap(), INPUT_BYTES);
    assert_eq!(std::fs::read(&output).unwrap(), expected_output);
    let stale = control(
        &mut input,
        &mut stdout,
        "stale",
        request.clone(),
        &mut captured,
    )
    .await;
    assert_eq!(
        stale["response"]["staging"]["error_code"], "output_conflict",
        "{stale}"
    );
    assert_eq!(std::fs::read(&output).unwrap(), expected_output);

    let mut traversal = request.clone();
    traversal["input_files"][0]["lane_path"] = json!("/working/../outside-canary.txt");
    let refused = control(
        &mut input,
        &mut stdout,
        "traversal",
        traversal,
        &mut captured,
    )
    .await;
    assert_eq!(refused["response"]["staging"]["ok"], false, "{refused}");
    let mut symlink = request;
    symlink["output_files"][0]["lane_path"] = json!("/working/escape.txt");
    symlink["output_files"][0]["if_match"] = json!(etag(canary));
    let refused = control(&mut input, &mut stdout, "symlink", symlink, &mut captured).await;
    assert_eq!(refused["response"]["staging"]["ok"], false, "{refused}");
    assert_eq!(std::fs::read(&outside).unwrap(), canary);
    assert_eq!(
        std::fs::read_link(project.join("escape.txt")).unwrap(),
        outside
    );
    assert_eq!(std::fs::read(&output).unwrap(), expected_output);
    input.shutdown().await.unwrap();
    drop(input);
    let exit = tokio::time::timeout(Duration::from_secs(20), run)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(exit.code, 0, "{exit:?}");
    assert_eq!(exit.shutdown, HeadlessShutdownStatus::Complete, "{exit:?}");
    assert_eq!(exit.delivery, HeadlessDeliveryStatus::Complete, "{exit:?}");
    let mut tail = String::new();
    tokio::time::timeout(Duration::from_secs(5), stdout.read_to_string(&mut tail))
        .await
        .unwrap()
        .unwrap();
    captured.stdout.write_all(tail.as_bytes()).unwrap();
    captured.stdout.flush().unwrap();
    for line in tail.lines() {
        captured.frames.push(serde_json::from_str(line).unwrap());
    }
    assert_eq!(
        std::env::var_os("HOME"),
        original_home,
        "the test never repurposes HOME"
    );

    let frames = std::fs::read_to_string(&peer_log)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    let calls = frames
        .iter()
        .filter(|frame| frame["method"] == "tools/call")
        .collect::<Vec<_>>();
    assert_eq!(calls.len(),3,"success, stale publication and symlink publication execute; traversal is refused before the peer");
    for call in calls {
        let args = &call["params"]["arguments"];
        for field in ["input", "out"] {
            let path = PathBuf::from(args[field].as_str().unwrap());
            assert!(
                path.starts_with(root.join("tmp")),
                "the peer sees scratch paths, never /working or a direct project path: {path:?}"
            );
            assert!(
                !path.exists(),
                "scratch files are removed after the call: {path:?}"
            );
        }
    }
    let sequence = frames
        .iter()
        .filter(|frame| {
            matches!(
                frame["method"].as_str(),
                Some("initialize" | "tools/list" | "tools/call")
            )
        })
        .map(|frame| json!([frame["method"], frame["id"]]))
        .collect::<Vec<_>>();
    assert_eq!(
        sequence,
        json!([
            ["initialize", 0],
            ["tools/list", 1],
            ["tools/call", 2],
            ["tools/call", 3],
            ["tools/call", 4]
        ])
        .as_array()
        .unwrap()
        .clone()
    );
    std::fs::write(root.join("acceptance.json"),serde_json::to_vec_pretty(&json!({"runnerExitCode":exit.code,"inputExact":true,"outputExact":true,"staleEtagRejected":true,"outsideCanaryUnchanged":true,"shutdown":"complete","delivery":"complete","stdioRequestSequence":sequence,"staged":staged,"stale":stale})).unwrap()).unwrap();
}
