//! On-disk team-file helpers — 1:1 port of the path/IO surface of
//! `claude-code/src/utils/swarm/teamHelpers.ts` used by the coordinator
//! `TeamCreate` / `TeamDelete` tools.
//!
//! Layout (claude-code):
//! - team dir:  `~/.lingxi/teams/{sanitize(name)}/`
//! - team file: `~/.lingxi/teams/{sanitize(name)}/config.json`
//! - task dir:  `~/.lingxi/tasks/{sanitize(name)}/`
//!
//! NOTE: this is the claude-code *coordinator* team-file subsystem
//! (`~/.lingxi/teams/`), DISTINCT from the LingXi-internal `team-mem`
//! subsystem in `tools/team` (`~/.lingxi/team-mem/`). They do not share a
//! directory or a schema.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// `TeamFile` — 1:1 with the TS `TeamFile` type (`teamHelpers.ts:64-90`),
/// reduced to the fields the coordinator `TeamCreate` actually writes
/// (`TeamCreateTool.ts:157-175`). Unknown fields are preserved on read via
/// `serde(default)` tolerance — the coordinator only writes the lead member.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamFile {
    /// Team name (the un-sanitized display name).
    pub name: String,
    /// Optional free-form description/purpose.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub description: Option<String>,
    /// Creation timestamp (Unix milliseconds, mirroring TS `Date.now()`).
    #[serde(rename = "createdAt")]
    pub created_at: u64,
    /// Deterministic lead agent id (`team-lead@{name}` analog).
    #[serde(rename = "leadAgentId")]
    pub lead_agent_id: String,
    /// Actual session id of the leader (for team discovery).
    #[serde(
        rename = "leadSessionId",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub lead_session_id: Option<String>,
    /// Team members. The coordinator writes exactly one: the lead.
    pub members: Vec<TeamMember>,
}

/// A single team member — 1:1 with the TS member object the coordinator writes
/// (`TeamCreateTool.ts:164-173`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamMember {
    /// Member agent id.
    #[serde(rename = "agentId")]
    pub agent_id: String,
    /// Member name (the lead is `team-lead`).
    pub name: String,
    /// Agent type/role.
    #[serde(rename = "agentType", skip_serializing_if = "Option::is_none", default)]
    pub agent_type: Option<String>,
    /// Model id resolved for the member.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub model: Option<String>,
    /// Join timestamp (Unix milliseconds).
    #[serde(rename = "joinedAt")]
    pub joined_at: u64,
    /// tmux pane id (empty for in-process teammates).
    #[serde(rename = "tmuxPaneId")]
    pub tmux_pane_id: String,
    /// Working directory.
    pub cwd: String,
    /// PR-activity subscriptions (empty on creation).
    pub subscriptions: Vec<String>,
}

/// `sanitizeName` (`teamHelpers.ts:100-102`): replace every non-alphanumeric
/// char with `-` and lowercase.
#[must_use]
pub fn sanitize_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect()
}

/// Resolve the config-home root: `$LINGXI_CONFIG_DIR` when set wins (claude-code
/// `tr()` `??`: an empty value is honored verbatim → cwd-relative), else
/// `$HOME/.claude` (tests redirect `$HOME` to a tempdir). Returns `None` when
/// neither resolves.
#[must_use]
pub fn lingxi_home() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os(branding::CONFIG_DIR_ENV) {
        return Some(PathBuf::from(dir));
    }
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(branding::DOT_DIR))
}

/// `getTeamDir` (`teamHelpers.ts:115-117`): `<lingxi_home>/teams/{sanitize}`.
#[must_use]
pub fn team_dir(home: &Path, name: &str) -> PathBuf {
    home.join("teams").join(sanitize_name(name))
}

/// `getTeamFilePath` (`teamHelpers.ts:122-124`):
/// `<lingxi_home>/teams/{sanitize}/config.json`.
#[must_use]
pub fn team_file_path(home: &Path, name: &str) -> PathBuf {
    team_dir(home, name).join("config.json")
}

/// `getTasksDir` analog: `<lingxi_home>/tasks/{sanitize}`.
#[must_use]
pub fn task_dir(home: &Path, name: &str) -> PathBuf {
    home.join("tasks").join(sanitize_name(name))
}

/// `readTeamFile` existence probe (`teamHelpers.ts:131-142`): true iff a team
/// config file exists for `name`. Used by `generateUniqueTeamName`.
#[must_use]
pub fn team_file_exists(home: &Path, name: &str) -> bool {
    team_file_path(home, name).is_file()
}

/// `writeTeamFileAsync` (`teamHelpers.ts:175-182`): create the team dir and
/// write `config.json` as pretty JSON.
///
/// # Errors
/// Returns the underlying `std::io::Error` on a mkdir / write / serialize
/// failure.
pub fn write_team_file(home: &Path, name: &str, file: &TeamFile) -> std::io::Result<()> {
    let dir = team_dir(home, name);
    std::fs::create_dir_all(&dir)?;
    let json = serde_json::to_string_pretty(file)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(team_file_path(home, name), json)
}

/// Read + parse a team's `config.json` (`readTeamFile`, `teamHelpers.ts`).
///
/// # Errors
/// Returns the underlying `std::io::Error` on a read failure, or an
/// `InvalidData` error when the file is not valid `TeamFile` JSON.
pub fn read_team_file(home: &Path, name: &str) -> std::io::Result<TeamFile> {
    let content = std::fs::read_to_string(team_file_path(home, name))?;
    serde_json::from_str(&content)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

/// Remove one member from a team's `config.json`, matching by agent id OR
/// display name — the port of the oracle's `jqt(teamName, {agentId, name})`
/// (2.1.223 shutdown paths: `Urv` @261175890 and the print.ts
/// `shutdown_approved` handler @262044791, both of which remove the member
/// from the team file BEFORE unassigning its tasks).
///
/// Returns `true` when a member was removed and the file written back.
///
/// # Errors
/// Propagates read/parse/write failures from the underlying file IO.
pub fn remove_team_member(
    home: &Path,
    team_name: &str,
    agent_id: &str,
    member_name: &str,
) -> std::io::Result<bool> {
    let mut file = read_team_file(home, team_name)?;
    let before = file.members.len();
    file.members
        .retain(|m| m.agent_id != agent_id && m.name != member_name);
    if file.members.len() == before {
        return Ok(false);
    }
    write_team_file(home, team_name, &file)?;
    Ok(true)
}

/// `cleanupTeamDirectories` (`TeamDeleteTool.ts:101` → `teamHelpers.ts:641-683`),
/// reduced to the directory removal the coordinator needs: remove the team dir
/// (`~/.lingxi/teams/{name}/`) and the tasks dir (`~/.lingxi/tasks/{name}/`).
/// Worktree teardown is out of scope (in-process teammates have no worktrees).
/// Best-effort: a missing dir is not an error.
pub fn cleanup_team_directories(home: &Path, name: &str) {
    let team = team_dir(home, name);
    if team.exists() {
        let _ = std::fs::remove_dir_all(&team);
    }
    let tasks = task_dir(home, name);
    if tasks.exists() {
        let _ = std::fs::remove_dir_all(&tasks);
    }
}

/// Unix-millisecond timestamp (TS `Date.now()`).
#[must_use]
pub fn now_unix_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_matches_ts() {
        assert_eq!(sanitize_name("Alpha Team"), "alpha-team");
        assert_eq!(sanitize_name("My_Team!"), "my-team-");
        assert_eq!(sanitize_name("ABC123"), "abc123");
        assert_eq!(sanitize_name("a/b\\c"), "a-b-c");
    }

    #[test]
    fn paths_compose() {
        let home = PathBuf::from("/home/u/.lingxi");
        assert_eq!(
            team_file_path(&home, "Alpha Team"),
            PathBuf::from("/home/u/.lingxi/teams/alpha-team/config.json")
        );
        assert_eq!(
            task_dir(&home, "Alpha Team"),
            PathBuf::from("/home/u/.lingxi/tasks/alpha-team")
        );
    }

    #[test]
    fn write_then_exists_then_cleanup() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join(".lingxi");
        assert!(!team_file_exists(&home, "alpha"));

        let file = TeamFile {
            name: "alpha".into(),
            description: Some("do work".into()),
            created_at: 123,
            lead_agent_id: "team-lead@alpha".into(),
            lead_session_id: Some("sess-1".into()),
            members: vec![TeamMember {
                agent_id: "team-lead@alpha".into(),
                name: "team-lead".into(),
                agent_type: Some("team-lead".into()),
                model: Some("claude-x".into()),
                joined_at: 123,
                tmux_pane_id: String::new(),
                cwd: "/work".into(),
                subscriptions: vec![],
            }],
        };
        write_team_file(&home, "alpha", &file).unwrap();
        assert!(team_file_exists(&home, "alpha"));

        // The written JSON uses the TS field names + shape.
        let raw = std::fs::read_to_string(team_file_path(&home, "alpha")).unwrap();
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["name"], "alpha");
        assert_eq!(v["createdAt"], 123);
        assert_eq!(v["leadAgentId"], "team-lead@alpha");
        assert_eq!(v["leadSessionId"], "sess-1");
        assert_eq!(v["members"][0]["agentId"], "team-lead@alpha");
        assert_eq!(v["members"][0]["name"], "team-lead");
        assert_eq!(v["members"][0]["agentType"], "team-lead");
        assert_eq!(v["members"][0]["joinedAt"], 123);
        assert_eq!(v["members"][0]["subscriptions"], serde_json::json!([]));

        // Cleanup removes the team dir (also create the tasks dir to prove that
        // path is removed too).
        std::fs::create_dir_all(task_dir(&home, "alpha")).unwrap();
        cleanup_team_directories(&home, "alpha");
        assert!(!team_file_exists(&home, "alpha"));
        assert!(!team_dir(&home, "alpha").exists());
        assert!(!task_dir(&home, "alpha").exists());
    }
}
