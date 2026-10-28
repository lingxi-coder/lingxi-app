#![cfg(all(
    feature = "native-media",
    not(any(target_os = "android", target_os = "ios"))
))]
use remote_control_client::isolated_codec::{CodecClient, CodecLaunch};
use std::ffi::OsString;

#[test]
#[ignore = "private subprocess entry point"]
fn codec_worker_entry() {
    remote_control_client::isolated_codec::run_worker().unwrap();
}

const ENV_KEYS: &[&str] = &[
    "HOME",
    "USERPROFILE",
    "TMPDIR",
    "TMP",
    "TEMP",
    "SystemRoot",
    "WINDIR",
    "PATH",
    "GST_PLUGIN_PATH_1_0",
    "GST_REGISTRY_1_0",
    "GST_TRACERS",
];

fn observed_environment() -> serde_json::Value {
    let mut values = serde_json::Map::new();
    for key in ENV_KEYS {
        values.insert(
            (*key).to_owned(),
            std::env::var_os(key)
                .map(|value| serde_json::Value::String(value.to_string_lossy().into_owned()))
                .unwrap_or(serde_json::Value::Null),
        );
    }
    values.insert(
        "cwd".into(),
        serde_json::Value::String(
            std::env::current_dir()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
        ),
    );
    serde_json::Value::Object(values)
}

#[test]
#[ignore = "private worker environment subprocess fixture"]
fn codec_worker_environment_entry() {
    let directory = std::env::current_dir().unwrap();
    std::fs::write(
        directory.parent().unwrap().join("worker-environment.json"),
        serde_json::to_vec(&observed_environment()).unwrap(),
    )
    .unwrap();
    remote_control_client::isolated_codec::run_worker().unwrap();
}

#[test]
#[ignore = "private tool environment subprocess fixture"]
fn codec_tool_environment_entry() {
    let root = std::path::PathBuf::from(std::env::var_os("LINGXI_TEST_ENV_ROOT").unwrap());
    std::fs::write(
        root.join("tool-environment.json"),
        serde_json::to_vec(&observed_environment()).unwrap(),
    )
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "private poisoned-parent subprocess fixture"]
async fn codec_environment_driver() {
    let root = std::path::PathBuf::from(std::env::var_os("LINGXI_TEST_ENV_ROOT").unwrap());
    let canonical_root = root.canonicalize().unwrap();
    let original = observed_environment();
    let mut launch = launch(&root);
    *launch.arguments.last_mut().unwrap() = OsString::from("codec_worker_environment_entry");
    let worker = CodecClient::spawn(launch, true, true).await.unwrap();
    let observed: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("worker-environment.json")).unwrap())
            .unwrap();
    let cwd = std::path::PathBuf::from(observed["cwd"].as_str().unwrap());
    assert_eq!(
        cwd.canonicalize().unwrap().parent(),
        Some(canonical_root.as_path())
    );
    let home = cwd.join("home");
    let temp = cwd.join("tmp");
    assert!(home.is_dir() && temp.is_dir());
    for key in ["HOME", "USERPROFILE"] {
        assert_eq!(
            std::path::Path::new(observed[key].as_str().unwrap())
                .canonicalize()
                .unwrap(),
            home.canonicalize().unwrap(),
            "{key} is not the worker's private home"
        );
        assert_ne!(observed[key], original[key], "{key} leaked from parent");
    }
    for key in ["TMPDIR", "TMP", "TEMP"] {
        assert_eq!(
            std::path::Path::new(observed[key].as_str().unwrap())
                .canonicalize()
                .unwrap(),
            temp.canonicalize().unwrap(),
            "{key} is not the worker's private temp directory"
        );
        assert_ne!(observed[key], original[key], "{key} leaked from parent");
    }
    #[cfg(windows)]
    {
        let paths = platform_windows::system_paths::system_paths().unwrap();
        assert_eq!(
            observed["SystemRoot"].as_str().unwrap(),
            paths.windows_dir.to_string_lossy(),
            "SystemRoot is not the host system root"
        );
        assert_eq!(
            observed["WINDIR"].as_str().unwrap(),
            paths.windows_dir.to_string_lossy()
        );
        assert_ne!(observed["SystemRoot"], original["SystemRoot"]);
        let host_root =
            std::path::PathBuf::from(std::env::var_os("LINGXI_TEST_EXPECTED_SYSTEM_ROOT").unwrap())
                .canonicalize()
                .unwrap();
        assert_eq!(paths.windows_dir.canonicalize().unwrap(), host_root);
        assert_eq!(
            paths.system_dir.canonicalize().unwrap(),
            host_root.join("System32").canonicalize().unwrap()
        );
        let actual: Vec<_> = std::env::split_paths(observed["PATH"].as_str().unwrap()).collect();
        assert_eq!(actual.last(), Some(&paths.system_dir));
        let poison = root.join("hostile").join("path").canonicalize().unwrap();
        assert!(!actual
            .iter()
            .any(|entry| entry.canonicalize().ok().as_ref() == Some(&poison)));
    }
    #[cfg(not(windows))]
    {
        assert!(observed["SystemRoot"].is_null());
        assert!(observed["WINDIR"].is_null());
        assert!(observed["PATH"].is_null());
    }
    assert_eq!(observed["GST_PLUGIN_PATH_1_0"], "");
    assert_ne!(observed["GST_REGISTRY_1_0"], original["GST_REGISTRY_1_0"]);
    assert!(observed["GST_TRACERS"].is_null());
    let mut packets = 0;
    for _ in 0..8 {
        packets += worker
            .encode_audio(48_000, &[0.1; 960])
            .await
            .unwrap()
            .len();
    }
    assert!(packets > 0);
    worker.shutdown().await.unwrap();
    assert!(
        !cwd.exists(),
        "private directory must be removed only after worker exit"
    );

    // A user tool still inherits the coordinator's workspace variables. This
    // is a separate child and cannot observe the codec's private env rewrite.
    let tool = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--ignored", "--exact", "codec_tool_environment_entry"])
        .output()
        .unwrap();
    assert!(
        tool.status.success(),
        "tool fixture failed: {}",
        String::from_utf8_lossy(&tool.stderr)
    );
    let observed_tool: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("tool-environment.json")).unwrap())
            .unwrap();
    for key in ENV_KEYS {
        assert_eq!(
            observed_tool[key], original[key],
            "tool lost workspace {key}"
        );
    }
    std::fs::remove_file(root.join("worker-environment.json")).unwrap();
    std::fs::remove_file(root.join("tool-environment.json")).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hostile_parent_environment_is_not_inherited_by_the_codec_worker() {
    let root = tempfile::tempdir().unwrap();
    let hostile = root.path().join("hostile");
    std::fs::create_dir(&hostile).unwrap();
    let poison_path = hostile.join("path");
    std::fs::create_dir(&poison_path).unwrap();
    let mut path_entries: Vec<_> =
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).collect();
    path_entries.insert(0, poison_path);
    let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--ignored",
            "--exact",
            "codec_environment_driver",
            "--nocapture",
        ])
        .kill_on_drop(true)
        .env("LINGXI_TEST_ENV_ROOT", root.path())
        .env("PATH", std::env::join_paths(path_entries).unwrap());
    #[cfg(windows)]
    command.env(
        "LINGXI_TEST_EXPECTED_SYSTEM_ROOT",
        std::env::var_os("SystemRoot").expect("host Windows system root"),
    );
    for key in [
        "HOME",
        "USERPROFILE",
        "TMPDIR",
        "TMP",
        "TEMP",
        "SystemRoot",
        "WINDIR",
    ] {
        let value = hostile.join(key);
        std::fs::create_dir(&value).unwrap();
        command.env(key, value);
    }
    command
        .env("GST_PLUGIN_PATH_1_0", hostile.join("plugins"))
        .env("GST_REGISTRY_1_0", hostile.join("registry"))
        .env("GST_TRACERS", "hostile-tracer");
    let output = tokio::time::timeout(std::time::Duration::from_secs(45), command.output())
        .await
        .expect("environment driver timed out")
        .unwrap();
    assert!(
        output.status.success(),
        "environment driver failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed; 0 failed;"));
}

fn launch(root: &std::path::Path) -> CodecLaunch {
    let (executable, arguments) = match std::env::var_os("LINGXI_TEST_CODEC_GIO_WRAPPER") {
        Some(wrapper) => {
            let wrapper = std::path::PathBuf::from(wrapper);
            let bridge = std::path::PathBuf::from(
                std::env::var_os("LINGXI_TEST_CODEC_BRIDGE_BINARY").expect("test bridge path"),
            );
            assert!(wrapper.is_absolute() && wrapper.is_file());
            assert!(bridge.is_absolute() && bridge.is_file());
            (wrapper, vec![bridge.into_os_string()])
        }
        None => (
            std::env::current_exe().unwrap(),
            vec![
                OsString::from("--ignored"),
                OsString::from("--exact"),
                OsString::from("codec_worker_entry"),
                // Diagnostic-only: otherwise libtest captures the child's
                // stage messages until exit, and a timed-out child emits none.
                OsString::from("--nocapture"),
            ],
        ),
    };
    CodecLaunch {
        executable,
        arguments,
        plugin_directory: lingxi_webrtc_media::configured_codec_plugin_directory().unwrap(),
        loader_directory: lingxi_webrtc_media::configured_codec_loader_directory(),
        private_root: root.to_path_buf(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_rust_codec_worker_encodes_audio_and_confirms_exit() {
    let root = tempfile::tempdir().unwrap();
    let worker = CodecClient::spawn(launch(root.path()), true, true)
        .await
        .unwrap();
    let samples: Vec<f32> = (0..960)
        .map(|i| ((i as f32 * 440.0 * std::f32::consts::TAU) / 48_000.0).sin() * 0.2)
        .collect();
    let mut count = 0;
    for _ in 0..12 {
        count += worker.encode_audio(48_000, &samples).await.unwrap().len();
    }
    assert!(count > 0, "Rust AudioProcessor produced no Opus packets");
    worker.reset_audio().await.unwrap();
    worker.shutdown().await.unwrap();
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
}

#[test]
#[ignore = "private crashing subprocess fixture"]
fn codec_worker_crash_entry() {
    use std::io::{Read, Write};
    use std::net::TcpStream;

    let mut bootstrap = [0u8; 48];
    std::io::stdin().read_exact(&mut bootstrap).unwrap();
    let connect = |offset: usize, kind: u8| {
        let port = u16::from_be_bytes(bootstrap[offset..offset + 2].try_into().unwrap());
        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream.write_all(&bootstrap[6..38]).unwrap();
        stream.write_all(&[kind]).unwrap();
        stream
    };
    let mut control = connect(0, b'C');
    let mut audio = connect(2, b'A');
    let _video = connect(4, b'V');
    loop {
        let mut command = [0u8; 1];
        control.read_exact(&mut command).unwrap();
        if command[0] == b'F' {
            break;
        }
        assert_eq!(command[0], b'P');
        control.write_all(b"P").unwrap();
        let mut timing = [0u8; 16];
        control.read_exact(&mut timing).unwrap();
    }
    // Exit only after the parent actually starts an operation, so this probes
    // loss after admission rather than a mere failure to start.
    let mut header = [0u8; 8];
    audio.read_exact(&mut header).unwrap();
    std::process::exit(17);
}

#[test]
#[ignore = "private delayed-bootstrap subprocess fixture"]
fn codec_worker_delayed_boot_entry() {
    std::thread::sleep(std::time::Duration::from_millis(800));
    remote_control_client::isolated_codec::run_worker().unwrap();
}

#[test]
#[ignore = "private startup cancellation subprocess fixture"]
fn codec_worker_hanging_start_entry() {
    use std::io::Read;

    let mut bootstrap = [0u8; 48];
    std::io::stdin().read_exact(&mut bootstrap).unwrap();
    // The parent sees this marker only after this exact child consumed its
    // bootstrap. It must reap the child before removing this directory.
    std::fs::write("startup-entered", std::process::id().to_string()).unwrap();
    loop {
        std::thread::park();
    }
}

async fn wait_for_empty_root(root: &std::path::Path) {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            if std::fs::read_dir(root).unwrap().next().is_none() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("owned child must be reaped before its private directory is removed");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn admitted_operation_crash_is_unknown_and_exact_child_is_reaped() {
    let root = tempfile::tempdir().unwrap();
    let mut launch = launch(root.path());
    *launch.arguments.last_mut().unwrap() = OsString::from("codec_worker_crash_entry");
    let worker = CodecClient::spawn(launch, true, false).await.unwrap();
    assert!(worker.encode_audio(48_000, &[0.0; 256]).await.is_err());
    assert!(
        worker.shutdown().await.is_err(),
        "crashed child cannot prove clean shutdown"
    );
    wait_for_empty_root(root.path()).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelling_started_child_retains_owner_until_wait_finishes() {
    let root = tempfile::tempdir().unwrap();
    let mut launch = launch(root.path());
    *launch.arguments.last_mut().unwrap() = OsString::from("codec_worker_hanging_start_entry");
    let spawn = tokio::spawn(async move { CodecClient::spawn(launch, true, false).await });
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let entered = std::fs::read_dir(root.path())
                .unwrap()
                .any(|entry| entry.unwrap().path().join("startup-entered").is_file());
            if entered {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("fixture child must consume its bootstrap before cancellation");
    spawn.abort();
    assert!(matches!(spawn.await, Err(error) if error.is_cancelled()));
    wait_for_empty_root(root.path()).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delayed_audio_request_retains_first_sample_timestamps() {
    use std::time::{Duration, Instant};

    let root = tempfile::tempdir().unwrap();
    let worker = CodecClient::spawn(launch(root.path()), true, false)
        .await
        .unwrap();
    // Construct the direct reference before choosing the initial sample time,
    // as the original in-process host does. The worker must also retain this
    // first chunk even though its encoder is constructed after the delay.
    let mut direct = lingxi_webrtc_media::AudioProcessor::new(48_000, 48_000).unwrap();
    let first_sample_at = Instant::now();
    let samples: Vec<f32> = (0..960)
        .map(|i| ((i as f32 * 440.0 * std::f32::consts::TAU) / 48_000.0).sin() * 0.2)
        .collect();
    let mut expected = Vec::new();
    for packet_index in 0..4 {
        let packet_at = first_sample_at + Duration::from_millis(packet_index * 20);
        for (chunk_index, chunk) in samples.chunks(256).enumerate() {
            let ns = (chunk_index as u64 * 256 * 1_000_000_000) / 48_000;
            expected.extend(
                direct
                    .capture(chunk, packet_at + Duration::from_nanos(ns))
                    .unwrap(),
            );
        }
    }
    assert!(
        !expected.is_empty(),
        "direct encoder produced no reference packets"
    );
    // Preserve the host timestamp while delaying admission to the real IPC
    // request. The old implementation timestamps at worker processing time.
    tokio::time::sleep(Duration::from_millis(250)).await;
    let mut actual = Vec::new();
    for packet_index in 0..4 {
        let packet_at = first_sample_at + Duration::from_millis(packet_index * 20);
        actual.extend(
            worker
                .encode_audio_at(48_000, &samples, packet_at)
                .await
                .unwrap(),
        );
    }
    assert_eq!(
        actual.len(),
        expected.len(),
        "the delayed first capture must not be discarded"
    );
    for (actual, expected) in actual.iter().zip(&expected) {
        let error = if actual.at >= expected.at {
            actual.at.duration_since(expected.at)
        } else {
            expected.at.duration_since(actual.at)
        };
        assert!(
            error < Duration::from_millis(5),
            "queued audio timestamp moved by {error:?}"
        );
    }
    worker.shutdown().await.unwrap();
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn slow_boot_cannot_hide_a_stale_audio_request() {
    use std::time::{Duration, Instant};

    let root = tempfile::tempdir().unwrap();
    let mut launch = launch(root.path());
    *launch.arguments.last_mut().unwrap() = OsString::from("codec_worker_delayed_boot_entry");
    let worker = CodecClient::spawn(launch, true, false).await.unwrap();
    assert!(worker.clock_uncertainty() <= Duration::from_millis(50));
    let first_sample_at = Instant::now();
    tokio::time::sleep(Duration::from_millis(700)).await;
    let result = worker
        .encode_audio_at(48_000, &[0.1; 1920], first_sample_at)
        .await;
    assert!(
        result.is_err(),
        "slow bootstrap must not hide stale capture age"
    );
    worker.shutdown().await.unwrap();
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
}

#[cfg(feature = "static-media")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn static_plugins_start_without_a_dynamic_plugin_directory() {
    assert!(lingxi_webrtc_media::codec_uses_static_plugins());
    assert!(lingxi_webrtc_media::configured_codec_plugin_directory()
        .unwrap()
        .is_none());
    let root = tempfile::tempdir().unwrap();
    let mut launch = launch(root.path());
    launch.plugin_directory = None;
    let worker = CodecClient::spawn(launch, true, false).await.unwrap();
    let mut packets = 0;
    for _ in 0..12 {
        packets += worker
            .encode_audio(48_000, &[0.1; 960])
            .await
            .unwrap()
            .len();
    }
    assert!(packets > 0);
    worker.shutdown().await.unwrap();
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
}
