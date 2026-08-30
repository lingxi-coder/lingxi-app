//! 校验仓库里签入的 plugin workflow 脚本能通过**运行时真正用的**那两个校验器。
//!
//! ## 为什么这个测试住在 `workflow` crate 里
//!
//! `validate_meta` 和 `check_determinism` 是这里的 `pub fn`，而被校验的脚本是
//! `plugins/lingxi-local-app/workflows/*.js`。放在别处就得把校验器再导出一层，
//! 或者复制一份判据——而判据复制出第二份的那一刻，它就开始漂移了。
//!
//! ## 为什么它必须存在（这不是假设，是已经发生过一次的事）
//!
//! 这三个脚本第一次交付时，**全部三个都通不过 `validate_meta`**：
//!
//! ```text
//! meta must be a pure literal: non-literal node type in meta: BinaryExpression
//! ```
//!
//! 原因是 `description` 写成了 `'…' + '…'` 的多行字符串拼接。那是**语法完全合法
//! 的 JavaScript**，所以 `node --check` 三个都是绿的——交付报告里也确实写了
//! 「all three scripts pass `node --check`」。
//!
//! 更值得记的是：`local-app-build.js` 的注释里**引用了这个校验器**，写着
//! 「`export const meta` parses as the engine's required first-statement literal
//! (`workflow/src/lib.rs`, `validate_meta`)」。它**引用了判据，却没有运行判据**。
//!
//! ⛔ 所以 `node --check` 对 workflow 脚本是**假绿**。判据是这两个函数，不是
//! 语法解析器。
#![allow(clippy::needless_raw_string_hashes)]

use std::path::{Path, PathBuf};

/// 签入的 plugin workflow 脚本目录。
fn workflow_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../plugins/lingxi-local-app/workflows")
}

#[test]
fn every_checked_in_plugin_workflow_passes_the_runtime_validators() {
    let dir = workflow_dir();
    assert!(
        dir.is_dir(),
        "plugin workflow directory missing at {} — refusing to report a clean result from a \
         directory that does not exist",
        dir.display()
    );

    let mut scripts: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("read workflow dir")
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|ext| ext == "js"))
        .collect();
    scripts.sort();

    // Fail closed. 一个扫到零个文件的门和没有门是同一件事,但它看起来像通过了。
    assert!(
        !scripts.is_empty(),
        "enumerated ZERO .js workflow scripts under {} — refusing to report clean from an empty \
         enumeration",
        dir.display()
    );

    let mut failures = Vec::new();
    for script in &scripts {
        let name = script.file_name().unwrap_or_default().to_string_lossy().to_string();
        let src = match std::fs::read_to_string(script) {
            Ok(src) => src,
            Err(error) => {
                failures.push(format!("{name}: unreadable ({error})"));
                continue;
            }
        };
        if let Err(error) = workflow::validate_meta(&src) {
            failures.push(format!(
                "{name}: validate_meta rejected it — {error:?}. `meta` must be a PURE LITERAL; \
                 a concatenated description (`'a' + 'b'`) is valid JavaScript and passes \
                 `node --check`, but parses as a BinaryExpression and is refused here."
            ));
        }
        if let Err(error) = workflow::check_determinism(&src) {
            failures.push(format!(
                "{name}: check_determinism rejected it — {error:?}. Date.now(), Math.random() \
                 and zero-arg new Date() break resume, because the journal replays prior agent() \
                 results but re-runs the JS body."
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "{} of {} checked-in plugin workflow script(s) fail the runtime validators:\n  - {}",
        failures.len(),
        scripts.len(),
        failures.join("\n  - ")
    );
}
