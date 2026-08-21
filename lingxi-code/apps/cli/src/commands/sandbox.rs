//! `lingxi-cli sandbox install|status` — the hidden Windows-sandbox family.
//!
//! (CLI-10, cc 2.1.238) NEW in 2.1.238: a diff of every `.command("…")` name
//! between 2.1.220 and 2.1.238 yields exactly two new names, `sandbox` and its
//! child `install`. Registration (cc-238.js @~245141000):
//!
//! ```js
//! let c = r.command("sandbox", {hidden:!0});
//! c.command("install").description('Install the Windows sandbox user and network filters. Self-elevates (one UAC prompt). Prints a JSON {status, message} result and exits 0 only when status is "ok".')
//! c.command("status").description("Print Windows sandbox availability and install state as JSON {available, installed, policyLocked, reasons}.")
//! ```
//!
//! Both handlers open on the same gate (`RUy` @308569025):
//!
//! ```js
//! function RUy(){
//!   if(Wt()!=="windows") return "Windows sandbox install is only available on native Windows.";
//!   if(!I1e())          return "The Windows sandbox is not enabled on this build.";
//!   if(!fi.isPlatformInEnabledList()) return "Sandboxing is disabled for this platform by the enabledPlatforms policy setting.";
//!   return null}
//! ```
//!
//! and then:
//!
//! ```js
//! async function hQ0(){ let e=RUy(), t = e ? {status:"unavailable",message:e} : … ;
//!   await mJ(JSON.stringify(t)+"\n"); return exit(t.status==="ok"?0:1)}
//! async function gQ0(){ let e=RUy(), t;
//!   if(e) t={available:!1,installed:!1,policyLocked:!1,reasons:[e]}; else {…}
//!   await mJ(JSON.stringify(t)+"\n"); return exit(0)}
//! ```
//!
//! lingxi-cli ships no Windows sandbox user/network-filter installer, so BOTH
//! oracle branches that this port can ever reach are the *gate* branches, and
//! both are reproducible byte-exactly:
//!
//! * off Windows → the first gate string (identical to the oracle);
//! * on Windows → the second gate string, because the Windows sandbox is
//!   genuinely not enabled on this build.
//!
//! Nothing is faked: the port never claims `status:"ok"`, never claims
//! `available:true`, and never self-elevates.

use clap::{Args, Subcommand};

/// The gate reason (`RUy`), or `None` when a real Windows sandbox is available
/// — a state lingxi-cli never reaches, since it ships no Windows sandbox.
fn unavailable_reason() -> Option<&'static str> {
    if !cfg!(windows) {
        return Some("Windows sandbox install is only available on native Windows.");
    }
    // `I1e()` — "is the Windows sandbox enabled on this build". lingxi-cli has
    // no Windows sandbox user/network-filter provisioning at all, so this arm
    // is the honest answer on Windows rather than the policy arm below it.
    Some("The Windows sandbox is not enabled on this build.")
}

/// `sandbox` args (hidden family).
#[derive(Debug, Clone, Args)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Sub,
}

/// `sandbox` children.
#[derive(Debug, Clone, Subcommand)]
pub enum Sub {
    /// Install the Windows sandbox user and network filters. Self-elevates
    /// (one UAC prompt). Prints a JSON {status, message} result and exits 0
    /// only when status is "ok".
    Install,
    /// Print Windows sandbox availability and install state as JSON
    /// {available, installed, policyLocked, reasons}.
    Status,
}

/// Run the `sandbox` family.
pub async fn run(cli: &Cli) -> i32 {
    // `preserve_order` is on for the workspace's serde_json, so `json!` keeps
    // the oracle's key order (`{status, message}` / `{available, installed,
    // policyLocked, reasons}`) rather than sorting it.
    match cli.command {
        Sub::Install => {
            let reason = unavailable_reason();
            let payload = serde_json::json!({
                "status": "unavailable",
                "message": reason.unwrap_or_default(),
            });
            println!("{payload}");
            // `exits 0 only when status is "ok"`.
            crate::exit_codes::RUNTIME_ERROR
        }
        Sub::Status => {
            let reason = unavailable_reason();
            let payload = serde_json::json!({
                "available": false,
                "installed": false,
                "policyLocked": false,
                "reasons": reason.map(|r| vec![r]).unwrap_or_default(),
            });
            println!("{payload}");
            crate::exit_codes::SUCCESS
        }
    }
}
