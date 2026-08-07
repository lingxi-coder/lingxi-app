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
            return Err(reject(format!("duplicate path `{}` in one batch", write.path)));
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

fn screen_path(path: &str) -> Result<(), AppError> {
    if path.is_empty() {
        return Err(reject("empty write path"));
    }
    // 反斜杠先于任何 `/` 分段判断处理：否则 `app\..\secret.js` 会被
    // 当成单个合法分段混过去。
    if path.contains('\\') {
        return Err(reject(format!("`{path}` contains a backslash")));
    }
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
    for segment in path.split('/') {
        if segment.is_empty() || segment == "." || segment == ".." {
            return Err(reject(format!("`{path}` contains a `{segment}` segment")));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &str) -> FileWrite {
        FileWrite { path: path.into(), contents: "export default function P(){return null}".into() }
    }

    #[test]
    fn accepts_writes_under_every_writable_root() {
        let writes = vec![
            write("app/page.jsx"),
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
    }

    #[test]
    fn rejects_a_windows_style_separator() {
        screen_writes(&[write("app\\..\\secret.js")])
            .expect_err("backslashes must not smuggle a traversal past a `/`-only check");
    }

    #[test]
    fn rejects_the_lockfile_and_manifest() {
        screen_writes(&[write("package.json")]).expect_err("package.json is locked");
        screen_writes(&[write("package-lock.json")]).expect_err("the lockfile is locked");
    }

    #[test]
    fn rejects_a_duplicate_path_within_one_batch() {
        screen_writes(&[write("app/page.jsx"), write("app/page.jsx")])
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
            path: "app/page.jsx".into(),
            contents: "x".repeat(MAX_GENERATED_FILE_BYTES + 1),
        };
        screen_writes(&[big]).expect_err("a single file is capped");
    }

    #[test]
    fn rejects_a_batch_over_the_total_byte_cap() {
        let each = MAX_GENERATED_FILE_BYTES;
        let count = MAX_GENERATED_TOTAL_BYTES / each + 1;
        let writes: Vec<_> = (0..count)
            .map(|i| FileWrite { path: format!("app/p{i}.jsx"), contents: "x".repeat(each) })
            .collect();
        screen_writes(&writes).expect_err("the batch total is capped");
    }

    #[test]
    fn rejects_an_empty_batch() {
        screen_writes(&[]).expect_err("a generation that writes nothing is a failure, not a success");
    }
}
