//! Explicit URL commands never become prompts or start a model turn.
use clap::Args;
use lingxi_deep_links::{build, parse, Target};
use std::collections::BTreeMap;
#[derive(Debug, Clone, Args)]
pub struct Open {
    pub url: String,
    /// Open supported local destinations in this terminal
    #[arg(long)]
    pub cli: bool,
}
#[derive(Debug, Clone, Args)]
pub struct Link {
    /// Destination, for example session/UUID or settings/general
    pub target: String,
    #[arg(long, conflicts_with = "project")]
    pub workspace: Option<String>,
    /// Existing local project; its canonical path is hashed rather than exposed
    #[arg(long)]
    pub project: Option<std::path::PathBuf>,
    #[arg(long)]
    pub message: Option<String>,
    /// Draft text for a new conversation, carried in the URL fragment
    #[arg(long)]
    pub prompt: Option<String>,
    #[arg(long, default_value = "https", value_parser = ["https", "scheme"])]
    pub format: String,
}
pub fn link(args: &Link) -> i32 {
    let mut params = BTreeMap::new();
    if let Some(workspace) = &args.workspace {
        params.insert("workspace".into(), workspace.clone());
    }
    if let Some(project) = &args.project {
        use sha2::{Digest, Sha256};
        let path = match project.canonicalize() {
            Ok(path) => path,
            Err(_) => {
                eprintln!("Project does not exist");
                return 1;
            }
        };
        params.insert(
            "workspace".into(),
            format!(
                "project.{:x}",
                Sha256::digest(path.to_string_lossy().as_bytes())
            ),
        );
    }
    if let Some(message) = &args.message {
        params.insert("message".into(), message.clone());
    }
    if args.prompt.is_some() && args.target != "new" {
        eprintln!("--prompt requires the new destination");
        return 1;
    }
    let payload = args
        .prompt
        .as_ref()
        .map(|prompt| serde_json::json!({ "prompt": prompt }));
    match build(
        &Target {
            route: args.target.clone(),
            params,
            payload,
        },
        args.format == "scheme",
    ) {
        Ok(url) => {
            println!("{url}");
            0
        }
        Err(error) => {
            eprintln!("{error}");
            1
        }
    }
}
pub fn open(args: &Open) -> i32 {
    let target = match parse(&args.url) {
        Ok(t) => t,
        Err(error) => {
            eprintln!("{error}");
            return 1;
        }
    };
    if let Some(config_dir) = std::env::var_os("LINGXI_CONFIG_DIR") {
        let selected = std::path::PathBuf::from(config_dir);
        let default =
            std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".lingxi");
        let same_store = selected == default
            || selected
                .canonicalize()
                .ok()
                .zip(default.canonicalize().ok())
                .is_some_and(|(left, right)| left == right);
        if !same_store
            && (target.route.starts_with("session/")
                || target.route.starts_with("project/")
                || target.params.contains_key("workspace"))
        {
            eprintln!("Desktop cannot verify this custom LINGXI_CONFIG_DIR; use --cli for this local destination");
            return 1;
        }
    }
    let url = match build(&target, true) {
        Ok(u) => u,
        Err(error) => {
            eprintln!("{error}");
            return 1;
        }
    };
    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("open").arg(&url).status();
    #[cfg(target_os = "linux")]
    let result = std::process::Command::new("xdg-open").arg(&url).status();
    #[cfg(target_os = "windows")]
    let result = std::process::Command::new("explorer.exe")
        .arg(&url)
        .status();
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    let result: std::io::Result<std::process::ExitStatus> =
        Err(std::io::Error::other("Unsupported platform"));
    match result {
        Ok(status) if status.success() => 0,
        _ => {
            eprintln!("Unable to open LingXi. Install the desktop application and register its URL protocol.");
            1
        }
    }
}

/// Resolve only within the selected local store, never across arbitrary disks.
pub fn prepare_cli(args: &Open, parsed: &mut crate::argv::Argv) -> Result<(), String> {
    use sha2::{Digest, Sha256};
    if parsed.print
        || parsed.input_format.is_some()
        || parsed.output_format.is_some()
        || parsed.continue_session
        || parsed.resume.is_some()
        || parsed.prompt.is_some()
        || parsed.resume_session_at.is_some()
        || parsed.resume_drops_turn.is_some()
        || parsed.prefill.is_some()
        || parsed.prefill_b64.is_some()
    {
        return Err(
            "--cli links cannot be combined with prompt, print, streaming, or resume options"
                .into(),
        );
    }
    let target = parse(&args.url).map_err(str::to_owned)?;
    let mut parts = target.route.split('/');
    let kind = parts.next().unwrap_or_default();
    let id = parts.next();
    if !["session", "project", "new"].contains(&kind) {
        return Err("Use `lingxi open <url>` for this desktop destination".into());
    }
    let home = std::env::var_os("LINGXI_CONFIG_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".lingxi")
        });
    let current_cwd = std::env::current_dir()
        .ok()
        .and_then(|cwd| cwd.canonicalize().ok());
    let requested = target
        .params
        .get("workspace")
        .cloned()
        .or_else(|| (kind == "project").then(|| id.unwrap_or_default().to_owned()));
    let mut roots = std::collections::BTreeSet::new();
    if let Ok(cwd) = std::env::current_dir() {
        roots.insert(cwd);
    }
    if let Ok(projects) = std::fs::read_dir(home.join("projects")) {
        for project in projects.flatten() {
            if let Ok(entries) = std::fs::read_dir(project.path()) {
                for entry in entries
                    .flatten()
                    .filter(|e| e.path().extension().is_some_and(|x| x == "jsonl"))
                {
                    use std::io::BufRead;
                    if let Ok(file) = std::fs::File::open(entry.path()) {
                        for line in std::io::BufReader::new(file).lines().take(64).flatten() {
                            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) {
                                if let Some(cwd) = value.get("cwd").and_then(|x| x.as_str()) {
                                    roots.insert(cwd.into());
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    let candidates: Vec<_> = roots
        .into_iter()
        .filter_map(|path| path.canonicalize().ok())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .filter(|path| {
            if let Some(key) = &requested {
                return *key
                    == format!(
                        "project.{:x}",
                        Sha256::digest(path.to_string_lossy().as_bytes())
                    );
            }
            if kind != "session" {
                return current_cwd.as_ref() == Some(path);
            }
            home.join("projects")
                .join(session::jsonl::project_dir_name(&path.to_string_lossy()))
                .join(format!("{}.jsonl", id.unwrap_or_default()))
                .is_file()
        })
        .collect();
    if candidates.len() != 1 {
        return Err("Project is unavailable or ambiguous in this local data directory".into());
    }
    let cwd = candidates.into_iter().next().unwrap();
    if kind == "session" {
        let id = id.ok_or("Missing session")?;
        let transcript = home
            .join("projects")
            .join(session::jsonl::project_dir_name(&cwd.to_string_lossy()))
            .join(format!("{id}.jsonl"));
        if !transcript.is_file() {
            return Err("Session is unavailable in this local project".into());
        }
        if let Some(uuid) = target.params.get("message") {
            use std::io::BufRead;
            let found = std::io::BufReader::new(
                std::fs::File::open(&transcript).map_err(|_| "Unreadable session")?,
            )
            .lines()
            .map_while(Result::ok)
            .any(|line| {
                serde_json::from_str::<serde_json::Value>(&line)
                    .ok()
                    .is_some_and(|row| {
                        row.get("uuid").and_then(|x| x.as_str()) == Some(uuid)
                            && !row
                                .get("isSidechain")
                                .and_then(|x| x.as_bool())
                                .unwrap_or(false)
                            && !row
                                .get("isVisibleInTranscriptOnly")
                                .and_then(|x| x.as_bool())
                                .unwrap_or(false)
                            && !row
                                .get("isCompactSummary")
                                .and_then(|x| x.as_bool())
                                .unwrap_or(false)
                            && (matches!(
                                row.get("type").and_then(|x| x.as_str()),
                                Some("user" | "assistant")
                            ) || row.get("subtype").and_then(|x| x.as_str())
                                == Some("compact_boundary"))
                    })
            });
            if !found {
                return Err("Message is unavailable in this session".into());
            }
        }
        parsed.resume = Some(id.to_owned());
    }
    parsed.command = None;
    parsed.cwd = Some(cwd);
    parsed.prefill = target
        .payload
        .as_ref()
        .and_then(|x| x.get("prompt"))
        .and_then(|x| x.as_str())
        .map(str::to_owned);
    parsed.deep_link_message = target.params.get("message").cloned();
    parsed.deep_link_origin = true;
    parsed.brief = false;
    Ok(())
}
