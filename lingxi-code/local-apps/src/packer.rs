//! Deterministic builtin plugin packer (P1.1 + P1.2).
//!
//! Two measured facts drive this module's shape:
//!
//! - **Pruning is not optional hardening.** On disk today, `babylon-3d`
//!   carries 20,093 files and `phaser-2d` 13,014 after `pnpm install`; the
//!   whole templates tree is 524 MiB against the §19.0 4 MiB archive
//!   ceiling. Without pruning `node_modules`, `dist`, `.vite`, and
//!   `.lingxi-build-state`, packing any real family is a non-starter.
//! - **The packer is inventory-driven, not a directory walk.**
//!   `runtime-profiles/` has 124 tracked files but only 112 are referenced by
//!   `profile_file!` call sites — the other 12 are dead scaffold, the same
//!   three paths repeated across four families. A walk would carry them; an
//!   explicit [`InventoryEntry`] list does not. The walk this module *does*
//!   perform exists only to catch the inverse mistake — a file sitting in a
//!   real (non-pruned) directory that the inventory never declared — so the
//!   build fails loudly instead of silently shipping (or silently omitting)
//!   an untracked file.
//!
//! This module packs a **synthetic fixture plugin root**, not the real
//! `templates/` tree — see the P1.1/P1.2 task brief for why that split is
//! mandatory rather than a convenience. Packing the real package (deriving
//! its [`InventoryEntry`] list from `profile_file!` call sites or from
//! `docs/local-apps/harness/template-migration-manifest.json`) is later work.
//!
//! ## Archive format
//!
//! The archive is a from-scratch deterministic container, not a zip: this
//! crate has no `zip` dependency (that lives in the `plugin` crate, on the
//! *unpack* side — `plugin/src/mcpb.rs`), and adding one is a `Cargo.toml`
//! change outside this task's owned files. Producing a real `.mcpb`-compatible
//! zip is later work; what matters for P1.1/P1.2 is that the archive is
//! **byte-stable across repeated builds of the same inputs** and **sensitive
//! to every content byte**, which this format satisfies without needing zip's
//! own determinism knobs (timestamps, extra fields, entry order).
//!
//! Records are sorted by path and laid out as:
//! `[u32 LE path_len][path bytes][u64 LE content_len][content bytes]`,
//! concatenated with no separators, header, or trailer. The archive digest is
//! the lowercase-hex SHA-256 of that concatenation.
//!
//! ## Does the pack side need its own `MAX_FILES` (cf. `plugin/src/mcpb.rs`)?
//!
//! `plugin/src/mcpb.rs`'s `MAX_FILES = 10_000` guards the *unpack* path,
//! where the byte count comes from an untrusted archive someone hands the
//! host. The pack side's file set is not attacker-controlled in the same
//! way: it is exactly the curated [`InventoryEntry`] list (112 entries for
//! the real templates tree today), not a raw scan of however large a
//! directory happens to be. The failure mode a count cap would guard against
//! — unbounded growth silently entering the archive — is instead caught by
//! [`PackerError::FileOutsideInventory`]: a build fails loudly the moment a
//! non-pruned directory grows a file the inventory does not name, rather
//! than the archive silently ballooning. So no separate pack-side file-count
//! limit is added here; the §19.0 byte-size ceilings (4 MiB archive / 12 MiB
//! extracted) remain the numeric gate and are a later task's responsibility
//! once this module packs the real package.

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::path::{Component, Path};

use sha2::{Digest, Sha256};
use thiserror::Error;

/// Build-artifact directory names pruned unconditionally, at any depth,
/// before the packer inspects — or even descends into — their contents.
pub const PRUNE_DIR_NAMES: [&str; 4] = ["node_modules", "dist", ".vite", ".lingxi-build-state"];

/// Lowercase-hex SHA-256 of `bytes`.
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// One path the packer is authorized to include. Driving inclusion from this
/// explicit list (rather than "whatever the walk finds") is what keeps dead
/// scaffold out of the archive without hand-pruning it on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InventoryEntry {
    /// Root-relative path, `/`-separated, matching on-disk and archive layout.
    pub path: String,
    /// Optional known-answer content hash (lowercase-hex SHA-256), checked
    /// against the file actually read when present — pins a specific entry
    /// against silent drift independent of the whole-archive digest.
    pub expected_sha256: Option<String>,
}

impl InventoryEntry {
    /// An entry with no pinned hash.
    #[must_use]
    pub fn new(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            expected_sha256: None,
        }
    }

    /// An entry pinned to a known content hash.
    #[must_use]
    pub fn with_sha256(path: impl Into<String>, sha256: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            expected_sha256: Some(sha256.into()),
        }
    }
}

/// One file actually packed — the inventory delivered alongside the archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackedFile {
    /// Root-relative path, `/`-separated.
    pub path: String,
    /// Content length in bytes.
    pub bytes: u64,
    /// Lowercase-hex SHA-256 of the content.
    pub sha256: String,
}

/// Output of [`pack`].
#[derive(Debug, Clone)]
pub struct PackResult {
    /// The archive bytes (see module docs for the record format).
    pub archive: Vec<u8>,
    /// Lowercase-hex SHA-256 of `archive`.
    pub archive_digest: String,
    /// The resolved per-file inventory, sorted by path (the archive's own
    /// record order).
    pub inventory: Vec<PackedFile>,
}

/// Failure detail. Every variant names the offending path so a caller (and a
/// test) can assert on *what* was rejected, not just that something was.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum PackerError {
    /// A file sits in a real (non-pruned) directory that the inventory never
    /// declared. This is the packer's core reconciliation gate.
    #[error("file outside the inventory in a non-prune directory: {0}")]
    FileOutsideInventory(String),
    /// An inventory entry names a file that is not on disk (`NotFound`
    /// specifically — an entry that exists but is unreadable is
    /// [`PackerError::Io`]).
    #[error("inventory entry missing on disk: {0}")]
    MissingOnDisk(String),
    /// An inventory entry's pinned `expected_sha256` did not match the file
    /// actually read.
    #[error("inventory entry sha256 mismatch for {path}: expected {expected}, got {actual}")]
    HashMismatch {
        /// The offending root-relative path.
        path: String,
        /// The hash pinned in the inventory.
        expected: String,
        /// The hash actually computed from disk.
        actual: String,
    },
    /// The same root-relative path was declared twice.
    #[error("duplicate inventory path: {0}")]
    DuplicatePath(String),
    /// An inventory path is absolute, empty, or contains a `.`/`..`/prefix
    /// component — anything that could resolve outside `root`.
    #[error("inventory path is not a plain relative path: {0}")]
    InvalidPath(String),
    /// A filesystem read failed for a reason other than the file being
    /// absent — the directory walk itself, or an inventory entry that exists
    /// but cannot be read (a directory, a permission-denied file). Distinct
    /// from [`PackerError::MissingOnDisk`] so a caller is not sent hunting for
    /// a file that is present.
    #[error("failed to read {path}: {detail}")]
    Io {
        /// The path being read when the failure occurred.
        path: String,
        /// `to_string()` of the underlying `std::io::Error`.
        detail: String,
    },
}

/// True when `rel` (any path, entry-declared or disk-observed) has a
/// component matching one of [`PRUNE_DIR_NAMES`] — i.e. it lives under a
/// build-artifact directory that is pruned regardless of what the inventory
/// says about it.
fn is_pruned(rel: &Path) -> bool {
    rel.components().any(|component| {
        matches!(component, Component::Normal(name) if PRUNE_DIR_NAMES.iter().any(|p| name == OsStr::new(p)))
    })
}

/// Validate that `path` is a plain root-relative path: non-empty, no
/// leading/embedded `.`/`..`/root/prefix component. Guards the `root.join`
/// below from ever resolving outside `root`.
fn validate_relative_path(path: &str) -> Result<(), PackerError> {
    if path.is_empty() {
        return Err(PackerError::InvalidPath(path.to_string()));
    }
    let candidate = Path::new(path);
    let all_normal = candidate
        .components()
        .all(|component| matches!(component, Component::Normal(_)));
    if !all_normal {
        return Err(PackerError::InvalidPath(path.to_string()));
    }
    Ok(())
}

/// Join `rel`'s components with `/` regardless of host path separator, so
/// the observed-on-disk string always matches the `/`-separated convention
/// [`InventoryEntry::path`] uses.
fn to_slash_string(rel: &Path) -> String {
    rel.components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

/// Recursively collect every regular file under `dir` (relative to `root`,
/// `/`-joined), never descending into a [`PRUNE_DIR_NAMES`] directory.
fn walk_non_pruned_files(
    root: &Path,
    dir: &Path,
    out: &mut BTreeSet<String>,
) -> Result<(), PackerError> {
    let entries = std::fs::read_dir(dir).map_err(|e| PackerError::Io {
        path: dir.display().to_string(),
        detail: e.to_string(),
    })?;
    for entry in entries {
        let entry = entry.map_err(|e| PackerError::Io {
            path: dir.display().to_string(),
            detail: e.to_string(),
        })?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|e| PackerError::Io {
            path: path.display().to_string(),
            detail: e.to_string(),
        })?;
        if file_type.is_dir() {
            let name = entry.file_name();
            if PRUNE_DIR_NAMES.iter().any(|p| name == OsStr::new(p)) {
                // Pruned: never descended into, so an arbitrarily large
                // node_modules never costs a directory read per file.
                continue;
            }
            walk_non_pruned_files(root, &path, out)?;
        } else if file_type.is_file() {
            let rel = path
                .strip_prefix(root)
                .expect("walked path is always under root by construction");
            out.insert(to_slash_string(rel));
        }
        // Symlinks are neither `is_dir()` nor `is_file()` here and are
        // silently skipped — the synthetic fixture never contains one, and
        // symlink-escape handling is out of this task's scope (see the
        // written report).
    }
    Ok(())
}

/// Serialize one archive record: `[u32 LE path_len][path][u64 LE content_len][content]`.
fn write_record(buf: &mut Vec<u8>, path: &str, content: &[u8]) {
    let path_bytes = path.as_bytes();
    buf.extend_from_slice(&(path_bytes.len() as u32).to_le_bytes());
    buf.extend_from_slice(path_bytes);
    buf.extend_from_slice(&(content.len() as u64).to_le_bytes());
    buf.extend_from_slice(content);
}

/// Pack `root` into a deterministic archive, using `inventory` as the sole
/// authority on what belongs in it.
///
/// Algorithm:
/// 1. Validate every inventory path (non-empty, purely relative, no
///    duplicates).
/// 2. Walk `root` on disk, never descending into a pruned directory. Any
///    file found in a non-pruned directory that the inventory did not
///    declare fails the build ([`PackerError::FileOutsideInventory`]).
/// 3. Read, hash, and (if pinned) verify every non-pruned inventory entry,
///    in path-sorted order.
/// 4. Concatenate sorted records into the archive and hash the whole thing.
///
/// # Errors
/// See [`PackerError`] variants; each names the offending path.
pub fn pack(root: &Path, inventory: &[InventoryEntry]) -> Result<PackResult, PackerError> {
    let mut declared_paths: BTreeSet<&str> = BTreeSet::new();
    for entry in inventory {
        validate_relative_path(&entry.path)?;
        if !declared_paths.insert(entry.path.as_str()) {
            return Err(PackerError::DuplicatePath(entry.path.clone()));
        }
    }

    // Reconcile: every file the walk finds outside a pruned directory must
    // be declared. This is what makes the packer's inventory authoritative
    // rather than aspirational — a stray file cannot silently ship, and a
    // stray file cannot silently vanish either (the walk would have caught
    // its absence from `declared_paths` before we ever try to read it).
    let mut present = BTreeSet::new();
    walk_non_pruned_files(root, root, &mut present)?;
    let non_pruned_declared: BTreeSet<&str> = declared_paths
        .iter()
        .copied()
        .filter(|p| !is_pruned(Path::new(p)))
        .collect();
    if let Some(stray) = present
        .iter()
        .find(|p| !non_pruned_declared.contains(p.as_str()))
    {
        return Err(PackerError::FileOutsideInventory(stray.clone()));
    }

    let mut ordered: Vec<&InventoryEntry> = inventory
        .iter()
        .filter(|entry| !is_pruned(Path::new(&entry.path)))
        .collect();
    ordered.sort_by(|a, b| a.path.cmp(&b.path));

    let mut archive = Vec::new();
    let mut packed = Vec::with_capacity(ordered.len());
    for entry in ordered {
        let absolute = root.join(&entry.path);
        let bytes = std::fs::read(&absolute).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                PackerError::MissingOnDisk(entry.path.clone())
            } else {
                // Not "absent" — unreadable. Reporting this as MissingOnDisk
                // would send a caller looking for a file that is right there.
                PackerError::Io {
                    path: entry.path.clone(),
                    detail: e.to_string(),
                }
            }
        })?;
        let sha256 = sha256_hex(&bytes);
        if let Some(expected) = &entry.expected_sha256 {
            if expected != &sha256 {
                return Err(PackerError::HashMismatch {
                    path: entry.path.clone(),
                    expected: expected.clone(),
                    actual: sha256.clone(),
                });
            }
        }
        write_record(&mut archive, &entry.path, &bytes);
        packed.push(PackedFile {
            path: entry.path.clone(),
            bytes: bytes.len() as u64,
            sha256,
        });
    }

    let archive_digest = sha256_hex(&archive);
    Ok(PackResult {
        archive,
        archive_digest,
        inventory: packed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Write `content` to `root/rel`, creating parent directories as needed.
    fn write_file(root: &Path, rel: &str, content: &[u8]) {
        let full = root.join(rel);
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(full, content).unwrap();
    }

    /// A small synthetic fixture: three files across two subdirectories, none
    /// of them in a pruned directory.
    fn small_fixture(root: &Path) -> Vec<InventoryEntry> {
        write_file(root, "plugin.json", b"{\"name\":\"fixture\"}");
        write_file(
            root,
            "app/screens/home.jsx",
            b"export default function Home() {}\n",
        );
        write_file(
            root,
            "app/screens/detail.jsx",
            b"export default function Detail() {}\n",
        );
        vec![
            InventoryEntry::new("plugin.json"),
            InventoryEntry::new("app/screens/home.jsx"),
            InventoryEntry::new("app/screens/detail.jsx"),
        ]
    }

    #[test]
    fn two_builds_of_the_same_root_produce_the_same_digest() {
        let tmp = tempfile::tempdir().unwrap();
        let inventory = small_fixture(tmp.path());

        let first = pack(tmp.path(), &inventory).unwrap();
        let second = pack(tmp.path(), &inventory).unwrap();

        assert_eq!(first.archive, second.archive);
        assert_eq!(first.archive_digest, second.archive_digest);
        assert_eq!(first.inventory, second.inventory);
    }

    #[test]
    fn digest_is_identical_on_a_dirty_and_a_clean_checkout() {
        let clean = tempfile::tempdir().unwrap();
        let inventory = small_fixture(clean.path());
        let clean_result = pack(clean.path(), &inventory).unwrap();

        // Simulate a checkout that has been through `pnpm install` and a
        // build: populate all four pruned directories, at more than one
        // depth, with content that VARIES in size and byte content run to
        // run — if the packer ever let any of this leak into the archive,
        // the digest below would not match `clean_result`.
        let dirty = tempfile::tempdir().unwrap();
        let dirty_inventory = small_fixture(dirty.path());
        write_file(
            dirty.path(),
            "node_modules/left-pad/index.js",
            b"module.exports = leftPad;\n",
        );
        write_file(
            dirty.path(),
            "node_modules/.pnpm/some-pkg@1.0.0/node_modules/some-pkg/deep/file.js",
            &vec![7u8; 4096],
        );
        write_file(dirty.path(), "dist/bundle.js", &vec![9u8; 2048]);
        write_file(
            dirty.path(),
            "app/.vite/deps/chunk-ABCD.js",
            b"/* vite cache */",
        );
        write_file(
            dirty.path(),
            ".lingxi-build-state/last-build.json",
            b"{\"builtAt\":\"2026-08-29T00:00:00Z\"}",
        );
        let dirty_result = pack(dirty.path(), &dirty_inventory).unwrap();

        assert_eq!(clean_result.archive_digest, dirty_result.archive_digest);
        assert_eq!(clean_result.archive, dirty_result.archive);
    }

    #[test]
    fn a_file_outside_the_inventory_in_a_non_prune_dir_fails_the_build() {
        let tmp = tempfile::tempdir().unwrap();
        let inventory = small_fixture(tmp.path());
        // Not pruned, not declared.
        write_file(
            tmp.path(),
            "app/screens/forgotten.jsx",
            b"export default function Forgotten() {}\n",
        );

        let err = pack(tmp.path(), &inventory).unwrap_err();
        assert_eq!(
            err,
            PackerError::FileOutsideInventory("app/screens/forgotten.jsx".to_string())
        );
    }

    /// The named hazard for this batch: a digest that is (or degenerates
    /// into) a length fingerprint passes every test above, because none of
    /// them tamper with content while holding length constant. This test
    /// pins that a same-length, different-bytes edit changes the digest.
    #[test]
    fn same_length_different_bytes_changes_the_digest() {
        let a = tempfile::tempdir().unwrap();
        write_file(a.path(), "payload.bin", b"AAAA");
        let inv_a = vec![InventoryEntry::new("payload.bin")];
        let result_a = pack(a.path(), &inv_a).unwrap();

        let b = tempfile::tempdir().unwrap();
        write_file(b.path(), "payload.bin", b"BBBB"); // same length (4 bytes), different content
        let inv_b = vec![InventoryEntry::new("payload.bin")];
        let result_b = pack(b.path(), &inv_b).unwrap();

        // Pinned against the LITERAL 4, not against each other: comparing the
        // two sides is vacuous when `bytes` degenerates to a constant, so the
        // control that proves "the fixture really is same-length" would stop
        // proving anything exactly when it matters most.
        assert_eq!(
            result_a.inventory[0].bytes, 4,
            "fixture bug: a is not 4 bytes"
        );
        assert_eq!(
            result_b.inventory[0].bytes, 4,
            "fixture bug: b is not 4 bytes"
        );
        assert_ne!(result_a.archive_digest, result_b.archive_digest);
        assert_ne!(result_a.inventory[0].sha256, result_b.inventory[0].sha256);
    }

    /// Positive control for the above: an actual duplicate build of the
    /// SAME content must still agree, so the previous test is proof the
    /// digest reads bytes — not proof it is merely noisy/non-deterministic.
    #[test]
    fn same_length_same_bytes_same_digest_control() {
        let a = tempfile::tempdir().unwrap();
        write_file(a.path(), "payload.bin", b"AAAA");
        let inv = vec![InventoryEntry::new("payload.bin")];
        let r1 = pack(a.path(), &inv).unwrap();
        let r2 = pack(a.path(), &inv).unwrap();
        assert_eq!(r1.archive_digest, r2.archive_digest);
    }

    /// Known-answer vector: the exact digest for a single-file archive,
    /// pinned from an actual run of this function (not hand-computed), so a
    /// future edit that silently changes the record format or hashes the
    /// wrong bytes shows up as a value mismatch, not just "still passes".
    #[test]
    fn archive_digest_is_a_pinned_known_answer_vector() {
        let tmp = tempfile::tempdir().unwrap();
        write_file(tmp.path(), "hello.txt", b"hello world");
        let inventory = vec![InventoryEntry::new("hello.txt")];

        let result = pack(tmp.path(), &inventory).unwrap();

        // Record layout for this single entry:
        //   u32 LE path_len=9, "hello.txt", u64 LE content_len=11, "hello world"
        let mut expected_archive = Vec::new();
        expected_archive.extend_from_slice(&9u32.to_le_bytes());
        expected_archive.extend_from_slice(b"hello.txt");
        expected_archive.extend_from_slice(&11u64.to_le_bytes());
        expected_archive.extend_from_slice(b"hello world");
        assert_eq!(result.archive, expected_archive);

        assert_eq!(
            result.archive_digest,
            "4cf2739e19d5d8f1a0f8ec859cac115fe854d271b53a689b7f105726cebd49f5"
        );
        assert_eq!(result.inventory[0].sha256, sha256_hex(b"hello world"));
    }

    /// The archive must be canonically ordered, so its digest depends on the
    /// *set* of inventory entries and not on the order they were assembled in
    /// — the real inventory will be built from `profile_file!` call sites or a
    /// JSON manifest, and neither has a guaranteed iteration order.
    ///
    /// This is the axis the code actually reads. Packing the same root twice
    /// (`two_builds_of_the_same_root_produce_the_same_digest`) only *correlates*
    /// with determinism: deleting `ordered.sort_by` leaves that test — and
    /// every other test in this module before this one existed — green.
    #[test]
    fn inventory_declaration_order_does_not_change_the_digest() {
        let tmp = tempfile::tempdir().unwrap();
        let declared = small_fixture(tmp.path());
        // `small_fixture` returns entries in a deliberately UNSORTED order
        // (plugin.json, home, detail), so the sort is load-bearing here.
        assert_ne!(
            declared.iter().map(|e| e.path.as_str()).collect::<Vec<_>>(),
            {
                let mut sorted = declared.iter().map(|e| e.path.as_str()).collect::<Vec<_>>();
                sorted.sort_unstable();
                sorted
            },
            "fixture bug: declaration order is already sorted, so this test \
             cannot detect a missing sort"
        );

        let mut reversed = declared.clone();
        reversed.reverse();
        let mut rotated = declared.clone();
        rotated.rotate_left(1);

        let a = pack(tmp.path(), &declared).unwrap();
        let b = pack(tmp.path(), &reversed).unwrap();
        let c = pack(tmp.path(), &rotated).unwrap();

        // Asserted BEFORE the raw-byte comparisons below so that a regression
        // reports a readable list of paths rather than a 177-element byte dump
        // — the canonical order is specifically path-sorted, not "whatever the
        // first caller happened to pass".
        for (label, result) in [("declared", &a), ("reversed", &b), ("rotated", &c)] {
            assert_eq!(
                result
                    .inventory
                    .iter()
                    .map(|f| f.path.as_str())
                    .collect::<Vec<_>>(),
                vec![
                    "app/screens/detail.jsx",
                    "app/screens/home.jsx",
                    "plugin.json"
                ],
                "archive order is not path-sorted for the {label} inventory"
            );
        }

        assert_eq!(a.archive, b.archive);
        assert_eq!(a.archive, c.archive);
        assert_eq!(a.archive_digest, b.archive_digest);
        assert_eq!(a.archive_digest, c.archive_digest);
    }

    /// Multi-file known-answer vector. Unlike the single-file one, this pins
    /// record ORDER and the path bytes of three distinct entries. Every value
    /// below was computed independently of this implementation (see the review
    /// report) rather than captured from a run of it.
    #[test]
    fn multi_file_archive_is_a_pinned_known_answer_vector() {
        let tmp = tempfile::tempdir().unwrap();
        let inventory = small_fixture(tmp.path());
        let result = pack(tmp.path(), &inventory).unwrap();

        let mut expected = Vec::new();
        for (path, content) in [
            (
                "app/screens/detail.jsx",
                &b"export default function Detail() {}\n"[..],
            ),
            (
                "app/screens/home.jsx",
                &b"export default function Home() {}\n"[..],
            ),
            ("plugin.json", &b"{\"name\":\"fixture\"}"[..]),
        ] {
            expected.extend_from_slice(&(path.len() as u32).to_le_bytes());
            expected.extend_from_slice(path.as_bytes());
            expected.extend_from_slice(&(content.len() as u64).to_le_bytes());
            expected.extend_from_slice(content);
        }
        // Structural: all three fields of all three entries, not one field of
        // three. `bytes` in particular was unpinned — a constant 0 passed the
        // whole module.
        assert_eq!(
            result.inventory,
            vec![
                PackedFile {
                    path: "app/screens/detail.jsx".to_string(),
                    bytes: 36,
                    sha256: "ab39963930301457d73670d25561b095ff1b762ba2ba02f34cd1dd64715c038f"
                        .to_string(),
                },
                PackedFile {
                    path: "app/screens/home.jsx".to_string(),
                    bytes: 34,
                    sha256: "bd0b70df8dd27bd9ef83b144cfd64a1b86b9724ed27151c10cead147cd508880"
                        .to_string(),
                },
                PackedFile {
                    path: "plugin.json".to_string(),
                    bytes: 18,
                    sha256: "c04ab18e6ea42f6580a0a9f8da5d560d5e5a5ee13c45d27263e242befa64671d"
                        .to_string(),
                },
            ]
        );

        assert_eq!(expected.len(), 177);
        assert_eq!(result.archive, expected);
        assert_eq!(
            result.archive_digest,
            "7e4185d81113f671887d8bcaf013c46cc232b1bf3103e731e120a76bd0006f04"
        );
    }

    /// The digest must be sensitive to WHERE a file sits, not only to its
    /// bytes: two roots holding byte-identical content under different names
    /// are different packages.
    #[test]
    fn the_same_bytes_at_a_different_path_change_the_digest() {
        let a = tempfile::tempdir().unwrap();
        write_file(a.path(), "one.txt", b"same bytes");
        let ra = pack(a.path(), &[InventoryEntry::new("one.txt")]).unwrap();

        let b = tempfile::tempdir().unwrap();
        write_file(b.path(), "two.txt", b"same bytes");
        let rb = pack(b.path(), &[InventoryEntry::new("two.txt")]).unwrap();

        // Positive control: the CONTENT hash is identical on both sides, so a
        // difference below can only come from the path.
        assert_eq!(ra.inventory[0].sha256, rb.inventory[0].sha256);
        assert_ne!(ra.archive_digest, rb.archive_digest);
    }

    /// An entry that exists but cannot be read is not "missing". Reporting it
    /// as [`PackerError::MissingOnDisk`] sends a caller hunting for a file
    /// that is right there — a rejection erroring for the wrong reason still
    /// passes an `is_err()`-shaped test.
    #[test]
    fn an_unreadable_inventory_entry_reports_io_not_missing() {
        let tmp = tempfile::tempdir().unwrap();
        // An EMPTY directory: the walk finds no files under it (so this is not
        // a FileOutsideInventory case), but `fs::read` on it fails with
        // something other than NotFound.
        fs::create_dir_all(tmp.path().join("a-directory")).unwrap();
        let inventory = vec![InventoryEntry::new("a-directory")];

        let err = pack(tmp.path(), &inventory).unwrap_err();
        match err {
            PackerError::Io { path, detail } => {
                assert_eq!(path, "a-directory");
                assert!(!detail.is_empty(), "Io error must carry the reason");
            }
            other => panic!("expected Io naming the entry, got {other:?}"),
        }
    }

    #[test]
    fn missing_inventory_entry_fails_the_build() {
        let tmp = tempfile::tempdir().unwrap();
        let inventory = vec![InventoryEntry::new("never-written.txt")];
        let err = pack(tmp.path(), &inventory).unwrap_err();
        assert_eq!(
            err,
            PackerError::MissingOnDisk("never-written.txt".to_string())
        );
    }

    #[test]
    fn duplicate_inventory_path_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        write_file(tmp.path(), "a.txt", b"x");
        let inventory = vec![InventoryEntry::new("a.txt"), InventoryEntry::new("a.txt")];
        let err = pack(tmp.path(), &inventory).unwrap_err();
        assert_eq!(err, PackerError::DuplicatePath("a.txt".to_string()));
    }

    #[test]
    fn traversal_inventory_path_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let inventory = vec![InventoryEntry::new("../escape.txt")];
        let err = pack(tmp.path(), &inventory).unwrap_err();
        assert_eq!(err, PackerError::InvalidPath("../escape.txt".to_string()));
    }

    #[test]
    fn expected_sha256_mismatch_is_detected() {
        let tmp = tempfile::tempdir().unwrap();
        write_file(tmp.path(), "a.txt", b"actual content");
        let inventory = vec![InventoryEntry::with_sha256("a.txt", "0".repeat(64))];
        let err = pack(tmp.path(), &inventory).unwrap_err();
        match err {
            PackerError::HashMismatch {
                path,
                expected,
                actual,
            } => {
                assert_eq!(path, "a.txt");
                assert_eq!(expected, "0".repeat(64));
                // …and that `actual` reports the hash actually read, so the
                // message points at the real content rather than any value.
                assert_eq!(actual, sha256_hex(b"actual content"));
            }
            other => panic!("expected HashMismatch, got {other:?}"),
        }
    }

    #[test]
    fn a_file_declared_but_pruned_is_ignored_even_if_present() {
        // Defensive case: if an inventory ever DID list a path under a
        // pruned directory, pruning still wins — it is neither packed nor
        // required to exist on disk.
        let tmp = tempfile::tempdir().unwrap();
        write_file(tmp.path(), "a.txt", b"kept");
        write_file(tmp.path(), "node_modules/whatever.js", b"ignored");
        let inventory = vec![
            InventoryEntry::new("a.txt"),
            InventoryEntry::new("node_modules/whatever.js"),
        ];
        let result = pack(tmp.path(), &inventory).unwrap();
        assert_eq!(result.inventory.len(), 1);
        assert_eq!(result.inventory[0].path, "a.txt");
    }
}
