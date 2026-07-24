//! Session-scoped app-allowlist + grant-flags model.
//!
//! Parity target: the binary's tiered per-app permission surface (`read` /
//! `click` / `full`) plus the three opt-in capability grants
//! (`clipboardRead` / `clipboardWrite` / `systemKeyCombos`). Held inside
//! [`crate::ComputerTool`] itself (not threaded through the shared
//! `BuiltinToolContext`/`AppState`) — nothing outside this crate needs it,
//! and the tool instance already lives for the whole session (constructed
//! once at registration), so a private `Mutex` here is the minimal seam.

use std::collections::HashMap;

/// Per-app grant level. Ordered: `Read < Click < Full` (matches the binary's
/// escalating capability set — each tier is a strict superset of the one
/// below).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AppTier {
    /// Visible in screenshots only — no clicks or typing.
    Read,
    /// Plain clicks, scroll, drag, cursor queries. No right-click,
    /// modifier-clicks, or keyboard input.
    Click,
    /// Full interaction: right-click, modifier-clicks, typing, key presses.
    Full,
}

impl AppTier {
    /// Byte-faithful tier name as used in `list_granted_applications` output
    /// and tier-insufficient error messages.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            AppTier::Read => "read",
            AppTier::Click => "click",
            AppTier::Full => "full",
        }
    }
}

/// One allowlisted application.
#[derive(Debug, Clone)]
pub struct AllowedApp {
    /// Bundle id (or raw name, when the caller didn't supply a resolvable
    /// bundle id and no backend was available to resolve one).
    pub bundle_id: String,
    /// Granted tier.
    pub tier: AppTier,
}

/// The three opt-in capability grants `request_access` can request alongside
/// the app allowlist (binary field names, verbatim: `clipboardRead` /
/// `clipboardWrite` / `systemKeyCombos`).
#[derive(Debug, Clone, Copy, Default)]
pub struct GrantFlags {
    /// Permission to read the clipboard (`read_clipboard`).
    pub clipboard_read: bool,
    /// Permission to write the clipboard (`write_clipboard`).
    pub clipboard_write: bool,
    /// Permission to send system-level key combos (quit app, switch app,
    /// lock screen, …) via `key`/`hold_key`.
    pub system_key_combos: bool,
}

impl GrantFlags {
    /// Field-accessor form for use with [`crate::ComputerTool::require_grant_flag`]'s
    /// generic getter parameter.
    #[must_use]
    pub fn clipboard_read_enabled(self) -> bool {
        self.clipboard_read
    }
    /// See [`Self::clipboard_read_enabled`].
    #[must_use]
    pub fn clipboard_write_enabled(self) -> bool {
        self.clipboard_write
    }
}

/// Full session-scoped computer-use state for one `ComputerTool` instance.
#[derive(Debug, Default)]
pub struct SessionState {
    /// Bundle id → tier, insertion order preserved for stable
    /// `list_granted_applications` output.
    apps: HashMap<String, AppTier>,
    /// Insertion order, mirrored alongside `apps` (a `HashMap` alone doesn't
    /// preserve it).
    order: Vec<String>,
    /// Materialized `AllowedApp` list, kept in sync by [`Self::grant_app`].
    pub allowed_apps: Vec<AllowedApp>,
    /// Active capability grants.
    pub grant_flags: GrantFlags,
    /// The display id pinned by `switch_display`, or `None` for automatic
    /// selection.
    pub selected_display: Option<u32>,
    /// The monitor NAME last passed to `switch_display` (before it's
    /// resolved to a concrete id against a live display list). Surfaced via
    /// `list_granted_applications`-style introspection.
    pub pinned_display_name: Option<String>,
    /// Whether `left_mouse_down` has fired without a matching
    /// `left_mouse_up` yet (guards the "already held" `state_conflict`).
    pub mouse_button_held: bool,
}

impl SessionState {
    /// Grant (or upgrade) one app to `tier`. Never downgrades an existing
    /// grant — matches upstream's "previously granted apps remain granted"
    /// `request_access` contract (a second call adds/upgrades, never revokes).
    pub fn grant_app(&mut self, bundle_id: String, tier: AppTier) {
        let entry = self.apps.entry(bundle_id.clone()).or_insert(tier);
        if tier > *entry {
            *entry = tier;
        }
        if !self.order.contains(&bundle_id) {
            self.order.push(bundle_id);
        }
        self.rebuild_allowed_apps();
    }

    /// Merge newly-requested grant flags into the session's active set —
    /// truthy-only, matching upstream's dedupe+merge (a flag once granted
    /// stays granted; a later request that omits it doesn't revoke it).
    pub fn merge_grant_flags(&mut self, flags: GrantFlags) {
        self.grant_flags.clipboard_read |= flags.clipboard_read;
        self.grant_flags.clipboard_write |= flags.clipboard_write;
        self.grant_flags.system_key_combos |= flags.system_key_combos;
    }

    /// The granted tier for `bundle_id`, if any.
    #[must_use]
    pub fn tier_for(&self, bundle_id: &str) -> Option<AppTier> {
        self.apps.get(bundle_id).copied()
    }

    /// Pin the selected display by name. Resolving the name to a concrete
    /// display id happens at the next `screenshot`/`zoom` call against the
    /// live display list (this just records the pin request) — needs a live
    /// backend, which this module doesn't depend on, so `selected_display`
    /// stays `None` (auto) until a resolution pass fills it in.
    pub fn pin_display_by_name(&mut self, name: &str) {
        self.pinned_display_name = Some(name.to_string());
    }

    fn rebuild_allowed_apps(&mut self) {
        self.allowed_apps = self
            .order
            .iter()
            .map(|id| AllowedApp {
                bundle_id: id.clone(),
                tier: self.apps[id],
            })
            .collect();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tier_ordering_matches_capability_escalation() {
        assert!(AppTier::Read < AppTier::Click);
        assert!(AppTier::Click < AppTier::Full);
    }

    #[test]
    fn grant_app_never_downgrades() {
        let mut s = SessionState::default();
        s.grant_app("com.foo".into(), AppTier::Full);
        s.grant_app("com.foo".into(), AppTier::Read);
        assert_eq!(s.tier_for("com.foo"), Some(AppTier::Full));
    }

    #[test]
    fn grant_app_upgrades_when_higher_tier_requested() {
        let mut s = SessionState::default();
        s.grant_app("com.foo".into(), AppTier::Click);
        s.grant_app("com.foo".into(), AppTier::Full);
        assert_eq!(s.tier_for("com.foo"), Some(AppTier::Full));
    }

    #[test]
    fn merge_grant_flags_is_truthy_only_union() {
        let mut s = SessionState::default();
        s.merge_grant_flags(GrantFlags {
            clipboard_read: true,
            clipboard_write: false,
            system_key_combos: false,
        });
        s.merge_grant_flags(GrantFlags {
            clipboard_read: false,
            clipboard_write: true,
            system_key_combos: false,
        });
        assert!(s.grant_flags.clipboard_read);
        assert!(s.grant_flags.clipboard_write);
        assert!(!s.grant_flags.system_key_combos);
    }

    #[test]
    fn allowed_apps_preserves_insertion_order() {
        let mut s = SessionState::default();
        s.grant_app("com.b".into(), AppTier::Full);
        s.grant_app("com.a".into(), AppTier::Click);
        let ids: Vec<_> = s.allowed_apps.iter().map(|a| a.bundle_id.as_str()).collect();
        assert_eq!(ids, vec!["com.b", "com.a"]);
    }
}
