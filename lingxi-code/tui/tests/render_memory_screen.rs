//! M7-14 snapshot: the memory tier selector.

use iocraft::prelude::*;
use tui::screens::memory::{MemoryScreen, MemoryTierEntry};

#[test]
fn snapshot_memory_selector() {
    let tiers = vec![
        MemoryTierEntry {
            label: "Project memory".into(),
            description: "Checked in at ./LINGXI.md".into(),
            path: "/repo/LINGXI.md".into(),
            exists: true,
        },
        MemoryTierEntry {
            label: "User memory".into(),
            description: "Saved in ~/.lingxi/LINGXI.md".into(),
            path: "/home/.lingxi/LINGXI.md".into(),
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
