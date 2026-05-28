//! `/doctor` check runners. M5-11 ships 6 checks (T2 step 6 + T12).
//!
//! Each check returns a [`DoctorCheck`] with a `Pass`/`Warn`/`Fail` status.
//! The aggregate [`DoctorReport`] is what the `/doctor` slash-command
//! handler renders into the locked 6-row + Summary text panel.

use lingxi_traits::{CheckStatus, DoctorCheck, DoctorReport, DoctorSummary};
use std::path::Path;

/// Run all 6 doctor checks against the supplied config-dir root and
/// aggregate the results.
pub async fn run_all(config_dir: &Path) -> DoctorReport {
    let checks = vec![
        check_config_dir(config_dir).await,
        check_api_key(),
        check_network().await,
        check_disk_space(config_dir).await,
        check_git().await,
        check_telemetry_schema(),
    ];

    let mut summary = DoctorSummary::default();
    for c in &checks {
        match c.status {
            CheckStatus::Pass => summary.passed += 1,
            CheckStatus::Warn => summary.warnings += 1,
            CheckStatus::Fail => summary.failed += 1,
        }
    }
    DoctorReport { checks, summary }
}

async fn check_config_dir(p: &Path) -> DoctorCheck {
    let exists = tokio::fs::metadata(p).await.is_ok();
    if !exists {
        // Try to create it — many users won't have run /memory or /config yet.
        if tokio::fs::create_dir_all(p).await.is_err() {
            return DoctorCheck {
                name: "config-dir".to_string(),
                status: CheckStatus::Fail,
                detail: Some(format!(
                    "{} does not exist and cannot be created",
                    p.display()
                )),
            };
        }
    }
    let probe = p.join(".lingxi_writable_probe");
    let writable = tokio::fs::write(&probe, b"").await.is_ok();
    let _ = tokio::fs::remove_file(&probe).await;
    DoctorCheck {
        name: "config-dir".to_string(),
        status: if writable {
            CheckStatus::Pass
        } else {
            CheckStatus::Fail
        },
        detail: if writable {
            None
        } else {
            Some(format!("{} is not writable", p.display()))
        },
    }
}

fn check_api_key() -> DoctorCheck {
    let has_env = std::env::var_os("ANTHROPIC_API_KEY").is_some_and(|v| !v.is_empty());
    // M5-11 only checks the env var. A full keychain check (M2-06) is
    // tracked as a M5-14 follow-up.
    DoctorCheck {
        name: "api-key".to_string(),
        status: if has_env {
            CheckStatus::Pass
        } else {
            CheckStatus::Warn
        },
        detail: if has_env {
            None
        } else {
            Some("ANTHROPIC_API_KEY not set (sign in via /login or export the env var)".to_string())
        },
    }
}

async fn check_network() -> DoctorCheck {
    // TCP-reach probe rather than a full HTTPS request — avoids dragging an
    // HTTP transport into the doctor path. Uses std::net via spawn_blocking
    // because `lingxi-orchestrator`'s tokio feature set does not include
    // `net`. 5-second timeout.
    let res = tokio::task::spawn_blocking(|| {
        use std::net::ToSocketAddrs;
        let timeout = std::time::Duration::from_secs(5);
        let addrs: Vec<_> = match "api.anthropic.com:443".to_socket_addrs() {
            Ok(it) => it.collect(),
            Err(e) => return Err(format!("DNS failure: {e}")),
        };
        let addr = match addrs.first() {
            Some(a) => *a,
            None => return Err("DNS returned no addresses".to_string()),
        };
        match std::net::TcpStream::connect_timeout(&addr, timeout) {
            Ok(_) => Ok(()),
            Err(e) => Err(format!("connect failed: {e}")),
        }
    })
    .await;
    match res {
        Ok(Ok(())) => DoctorCheck {
            name: "network".to_string(),
            status: CheckStatus::Pass,
            detail: None,
        },
        Ok(Err(detail)) => DoctorCheck {
            name: "network".to_string(),
            status: CheckStatus::Fail,
            detail: Some(detail),
        },
        Err(e) => DoctorCheck {
            name: "network".to_string(),
            status: CheckStatus::Fail,
            detail: Some(format!("probe task panicked: {e}")),
        },
    }
}

async fn check_disk_space(p: &Path) -> DoctorCheck {
    // Simple probe: try to write a 1-byte test file. If that succeeds we
    // can't *measure* free space without an extra crate, so we report Pass
    // as a coarse Boolean. M5-14 may add a `fs2`-based size check.
    let probe = p.join(".lingxi_disk_probe");
    let _ = tokio::fs::create_dir_all(p).await;
    let writable = tokio::fs::write(&probe, b"\0").await.is_ok();
    let _ = tokio::fs::remove_file(&probe).await;
    DoctorCheck {
        name: "disk-space".to_string(),
        status: if writable {
            CheckStatus::Pass
        } else {
            CheckStatus::Warn
        },
        detail: if writable {
            None
        } else {
            Some(format!(
                "cannot write to {} (disk may be full)",
                p.display()
            ))
        },
    }
}

async fn check_git() -> DoctorCheck {
    let output = tokio::process::Command::new("git")
        .arg("--version")
        .output()
        .await;
    match output {
        Ok(o) if o.status.success() => DoctorCheck {
            name: "git".to_string(),
            status: CheckStatus::Pass,
            detail: Some(String::from_utf8_lossy(&o.stdout).trim().to_string()),
        },
        _ => DoctorCheck {
            name: "git".to_string(),
            status: CheckStatus::Warn,
            detail: Some("git not found (some features will be limited)".to_string()),
        },
    }
}

fn check_telemetry_schema() -> DoctorCheck {
    let actual = lingxi_telemetry::tengu::ALL_EVENT_NAMES.len();
    // M5-13: orchestrator block grew 15 → 17 (+2 REPL events) → 314 total.
    let expected = 314;
    DoctorCheck {
        name: "telemetry-schema".to_string(),
        status: if actual == expected {
            CheckStatus::Pass
        } else {
            CheckStatus::Fail
        },
        detail: if actual == expected {
            None
        } else {
            Some(format!(
                "ALL_EVENT_NAMES.len() = {actual}; expected {expected}"
            ))
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_telemetry_schema_passes_at_314() {
        let c = check_telemetry_schema();
        assert_eq!(c.name, "telemetry-schema");
        assert!(matches!(c.status, CheckStatus::Pass));
    }

    #[test]
    fn check_api_key_returns_pass_or_warn() {
        // We cannot safely mutate env vars in this crate (forbids unsafe).
        // Just check the call returns one of the expected statuses and the
        // correct name — both Pass and Warn are valid depending on whether
        // the test runner has ANTHROPIC_API_KEY set.
        let c = check_api_key();
        assert_eq!(c.name, "api-key");
        assert!(matches!(c.status, CheckStatus::Pass | CheckStatus::Warn));
    }

    #[tokio::test]
    async fn run_all_returns_6_checks() {
        let tmp = std::env::temp_dir().join("lingxi_diag_test");
        let report = run_all(&tmp).await;
        assert_eq!(report.checks.len(), 6, "doctor must run 6 checks");
        // Summary tallies match check count.
        let total = report.summary.passed + report.summary.warnings + report.summary.failed;
        assert_eq!(total, 6);
    }
}
