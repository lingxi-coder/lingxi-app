//! Memory file editor screen (M7-14).
//!
//! A `MemoryFileSelector` lists the project/user CLAUDE.md tiers (resolved
//! via [`lingxi_memory::claude_md::hierarchy::walk`]); selecting a tier
//! opens an inline edit view. Reads go through the M3 loader
//! ([`lingxi_memory::claude_md::loader::load_file`]); writes go back to the
//! same on-disk path (the M3 store — §4 R7, no new persistence).
