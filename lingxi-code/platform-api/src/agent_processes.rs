//! Host-independent ownership of live OS processes.
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

fn groups() -> &'static Mutex<HashMap<String, HashMap<u32, u64>>> {
    static GROUPS: OnceLock<Mutex<HashMap<String, HashMap<u32, u64>>>> = OnceLock::new();
    GROUPS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// One registration from an owner snapshot; the token is local to this host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegistrationEntry {
    pub pid: u32,
    pub token: u64,
}

pub struct Registration {
    owner: String,
    entry: RegistrationEntry,
}
impl Registration {
    pub fn entry(&self) -> RegistrationEntry {
        self.entry
    }
}
impl Drop for Registration {
    fn drop(&mut self) {
        let mut owners = groups()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(pids) = owners.get_mut(&self.owner) {
            if pids.get(&self.entry.pid) == Some(&self.entry.token) {
                pids.remove(&self.entry.pid);
            }
            if pids.is_empty() {
                owners.remove(&self.owner);
            }
        }
    }
}

pub fn register(owner: Option<&str>, pid: Option<u32>) -> Option<Registration> {
    let (owner, pid) = (owner?, pid?);
    static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);
    let token = NEXT_TOKEN
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
            value.checked_add(1)
        })
        .expect("process registration tokens exhausted");
    groups()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .entry(owner.to_owned())
        .or_default()
        .insert(pid, token);
    Some(Registration {
        owner: owner.to_owned(),
        entry: RegistrationEntry { pid, token },
    })
}

/// Capture identities once before an asynchronous owner-stop operation.
pub fn snapshot_entries(owner: &str) -> Vec<RegistrationEntry> {
    groups()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(owner)
        .into_iter()
        .flat_map(|pids| {
            pids.iter()
                .map(|(&pid, &token)| RegistrationEntry { pid, token })
        })
        .collect()
}

/// Recheck a direct-process snapshot before falling back to native signaling.
pub fn is_current(owner: &str, entry: RegistrationEntry) -> bool {
    groups()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(owner)
        .and_then(|pids| pids.get(&entry.pid))
        .copied()
        == Some(entry.token)
}

/// Snapshot the live registered PIDs for one owner without consuming them.
pub fn snapshot(owner: &str) -> Vec<u32> {
    snapshot_entries(owner)
        .into_iter()
        .map(|entry| entry.pid)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reused_pid_keeps_new_registration_when_old_guard_drops() {
        let owner = "registration-generation-reuse";
        let old = register(Some(owner), Some(1234)).unwrap();
        let stale = old.entry();
        let new = register(Some(owner), Some(1234)).unwrap();
        assert_ne!(stale, new.entry());
        assert!(!is_current(owner, stale));
        assert!(is_current(owner, new.entry()));
        drop(old);
        assert_eq!(snapshot_entries(owner), vec![new.entry()]);
        drop(new);
        assert!(snapshot(owner).is_empty());
    }
}
