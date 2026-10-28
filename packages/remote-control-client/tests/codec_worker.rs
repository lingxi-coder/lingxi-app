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
