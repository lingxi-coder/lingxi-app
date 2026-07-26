//! Forked-skill scoping sidecars — the two files that sit beside a forked
//! skill's session transcript and let a LATER resume re-establish the exact
//! permission scoping the fork ran under.
//!
//! A skill declaring `context: fork` runs as a background subagent under the
//! skill's own `allowed-tools` / `disallowed-tools` rather than the parent's.
//! That scoping is not derivable from the transcript, so resuming such an agent
//! without it would silently widen its permissions. Claude-code writes it to
//! disk at fork time and REFUSES to resume when it cannot be corroborated —
//! this module is the read/write half of that contract; the refusals live with
//! the resume path.
//!
//! Two files, both derived from the session JSONL path (claude `qon`):
//!
//! | file | claude | content |
//! |---|---|---|
//! | `<uuid>.forked-skill.json` | `scoping` | the [`ForkedSkillScoping`] record |
//! | `<uuid>.forked-skill.marker.json` | `provenanceMarker` | `{"forkedSkill":true,"skillName":…}` |
//!
//! The marker exists so a MISSING scoping record is distinguishable from a
//! never-forked agent: an agent with a marker but no scoping is
//! [`ScopingStatus::AbsentButMarked`], which the resume path refuses, while a
//! plain agent is [`ScopingStatus::Absent`], which it allows. Without the
//! marker, deleting the scoping file would downgrade a forked skill to an
//! unscoped resume — the marker turns that deletion into a hard failure.
//!
//! Both reads use `lstat` (claude `u7e.lstat`), NOT `stat`: a symlink planted
//! at either path reports `is_file() == false` and is rejected as
//! [`ScopingStatus::Malformed`] rather than followed to whatever it points at.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Maximum on-disk size of either sidecar (claude `Udd = 524288`). A larger
/// file is rejected unread — the record is a few hundred bytes, so anything
/// approaching this is not a record we wrote.
pub const SIDECAR_MAX_BYTES: u64 = 524_288;

/// Maximum length of a skill name (claude `UCo`: `.min(1).max(256)`).
pub const SKILL_NAME_MAX_LEN: usize = 256;

/// Maximum length of one frozen command-deny rule (claude
/// `De.array(De.string().max(1024))`).
pub const FROZEN_DENY_MAX_LEN: usize = 1024;

/// Maximum number of frozen command-deny rules (claude `.max(1000)`).
pub const FROZEN_DENY_MAX_COUNT: usize = 1000;

/// Minimum / maximum numeric `effort` (claude
/// `De.number().int().min(1).max(1000)`).
pub const EFFORT_MIN: i64 = 1;
/// See [`EFFORT_MIN`].
pub const EFFORT_MAX: i64 = 1000;

/// The named effort levels (claude `VN_`). An `effort` is either one of these
/// or an integer in `EFFORT_MIN..=EFFORT_MAX`.
pub const EFFORT_LEVELS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];

/// `effort` — either a named level or an integer (claude
/// `De.union([De.enum(VN_), De.number().int().min(1).max(1000)])`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Effort {
    /// One of [`EFFORT_LEVELS`].
    Level(String),
    /// An integer in `EFFORT_MIN..=EFFORT_MAX`.
    Steps(i64),
}

impl Effort {
    /// Whether this value satisfies the union's constraints.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        match self {
            Self::Level(s) => EFFORT_LEVELS.contains(&s.as_str()),
            Self::Steps(n) => (EFFORT_MIN..=EFFORT_MAX).contains(n),
        }
    }
}

/// The scoping record written beside a forked skill's transcript (claude
/// `qCo`).
///
/// Field order is the binary's object-literal order, and `serde_json`'s
/// `preserve_order` keeps it on the wire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForkedSkillScoping {
    /// The forked skill's name — the identity every resume check corroborates.
    #[serde(rename = "skillName")]
    pub skill_name: String,
    /// The display name the fork was attributed to (claude `attributionName`,
    /// sourced from the launch's `spawnedBySkill`).
    #[serde(rename = "attributionName")]
    pub attribution_name: String,
    /// The skill's declared effort, when it declared one. Omitted otherwise —
    /// claude spreads it conditionally (`...t.effort!==undefined && {effort}`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<Effort>,
    /// Command-deny rules FROZEN at fork time, replayed ahead of the live deny
    /// list on resume so a later settings edit cannot widen what the fork was
    /// allowed to run. Omitted when empty — claude's spread is gated on
    /// `f !== undefined && f.length > 0`, so an empty list writes no key.
    #[serde(
        rename = "frozenCommandDenies",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub frozen_command_denies: Option<Vec<String>>,
}

impl ForkedSkillScoping {
    /// Whether this record satisfies claude's `qCo` schema — the same check the
    /// binary runs BEFORE writing (`if(!qCo().safeParse(A).success) return
    /// forked_skill_scoping_unpersistable`). A record that would not round-trip
    /// must never reach disk, because the resume path treats an unparseable
    /// record as a refusal, not as "no scoping".
    #[must_use]
    pub fn is_valid(&self) -> bool {
        if !is_valid_skill_name(&self.skill_name) || !is_valid_skill_name(&self.attribution_name) {
            return false;
        }
        if let Some(effort) = &self.effort {
            if !effort.is_valid() {
                return false;
            }
        }
        if let Some(denies) = &self.frozen_command_denies {
            if denies.len() > FROZEN_DENY_MAX_COUNT
                || denies.iter().any(|d| d.len() > FROZEN_DENY_MAX_LEN)
            {
                return false;
            }
        }
        true
    }
}

/// The provenance marker (claude `Odd`) — the witness that this session ran as
/// a forked skill at all.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForkedSkillMarker {
    /// Always `true`; a marker whose flag is anything else fails the schema and
    /// reads as no marker.
    #[serde(rename = "forkedSkill")]
    pub forked_skill: bool,
    /// The skill's name, when the launch knew it.
    #[serde(rename = "skillName", default, skip_serializing_if = "Option::is_none")]
    pub skill_name: Option<String>,
}

/// Whether `name` satisfies claude's skill-name schema (`UCo`): 1..=256 chars,
/// containing neither `\r` nor `\n`.
///
/// The newline ban is not cosmetic: the name is interpolated into refusal
/// messages and matched against a task record, so an embedded newline could
/// forge a second line of a rendered message.
#[must_use]
pub fn is_valid_skill_name(name: &str) -> bool {
    let len = name.chars().count();
    len >= 1 && len <= SKILL_NAME_MAX_LEN && !name.contains(['\r', '\n'])
}

/// The two sidecar paths for a session transcript (claude `qon`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForkedSkillPaths {
    /// `<uuid>.forked-skill.json` — the scoping record.
    pub scoping: PathBuf,
    /// `<uuid>.forked-skill.marker.json` — the provenance marker.
    pub provenance_marker: PathBuf,
}

/// Derive both sidecar paths from a session JSONL path.
///
/// Claude replaces a TRAILING `.jsonl` (`e.replace(/\.jsonl$/, …)`); a path
/// that does not end in `.jsonl` is left intact and simply gains the suffix, so
/// the two sidecars stay siblings of whatever file was named.
#[must_use]
pub fn forked_skill_paths(session_jsonl: &Path) -> ForkedSkillPaths {
    let raw = session_jsonl.to_string_lossy();
    let stem = raw.strip_suffix(".jsonl").unwrap_or(&raw);
    ForkedSkillPaths {
        scoping: PathBuf::from(format!("{stem}.forked-skill.json")),
        provenance_marker: PathBuf::from(format!("{stem}.forked-skill.marker.json")),
    }
}

/// What a read of the scoping sidecar found (claude's `{status: …}` union).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScopingStatus {
    /// A well-formed record. The resume path may proceed once the identity
    /// corroborates.
    Valid(Box<ForkedSkillScoping>),
    /// The file exists but is not a usable record — not a regular file (e.g. a
    /// symlink or directory), over [`SIDECAR_MAX_BYTES`], not JSON, or failing
    /// the schema. ALWAYS a refusal: an unreadable scoping record is not the
    /// same as no scoping.
    Malformed,
    /// Neither file exists — an ordinary, never-forked session.
    Absent,
    /// The scoping record is gone but the provenance marker remains, i.e. this
    /// session DID run as a forked skill and its scoping has been removed.
    /// A refusal. Also the fallback for a marker `lstat` that fails for any
    /// reason other than "not found": an unreadable marker must not be read as
    /// an absent one.
    AbsentButMarked,
}

/// Write both sidecars for a fork about to launch (claude `qdd`).
///
/// Order matters and is the binary's: create the parent directory, write the
/// MARKER first, then the scoping record. A crash between the two leaves
/// [`ScopingStatus::AbsentButMarked`] — a refusal — whereas the reverse order
/// would leave a scoping record with no witness, which the cold-resume path
/// also refuses but which reads as a corrupted state rather than an
/// interrupted one.
///
/// # Errors
/// Any filesystem error from the directory create or either write.
pub async fn write_fork_records(
    session_jsonl: &Path,
    scoping: &ForkedSkillScoping,
) -> std::io::Result<()> {
    let paths = forked_skill_paths(session_jsonl);
    if let Some(parent) = paths.scoping.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let marker = ForkedSkillMarker {
        forked_skill: true,
        skill_name: Some(scoping.skill_name.clone()),
    };
    let marker_json =
        serde_json::to_string(&marker).map_err(|e| std::io::Error::other(e.to_string()))?;
    tokio::fs::write(&paths.provenance_marker, marker_json).await?;
    let scoping_json =
        serde_json::to_string(scoping).map_err(|e| std::io::Error::other(e.to_string()))?;
    tokio::fs::write(&paths.scoping, scoping_json).await?;
    Ok(())
}

/// Read the scoping sidecar for a session (claude `jdd` → `VCo`).
pub async fn read_scoping(session_jsonl: &Path) -> ScopingStatus {
    read_scoping_at(&forked_skill_paths(session_jsonl)).await
}

/// [`read_scoping`] against already-derived paths.
pub async fn read_scoping_at(paths: &ForkedSkillPaths) -> ScopingStatus {
    // `symlink_metadata` is `lstat`: a symlink planted here reports
    // `is_file() == false` and is rejected rather than followed.
    let meta = match tokio::fs::symlink_metadata(&paths.scoping).await {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return read_marker_presence(&paths.provenance_marker).await;
        }
        // A non-ENOENT stat error (permissions, I/O) is NOT "absent" — claude
        // only takes the marker branch on `Gt(o)` (its ENOENT predicate) and
        // falls through to `malformed` otherwise.
        Err(_) => return ScopingStatus::Malformed,
    };
    if !meta.is_file() || meta.len() > SIDECAR_MAX_BYTES {
        return ScopingStatus::Malformed;
    }
    let Ok(text) = tokio::fs::read_to_string(&paths.scoping).await else {
        return ScopingStatus::Malformed;
    };
    let Ok(scoping) = serde_json::from_str::<ForkedSkillScoping>(&text) else {
        return ScopingStatus::Malformed;
    };
    if !scoping.is_valid() {
        return ScopingStatus::Malformed;
    }
    ScopingStatus::Valid(Box::new(scoping))
}

/// The marker-only branch (claude `ZN_`): the scoping record is gone, so the
/// question is only whether this session was EVER a forked skill.
async fn read_marker_presence(marker: &Path) -> ScopingStatus {
    match tokio::fs::symlink_metadata(marker).await {
        Ok(_) => ScopingStatus::AbsentButMarked,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => ScopingStatus::Absent,
        // Fail CLOSED: a marker we cannot stat might exist, and treating it as
        // absent would silently downgrade a forked skill to an unscoped resume.
        Err(_) => ScopingStatus::AbsentButMarked,
    }
}

/// Read the skill name witnessed by the provenance marker (claude `Wdd`).
///
/// `None` for every failure mode — missing, not a regular file, oversized,
/// unparseable, or failing the schema. The cold-resume check compares this
/// against the scoping record's `skillName`, and a `None` never matches a valid
/// name, so every failure is a refusal there.
pub async fn read_marker_skill_name(session_jsonl: &Path) -> Option<String> {
    let paths = forked_skill_paths(session_jsonl);
    let meta = tokio::fs::symlink_metadata(&paths.provenance_marker)
        .await
        .ok()?;
    if !meta.is_file() || meta.len() > SIDECAR_MAX_BYTES {
        return None;
    }
    let text = tokio::fs::read_to_string(&paths.provenance_marker).await.ok()?;
    let marker = serde_json::from_str::<ForkedSkillMarker>(&text).ok()?;
    // `De.literal(!0)` — a marker whose flag is not exactly `true` fails the
    // schema, and a failed parse yields `undefined`.
    if !marker.forked_skill {
        return None;
    }
    match marker.skill_name {
        Some(name) if is_valid_skill_name(&name) => Some(name),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn scoping(skill: &str) -> ForkedSkillScoping {
        ForkedSkillScoping {
            skill_name: skill.into(),
            attribution_name: skill.into(),
            effort: None,
            frozen_command_denies: None,
        }
    }

    #[test]
    fn paths_replace_a_trailing_jsonl_suffix() {
        let p = forked_skill_paths(Path::new("/p/projects/x/abc-123.jsonl"));
        assert_eq!(
            p.scoping,
            PathBuf::from("/p/projects/x/abc-123.forked-skill.json")
        );
        assert_eq!(
            p.provenance_marker,
            PathBuf::from("/p/projects/x/abc-123.forked-skill.marker.json")
        );
    }

    /// Claude anchors the replace (`/\.jsonl$/`), so an INNER `.jsonl` is not
    /// touched — the sidecars stay siblings of the file that was named.
    #[test]
    fn only_a_trailing_jsonl_is_replaced() {
        let p = forked_skill_paths(Path::new("/p/a.jsonl.bak"));
        assert_eq!(p.scoping, PathBuf::from("/p/a.jsonl.bak.forked-skill.json"));
    }

    #[tokio::test]
    async fn round_trips_a_written_record() {
        let dir = tempdir().unwrap();
        let jsonl = dir.path().join("s.jsonl");
        let mut rec = scoping("code-review");
        rec.attribution_name = "reviewer".into();
        rec.effort = Some(Effort::Level("high".into()));
        rec.frozen_command_denies = Some(vec!["Bash(rm:*)".into()]);
        write_fork_records(&jsonl, &rec).await.unwrap();

        match read_scoping(&jsonl).await {
            ScopingStatus::Valid(got) => assert_eq!(*got, rec),
            other => panic!("expected Valid, got {other:?}"),
        }
        assert_eq!(
            read_marker_skill_name(&jsonl).await.as_deref(),
            Some("code-review")
        );
    }

    /// The wire shape is claude's: camelCase keys, and the two optional keys
    /// ABSENT rather than `null` when unset (claude spreads them conditionally).
    #[tokio::test]
    async fn omits_unset_optional_keys_on_the_wire() {
        let dir = tempdir().unwrap();
        let jsonl = dir.path().join("s.jsonl");
        write_fork_records(&jsonl, &scoping("s")).await.unwrap();
        let text =
            tokio::fs::read_to_string(forked_skill_paths(&jsonl).scoping)
                .await
                .unwrap();
        assert_eq!(text, r#"{"skillName":"s","attributionName":"s"}"#);
        let marker =
            tokio::fs::read_to_string(forked_skill_paths(&jsonl).provenance_marker)
                .await
                .unwrap();
        assert_eq!(marker, r#"{"forkedSkill":true,"skillName":"s"}"#);
    }

    #[tokio::test]
    async fn no_sidecars_is_absent() {
        let dir = tempdir().unwrap();
        assert_eq!(
            read_scoping(&dir.path().join("s.jsonl")).await,
            ScopingStatus::Absent
        );
    }

    /// The whole point of the marker: a DELETED scoping record on a session
    /// that did fork is a refusal, not a plain unscoped resume.
    #[tokio::test]
    async fn marker_without_scoping_is_absent_but_marked() {
        let dir = tempdir().unwrap();
        let jsonl = dir.path().join("s.jsonl");
        write_fork_records(&jsonl, &scoping("s")).await.unwrap();
        tokio::fs::remove_file(forked_skill_paths(&jsonl).scoping)
            .await
            .unwrap();
        assert_eq!(read_scoping(&jsonl).await, ScopingStatus::AbsentButMarked);
    }

    #[tokio::test]
    async fn unparseable_or_schema_failing_records_are_malformed() {
        let dir = tempdir().unwrap();
        for (name, body) in [
            ("a", "not json"),
            ("b", r#"{"skillName":"s"}"#),               // missing attributionName
            ("c", r#"{"skillName":"","attributionName":"s"}"#), // empty name
            (
                "d",
                r#"{"skillName":"s","attributionName":"s","effort":"turbo"}"#,
            ), // effort not in the union
            (
                "e",
                r#"{"skillName":"s","attributionName":"s","effort":0}"#,
            ), // effort below the min
        ] {
            let jsonl = dir.path().join(format!("{name}.jsonl"));
            tokio::fs::write(forked_skill_paths(&jsonl).scoping, body)
                .await
                .unwrap();
            assert_eq!(
                read_scoping(&jsonl).await,
                ScopingStatus::Malformed,
                "{name}: {body}"
            );
        }
    }

    /// `lstat`, not `stat`: a symlink at the scoping path is rejected outright
    /// rather than followed. Otherwise a planted link could point the resume
    /// path at an attacker-chosen scoping record.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_symlinked_scoping_record_is_malformed_not_followed() {
        let dir = tempdir().unwrap();
        let jsonl = dir.path().join("s.jsonl");
        let real = dir.path().join("real.json");
        tokio::fs::write(&real, serde_json::to_string(&scoping("evil")).unwrap())
            .await
            .unwrap();
        std::os::unix::fs::symlink(&real, forked_skill_paths(&jsonl).scoping).unwrap();

        assert_eq!(read_scoping(&jsonl).await, ScopingStatus::Malformed);
    }

    #[tokio::test]
    async fn an_oversized_record_is_rejected_unread() {
        let dir = tempdir().unwrap();
        let jsonl = dir.path().join("s.jsonl");
        let padded = format!(
            r#"{{"skillName":"s","attributionName":"s","_pad":"{}"}}"#,
            "x".repeat(SIDECAR_MAX_BYTES as usize)
        );
        tokio::fs::write(forked_skill_paths(&jsonl).scoping, padded)
            .await
            .unwrap();
        assert_eq!(read_scoping(&jsonl).await, ScopingStatus::Malformed);
    }

    #[tokio::test]
    async fn marker_witness_is_none_for_every_failure_mode() {
        let dir = tempdir().unwrap();
        for (name, body) in [
            ("a", "not json"),
            ("b", r#"{"forkedSkill":false,"skillName":"s"}"#),
            ("c", r#"{"forkedSkill":true}"#),
            ("d", "{\"forkedSkill\":true,\"skillName\":\"a\\nb\"}"),
        ] {
            let jsonl = dir.path().join(format!("{name}.jsonl"));
            tokio::fs::write(forked_skill_paths(&jsonl).provenance_marker, body)
                .await
                .unwrap();
            assert_eq!(read_marker_skill_name(&jsonl).await, None, "{name}: {body}");
        }
    }

    #[test]
    fn skill_names_reject_newlines_and_length_extremes() {
        assert!(is_valid_skill_name("ok"));
        assert!(is_valid_skill_name(&"x".repeat(SKILL_NAME_MAX_LEN)));
        assert!(!is_valid_skill_name(&"x".repeat(SKILL_NAME_MAX_LEN + 1)));
        assert!(!is_valid_skill_name(""));
        // A newline in the name could forge a line of a rendered refusal.
        assert!(!is_valid_skill_name("a\nb"));
        assert!(!is_valid_skill_name("a\rb"));
    }

    #[test]
    fn scoping_validation_matches_the_schema_bounds() {
        let mut rec = scoping("s");
        assert!(rec.is_valid());
        rec.frozen_command_denies = Some(vec!["x".repeat(FROZEN_DENY_MAX_LEN)]);
        assert!(rec.is_valid());
        rec.frozen_command_denies = Some(vec!["x".repeat(FROZEN_DENY_MAX_LEN + 1)]);
        assert!(!rec.is_valid(), "a rule over 1024 chars is unpersistable");
        rec.frozen_command_denies = Some(vec!["d".into(); FROZEN_DENY_MAX_COUNT + 1]);
        assert!(!rec.is_valid(), "over 1000 rules is unpersistable");
        rec.frozen_command_denies = None;
        rec.effort = Some(Effort::Steps(EFFORT_MAX));
        assert!(rec.is_valid());
        rec.effort = Some(Effort::Steps(EFFORT_MAX + 1));
        assert!(!rec.is_valid());
    }
}
