//! M7-14 snapshot: the memory tier selector.

use iocraft::prelude::*;
use lingxi_tui::screens::memory::{MemoryScreen, MemoryTierEntry};

#[test]
fn snapshot_memory_selector() {
    let tiers = vec![
        MemoryTierEntry {
            label: "Project memory".into(),
            description: "Checked in at ./CLAUDE.md".into(),
            path: "/repo/CLAUDE.md".into(),
            exists: true,
        },
        MemoryTierEntry {
            label: "User memory".into(),
            description: "Saved in ~/.claude/CLAUDE.md".into(),
            path: "/home/.claude/CLAUDE.md".into(),
            exists: false,
        },
    ];
    let mut el = element! {
        MemoryScreen(tiers: tiers, selected: 0usize, editing: false,
            buffer: String::new(), dirty: false, status: None)
    };
    let out = el.to_string();
    insta::assert_snapshot!(out);
}
