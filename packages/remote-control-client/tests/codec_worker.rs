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
                // Diagnostic-only: otherwise libtest captures the child's
                // stage messages until exit, and a timed-out child emits none.
                OsString::from("--nocapture"),
                OsString::from("codec_worker_entry"),
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

// Protocol fixture used to control an admitted request and independent exit
// stages. It uses the real client bootstrap, authentication and clock handshake.
fn run_shutdown_fixture(mode: &str) {
    use std::io::{Read, Write};
    use std::net::{Shutdown, TcpStream};
    use std::time::{Duration, Instant};

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
    let mut video = connect(4, b'V');
    for _ in 0..5 {
        let mut command = [0u8; 1];
        control.read_exact(&mut command).unwrap();
        assert_eq!(command, *b"P");
        control.write_all(b"P").unwrap();
        let mut timing = [0u8; 16];
        control.read_exact(&mut timing).unwrap();
    }
    let mut finish = [0u8; 1];
    control.read_exact(&mut finish).unwrap();
    assert_eq!(finish, *b"F");

    let directory = std::env::current_dir().unwrap();
    let control_directory = directory.clone();
    let control_thread = std::thread::spawn(move || {
        let mut command = [0u8; 1];
        control.read_exact(&mut command).unwrap();
        assert_eq!(command, *b"Q");
        std::fs::write(control_directory.join("control-q"), b"received").unwrap();
        control
    });
    let video_thread = std::thread::spawn(move || {
        let mut byte = [0u8; 1];
        assert_eq!(video.read(&mut byte).unwrap(), 0, "video must receive FIN");
    });

    if mode == "admitted" || mode == "stuck" {
        let mut lengths = [0u8; 8];
        audio.read_exact(&mut lengths).unwrap();
        let h = u32::from_be_bytes(lengths[..4].try_into().unwrap()) as usize;
        let p = u32::from_be_bytes(lengths[4..].try_into().unwrap()) as usize;
        assert!(h <= 1024 && p == 0);
        let mut header = vec![0; h];
        audio.read_exact(&mut header).unwrap();
        let request: serde_json::Value = serde_json::from_slice(&header).unwrap();
        assert_eq!(request["op"], "audio_reset");
        std::fs::write(directory.join("request-admitted"), b"received").unwrap();
        if mode == "stuck" {
            loop {
                std::thread::park();
            }
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        while !directory.join("release-request").is_file() {
            assert!(
                Instant::now() < deadline,
                "request fixture was not released"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        let reply = br#"{"kind":"empty"}"#;
        audio
            .write_all(&(reply.len() as u32).to_be_bytes())
            .unwrap();
        audio.write_all(&0u32.to_be_bytes()).unwrap();
        audio.write_all(reply).unwrap();
        audio.flush().unwrap();
    }
    let mut byte = [0u8; 1];
    assert_eq!(audio.read(&mut byte).unwrap(), 0, "audio must receive FIN");
    video_thread.join().unwrap();
    let mut control = control_thread.join().unwrap();
    if mode != "no_receipt" {
        control.write_all(b"D\0\0\0\0\0\0\0\0").unwrap();
        control.flush().unwrap();
        std::fs::write(directory.join("receipt-sent"), b"sent").unwrap();
    }
    if mode != "admitted" {
        loop {
            std::thread::park();
        }
    }
    control.shutdown(Shutdown::Write).unwrap();
}

#[test]
#[ignore = "private ordered-stop subprocess fixture"]
fn codec_worker_ordered_shutdown_entry() {
    run_shutdown_fixture("admitted");
}

#[test]
#[ignore = "private blocked-request subprocess fixture"]
fn codec_worker_blocked_request_entry() {
    run_shutdown_fixture("stuck");
}

#[test]
#[ignore = "private missing-receipt subprocess fixture"]
fn codec_worker_missing_receipt_entry() {
    run_shutdown_fixture("no_receipt");
}

#[test]
#[ignore = "private receipt-without-exit subprocess fixture"]
fn codec_worker_receipt_without_exit_entry() {
    run_shutdown_fixture("no_exit");
}

async fn wait_for_worker_marker(root: &std::path::Path, marker: &str) -> std::path::PathBuf {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            for entry in std::fs::read_dir(root).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() && path.join(marker).is_file() {
                    return path;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("worker fixture did not reach its expected stage")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_drains_admitted_request_before_data_fin_and_control_q() {
    let root = tempfile::tempdir().unwrap();
    let mut launch = launch(root.path());
    *launch.arguments.last_mut().unwrap() = OsString::from("codec_worker_ordered_shutdown_entry");
    let worker = CodecClient::spawn(launch, true, false).await.unwrap();
    let requester = worker.clone();
    let request = tokio::spawn(async move { requester.reset_audio().await });
    let directory = wait_for_worker_marker(root.path(), "request-admitted").await;

    // A second request is already waiting on the occupied lane lock when the
    // shutdown fence is raised. It must later leave without writing a frame.
    let queued = worker.reset_audio();
    tokio::pin!(queued);
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(100), &mut queued)
            .await
            .is_err()
    );
    // Poll shutdown until it sets the admission fence and waits. The fixture
    // cannot receive Q while its admitted request has not been released.
    let shutdown = worker.shutdown();
    tokio::pin!(shutdown);
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(250), &mut shutdown)
            .await
            .is_err()
    );
    assert!(
        !directory.join("control-q").exists(),
        "Q was delivered before the admitted request completed"
    );
    let after_fence =
        tokio::time::timeout(std::time::Duration::from_millis(250), worker.reset_audio())
            .await
            .expect("new request must not wait behind the admitted request");
    assert!(
        after_fence.is_err(),
        "new request crossed the shutdown fence"
    );
    std::fs::write(directory.join("release-request"), b"continue").unwrap();
    request.await.unwrap().unwrap();
    assert!(
        queued.await.is_err(),
        "queued request crossed the shutdown fence"
    );
    shutdown.await.unwrap();
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stuck_admitted_request_makes_bounded_shutdown_unverified() {
    let root = tempfile::tempdir().unwrap();
    let mut launch = launch(root.path());
    *launch.arguments.last_mut().unwrap() = OsString::from("codec_worker_blocked_request_entry");
    let worker = CodecClient::spawn(launch, true, false).await.unwrap();
    let requester = worker.clone();
    let request = tokio::spawn(async move { requester.reset_audio().await });
    let _directory = wait_for_worker_marker(root.path(), "request-admitted").await;
    let outcome = tokio::time::timeout(std::time::Duration::from_secs(7), worker.shutdown())
        .await
        .expect("shutdown must be bounded even while the lane lock is held");
    assert!(
        outcome.is_err(),
        "stuck request must leave shutdown unverified"
    );
    assert!(
        request.await.unwrap().is_err(),
        "stuck request cannot report success"
    );
    wait_for_empty_root(root.path()).await;
}

async fn assert_fin_or_receipt_alone_does_not_confirm_exit(entry: &str, marker: &str) {
    let root = tempfile::tempdir().unwrap();
    let mut launch = launch(root.path());
    *launch.arguments.last_mut().unwrap() = OsString::from(entry);
    let worker = CodecClient::spawn(launch, true, false).await.unwrap();
    let owned = worker.clone();
    let shutdown = tokio::spawn(async move { owned.shutdown().await });
    let _directory = wait_for_worker_marker(root.path(), marker).await;
    assert!(
        shutdown.await.unwrap().is_err(),
        "a FIN or D receipt without child exit must remain unverified"
    );
    wait_for_empty_root(root.path()).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn data_fin_without_worker_receipt_is_unverified() {
    assert_fin_or_receipt_alone_does_not_confirm_exit(
        "codec_worker_missing_receipt_entry",
        "control-q",
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn worker_receipt_without_actual_exit_is_unverified() {
    assert_fin_or_receipt_alone_does_not_confirm_exit(
        "codec_worker_receipt_without_exit_entry",
        "receipt-sent",
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_worker_partial_frame_never_sends_clean_receipt() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::process::Command;

    tokio::time::timeout(std::time::Duration::from_secs(20), async {
        let root = tempfile::tempdir().unwrap();
        let control_listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let audio_listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let video_listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let token = [0x6du8; 32];
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--ignored", "--exact", "--nocapture", "codec_worker_entry"])
            .current_dir(root.path())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut bootstrap = Vec::new();
        for listener in [&control_listener, &audio_listener, &video_listener] {
            bootstrap.extend_from_slice(&listener.local_addr().unwrap().port().to_be_bytes());
        }
        bootstrap.extend_from_slice(&token);
        bootstrap.extend_from_slice(b"LXCODEC1");
        bootstrap.extend_from_slice(&[0, 0]);
        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(&bootstrap).await.unwrap();
        stdin.shutdown().await.unwrap();
        let (mut control, _) = control_listener.accept().await.unwrap();
        let (mut audio, _) = audio_listener.accept().await.unwrap();
        let (mut video, _) = video_listener.accept().await.unwrap();
        for (stream, lane) in [(&mut control, b'C'), (&mut audio, b'A'), (&mut video, b'V')] {
            let mut proof = [0u8; 33];
            stream.read_exact(&mut proof).await.unwrap();
            assert_eq!(&proof[..32], &token);
            assert_eq!(proof[32], lane);
        }
        for _ in 0..5 {
            control.write_all(b"P").await.unwrap();
            let mut response = [0u8; 1];
            control.read_exact(&mut response).await.unwrap();
            assert_eq!(response, *b"P");
            control.write_all(&[0u8; 16]).await.unwrap();
        }
        control.write_all(b"F").await.unwrap();
        // Deliberately cut off the first frame's 8-byte length header.
        audio.write_all(&[0, 0, 0]).await.unwrap();
        audio.shutdown().await.unwrap();
        video.shutdown().await.unwrap();
        control.write_all(b"Q").await.unwrap();
        control.shutdown().await.unwrap();
        let mut events = Vec::new();
        control.read_to_end(&mut events).await.unwrap();
        let status = child.wait().await.unwrap();
        assert!(
            events.is_empty(),
            "partial frame must not receive graceful D"
        );
        assert!(
            !status.success(),
            "partial frame worker must report failure"
        );
    })
    .await
    .expect("partial-frame worker fixture exceeded its deadline");
}
