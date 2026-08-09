//! LLM 写盘产物的闸门。
//!
//! 纯函数，零 I/O：整批要么全过要么全拒，**不做部分写入**——一半新
//! 一半旧的工作区比干脆失败更难诊断。通过后仍要交给
//! `local_apps::validate_workspace_source` 做完整策略校验；本模块只
//! 负责在文件落地之前把明显越界的东西挡在外面。

use local_apps::{AppError, WRITABLE_ROOTS};

/// 一个待写文件。路径是相对工作区根的 POSIX 路径。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileWrite {
    pub path: String,
    pub contents: String,
}

pub const MAX_GENERATED_FILES: usize = 60;
pub const MAX_GENERATED_FILE_BYTES: usize = 256 * 1024;
pub const MAX_GENERATED_TOTAL_BYTES: usize = 4 * 1024 * 1024;

fn reject(message: impl Into<String>) -> AppError {
    AppError::InvalidRequest(message.into())
}

/// 逐条筛查一批写盘请求。
pub fn screen_writes(writes: &[FileWrite]) -> Result<(), AppError> {
    if writes.is_empty() {
        return Err(reject("the generator produced no files"));
    }
    if writes.len() > MAX_GENERATED_FILES {
        return Err(reject(format!(
            "the generator produced {} files, the limit is {MAX_GENERATED_FILES}",
            writes.len()
        )));
    }
    let mut total = 0usize;
    let mut seen = std::collections::BTreeSet::new();
    for write in writes {
        screen_path(&write.path)?;
        if !seen.insert(write.path.as_str()) {
            return Err(reject(format!(
                "duplicate path `{}` in one batch",
                write.path
            )));
        }
        let bytes = write.contents.len();
        if bytes > MAX_GENERATED_FILE_BYTES {
            return Err(reject(format!(
                "`{}` is {bytes} bytes, the per-file limit is {MAX_GENERATED_FILE_BYTES}",
                write.path
            )));
        }
        total += bytes;
    }
    if total > MAX_GENERATED_TOTAL_BYTES {
        return Err(reject(format!(
            "the batch is {total} bytes, the limit is {MAX_GENERATED_TOTAL_BYTES}"
        )));
    }
    Ok(())
}

/// Template files the generator owns outright. They are LOCKED (rewritten and
/// hash-checked on every job), so a model write here is refused BEFORE it
/// lands: letting it through would burn a repair attempt on a validator
/// rejection the screen can see coming.
pub(crate) const LOCKED_TEMPLATE_PATHS: &[&str] =
    &["lib/lingxi-bridge.js", "app/layout.jsx", "app/page.jsx"];

fn screen_path(path: &str) -> Result<(), AppError> {
    if path.is_empty() {
        return Err(reject("empty write path"));
    }
    // Case-INSENSITIVE: iOS and macOS resolve `lib/LingXi-Bridge.js` to the
    // same file as `lib/lingxi-bridge.js`, so a case-sensitive screen would
    // let a differently-cased write through to overwrite the host's own
    // helper — and then die at the hash check a repair attempt later, which
    // is exactly what screening early exists to avoid.
    if LOCKED_TEMPLATE_PATHS
        .iter()
        .any(|locked| locked.eq_ignore_ascii_case(path))
    {
        return Err(reject(format!(
            "`{path}` is provided by the host and cannot be written by the app"
        )));
    }
    // 反斜杠检查独立于下面的 `/` 分段判断，且不依赖顺序：`contains('\\')`
    // 扫描的是完整原始字符串，不受 `split('/')` 影响。它存在的理由不是
    // "抢在分段之前跑"（`split` 不消费 `path`，顺序其实无关），而是分段
    // 判断本身认不出反斜杠——`app/x\..\..\secret.js` 这类混合输入里，
    // 根段 `app` 合法、`x\..\..\secret.js` 又不是字面的 `..`，逐段判断
    // 会放行；必须有这条独立检查才能挡住它。
    if path.contains('\\') {
        return Err(reject(format!("`{path}` contains a backslash")));
    }
    // Note for the next reader: given `WRITABLE_ROOTS` never contains `""`,
    // the root-membership check a few lines down independently rejects
    // every leading-`/` path anyway (`"/app/x".split('/')` yields `""` as
    // the root token), so this branch is not reachable-as-necessary against
    // the current parser — verified by deleting it and re-running the
    // absolute-path tests, which stayed green. It stays as defense-in-depth
    // (a clearer, dedicated error message, and a guard against a future
    // change to the root/segment parsing — e.g. filtering out empty
    // segments to tolerate `//` — quietly turning a leading `/` into a
    // no-op).
    if path.starts_with('/') {
        return Err(reject(format!("`{path}` is absolute")));
    }
    let mut segments = path.split('/');
    let Some(root) = segments.next() else {
        return Err(reject(format!("`{path}` has no root segment")));
    };
    if !WRITABLE_ROOTS.contains(&root) {
        return Err(reject(format!(
            "`{path}` is outside the writable roots {WRITABLE_ROOTS:?}"
        )));
    }
    // `segments` now holds everything after the root (it was already
    // advanced once above), so this reuses one split instead of taking a
    // second pass over `path`. A write that names only the root itself
    // (e.g. `"app"`) must still be rejected: it would collide with the
    // directory `AppLayout::initialize()` scaffolds at that root.
    let mut has_child = false;
    for segment in segments {
        has_child = true;
        if segment.is_empty() || segment == "." || segment == ".." {
            return Err(reject(format!("`{path}` contains a `{segment}` segment")));
        }
    }
    if !has_child {
        return Err(reject(format!(
            "`{path}` writes directly to the root `{root}`, not a file beneath it"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_to_host_owned_source_are_refused() {
        for path in LOCKED_TEMPLATE_PATHS {
            let error = screen_writes(&[write(path)]).expect_err("the host owns this file");
            assert!(
                error.to_string().contains("provided by the host"),
                "{error}"
            );
        }
        // A sibling under the same root stays writable — the guard is one
        // path, not a ban on `lib/`.
        screen_writes(&[write("lib/format.js")]).expect("other lib files are the app's own");
    }

    /// The screen and the scaffold must agree on which files the host owns.
    /// If a path is added to one list and not the other, either the model
    /// burns a repair attempt on a hash mismatch (screened too late) or a
    /// host file is silently unwritable for no reason (screened too early).
    #[test]
    fn the_screened_paths_are_exactly_the_locked_template_files_under_a_writable_root() {
        let locked: Vec<&str> = crate::local_apps_generation::LOCKED_FILES
            .iter()
            .map(|(path, _)| *path)
            .filter(|path| path.contains('/'))
            .collect();
        assert_eq!(
            LOCKED_TEMPLATE_PATHS, locked,
            "LOCKED_TEMPLATE_PATHS must track LOCKED_FILES' entries that live under a \
             writable root (root-level files like package.json are already rejected by \
             the writable-root check)"
        );
    }

    fn write(path: &str) -> FileWrite {
        FileWrite {
            path: path.into(),
            contents: "export default function P(){return null}".into(),
        }
    }

    #[test]
    fn accepts_writes_under_every_writable_root() {
        let writes = vec![
            write("app/settings/page.jsx"),
            write("components/NoteList.jsx"),
            write("lib/store.js"),
            write("styles/globals.css"),
            write("public/icon.svg"),
        ];
        screen_writes(&writes).expect("all five writable roots are allowed");
    }

    #[test]
    fn rejects_a_write_outside_the_writable_roots() {
        screen_writes(&[write("pages/index.jsx")]).expect_err("pages/ is not writable");
    }

    #[test]
    fn rejects_a_parent_traversal() {
        screen_writes(&[write("app/../../etc/passwd")]).expect_err("`..` is rejected");
    }

    #[test]
    fn rejects_an_absolute_path() {
        screen_writes(&[write("/etc/passwd")]).expect_err("absolute paths are rejected");
        // NOTE: this second case does NOT mutation-isolate the dedicated
        // `path.starts_with('/')` check in `screen_path`, and neither can any
        // other input. Verified by deleting that check and re-running this
        // test: it stayed green, because `"/app/page.jsx".split('/')` yields
        // `""` as the root token, and `""` is never in `WRITABLE_ROOTS` — the
        // root-membership check a few lines below independently rejects
        // every leading-`/` path on its own. Kept anyway (see the comment on
        // the check itself) as a behavioral requirement and defense-in-depth,
        // not because this test can prove the line is load-bearing.
        screen_writes(&[write("/app/page.jsx")]).expect_err(
            "a leading slash is rejected even when the rest of the path looks writable",
        );
    }

    #[test]
    fn rejects_a_windows_style_separator() {
        screen_writes(&[write("app\\..\\secret.js")])
            .expect_err("backslashes must not smuggle a traversal past a `/`-only check");
    }

    #[test]
    fn rejects_a_backslash_traversal_mixed_with_a_valid_root() {
        // Root segment `app` is legitimate and the offending segment is not
        // literally `..`, so this only fails if the backslash scan runs
        // independently of the per-segment `.`/`..` check.
        screen_writes(&[write("app/x\\..\\..\\secret.js")])
            .expect_err("a backslash-encoded traversal under a valid root is still rejected");
    }

    #[test]
    fn rejects_a_bare_root_name() {
        for root in ["app", "components", "lib", "styles", "public"] {
            screen_writes(&[write(root)]).expect_err(
                "a write naming only the root itself, with no child segment, is rejected",
            );
        }
    }

    #[test]
    fn rejects_the_lockfile_and_manifest() {
        screen_writes(&[write("package.json")]).expect_err("package.json is locked");
        screen_writes(&[write("package-lock.json")]).expect_err("the lockfile is locked");
    }

    #[test]
    fn rejects_a_duplicate_path_within_one_batch() {
        screen_writes(&[
            write("components/AppShell.jsx"),
            write("components/AppShell.jsx"),
        ])
        .expect_err("a batch that writes one path twice is ambiguous");
    }

    #[test]
    fn rejects_more_than_sixty_files() {
        let writes: Vec<_> = (0..61).map(|i| write(&format!("app/p{i}.jsx"))).collect();
        screen_writes(&writes).expect_err("the file count is capped");
    }

    #[test]
    fn rejects_a_single_file_over_the_byte_cap() {
        let big = FileWrite {
            path: "components/AppShell.jsx".into(),
            contents: "x".repeat(MAX_GENERATED_FILE_BYTES + 1),
        };
        screen_writes(&[big]).expect_err("a single file is capped");
    }

    #[test]
    fn rejects_a_batch_over_the_total_byte_cap() {
        let each = MAX_GENERATED_FILE_BYTES;
        let count = MAX_GENERATED_TOTAL_BYTES / each + 1;
        let writes: Vec<_> = (0..count)
            .map(|i| FileWrite {
                path: format!("app/p{i}.jsx"),
                contents: "x".repeat(each),
            })
            .collect();
        screen_writes(&writes).expect_err("the batch total is capped");
    }

    #[test]
    fn rejects_an_empty_batch() {
        screen_writes(&[])
            .expect_err("a generation that writes nothing is a failure, not a success");
    }
}
