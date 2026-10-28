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
    CodecLaunch {
        executable: std::env::current_exe().unwrap(),
        arguments: vec![
            OsString::from("--ignored"),
            OsString::from("--exact"),
            OsString::from("codec_worker_entry"),
        ],
        plugin_directory: lingxi_webrtc_media::configured_codec_plugin_directory().unwrap(),
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
    let _control = connect(0, b'C');
    let mut audio = connect(2, b'A');
    let _video = connect(4, b'V');
    // Exit only after the parent actually starts an operation, so this probes
    // loss after admission rather than a mere failure to start.
    let mut header = [0u8; 8];
    audio.read_exact(&mut header).unwrap();
    std::process::exit(17);
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
