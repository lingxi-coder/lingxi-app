//! Exercise the real bridge-server binary's private codec entry point.
#![cfg(not(any(target_os = "android", target_os = "ios")))]

use remote_control_client::isolated_codec::{
    configured_plugin_directory, CodecClient, CodecLaunch,
};
use std::ffi::OsString;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bridge_binary_codec_entry_encodes_and_reaps_its_worker() {
    let root = tempfile::tempdir().unwrap();
    let worker = CodecClient::spawn(
        CodecLaunch {
            executable: std::path::PathBuf::from(env!("CARGO_BIN_EXE_bridge-server")),
            arguments: vec![OsString::from("--desktop-codec-worker")],
            plugin_directory: configured_plugin_directory().unwrap(),
            private_root: root.path().to_path_buf(),
        },
        true,
        true,
    )
    .await
    .unwrap();
    let samples: Vec<f32> = (0..960)
        .map(|i| ((i as f32 * 440.0 * std::f32::consts::TAU) / 48_000.0).sin() * 0.2)
        .collect();
    let mut packets = 0;
    for _ in 0..12 {
        packets += worker.encode_audio(48_000, &samples).await.unwrap().len();
    }
    assert!(packets > 0, "bridge worker produced no Opus packets");
    worker.shutdown().await.unwrap();
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
}
