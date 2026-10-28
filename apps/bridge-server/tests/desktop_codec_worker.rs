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
            loader_directory: remote_control_client::isolated_codec::configured_loader_directory(),
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

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn first_bridge_start_creates_the_selected_codec_root() {
    use std::process::Stdio;
    use std::time::Duration;

    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let binary = std::path::PathBuf::from(env!("CARGO_BIN_EXE_bridge-server"));
    for explicit_bridge_dir in [false, true] {
        let config_home = root.path().join(if explicit_bridge_dir {
            "config-explicit"
        } else {
            "config-default"
        });
        let bridge_dir = if explicit_bridge_dir {
            root.path().join("first-bridge-dir")
        } else {
            // Discovery uses the process HOME, independently of the
            // session-specific config-home override used as the codec root.
            root.path().join(branding::DOT_DIR).join("bridge")
        };
        let codec_root = if explicit_bridge_dir {
            bridge_dir.clone()
        } else {
            config_home.clone()
        };
        assert!(!config_home.exists());
        assert!(!bridge_dir.exists());
        let mut command = tokio::process::Command::new(&binary);
        command
            .env_clear()
            .env("HOME", root.path())
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .arg("--cwd")
            .arg(&workspace)
            .arg("--session-config-home")
            .arg(&config_home)
            .arg("--session-global-config-path")
            .arg(root.path().join("absent-global.json"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        if let Some(path) = std::env::var_os("LINGXI_GSTREAMER_PLUGIN_DIR") {
            command.env("LINGXI_GSTREAMER_PLUGIN_DIR", path);
        }
        if explicit_bridge_dir {
            command.arg("--bridge-dir").arg(&bridge_dir);
        }
        let mut bridge = command.spawn().unwrap();
        tokio::time::timeout(Duration::from_secs(45), async {
            loop {
                assert!(
                    bridge.try_wait().unwrap().is_none(),
                    "Bridge exited before publishing its lockfile"
                );
                let published = std::fs::read_dir(&bridge_dir)
                    .ok()
                    .is_some_and(|mut entries| {
                        entries.any(|entry| {
                            entry
                                .unwrap()
                                .path()
                                .extension()
                                .is_some_and(|ext| ext == "lock")
                        })
                    });
                if published {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("Bridge must publish its first lockfile");
        assert!(codec_root.canonicalize().unwrap().is_dir());
        let worker = CodecClient::spawn(
            CodecLaunch {
                executable: binary.clone(),
                arguments: vec![OsString::from("--desktop-codec-worker")],
                plugin_directory: configured_plugin_directory().unwrap(),
                loader_directory:
                    remote_control_client::isolated_codec::configured_loader_directory(),
                private_root: codec_root,
            },
            true,
            false,
        )
        .await
        .unwrap();
        let mut produced = 0;
        for _ in 0..8 {
            produced += worker
                .encode_audio(48_000, &[0.1; 960])
                .await
                .unwrap()
                .len();
        }
        assert!(produced > 0);
        worker.shutdown().await.unwrap();
        let pid = bridge.id().unwrap().to_string();
        let signalled = std::process::Command::new("/bin/kill")
            .args(["-TERM", &pid])
            .status()
            .unwrap();
        assert!(signalled.success());
        let status = match tokio::time::timeout(Duration::from_secs(30), bridge.wait()).await {
            Ok(result) => result.unwrap(),
            Err(_) => {
                let _ = bridge.kill().await;
                panic!("Bridge failed to complete graceful shutdown");
            }
        };
        assert!(status.success(), "Bridge shutdown failed: {status}");
    }
}
