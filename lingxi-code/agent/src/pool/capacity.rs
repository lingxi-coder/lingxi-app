//! Pool-local admission. A reservation is capacity, not an allocated agent.

use platform_api::panel_pool::PanelAdmissionCancellation as CancellationToken;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{watch, OwnedSemaphorePermit, Semaphore};
use tokio::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AdmissionError {
    InvalidCount,
    Full,
    QueueFull,
    Cancelled,
    Deadline,
    Closed,
}

#[derive(Default)]
struct State {
    queue: VecDeque<(u64, u32)>,
    next_id: u64,
    generation: u64,
    closed: bool,
}

pub(crate) struct CapacityCore {
    semaphore: Arc<Semaphore>,
    max: usize,
    state: Mutex<State>,
    changed: watch::Sender<u64>,
}

pub(crate) struct TrackedPoolPermit {
    owned: Option<OwnedSemaphorePermit>,
    core: Arc<CapacityCore>,
    group: Option<Arc<PoolGroupDrain>>,
}

pub(crate) struct PoolGroupDrain {
    pending: AtomicUsize,
    changed: tokio::sync::Notify,
}

impl PoolGroupDrain {
    pub(crate) fn new(count: usize) -> Arc<Self> {
        Arc::new(Self {
            pending: AtomicUsize::new(count),
            changed: tokio::sync::Notify::new(),
        })
    }
}

#[async_trait::async_trait]
impl platform_api::panel_pool::PanelPoolDrain for PoolGroupDrain {
    async fn wait(&self) {
        loop {
            let changed = self.changed.notified();
            if self.pending.load(Ordering::Acquire) == 0 {
                return;
            }
            changed.await;
        }
    }
}

impl TrackedPoolPermit {
    pub(crate) fn track_group(&mut self, group: Arc<PoolGroupDrain>) {
        assert!(
            self.group.is_none(),
            "physical slot already belongs to a group"
        );
        self.group = Some(group);
    }
}

struct Waiter {
    core: Arc<CapacityCore>,
    id: u64,
}

impl CapacityCore {
    pub(crate) fn new(max: usize) -> Result<Arc<Self>, AdmissionError> {
        if max > Semaphore::MAX_PERMITS {
            return Err(AdmissionError::InvalidCount);
        }
        Ok(Arc::new(Self {
            semaphore: Arc::new(Semaphore::new(max)),
            max,
            state: Mutex::new(State::default()),
            changed: watch::channel(0).0,
        }))
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|poisoned| {
            let mut state = poisoned.into_inner();
            state.closed = true;
            self.semaphore.close();
            self.changed.send_replace(state.generation);
            state
        })
    }

    fn notify(&self, state: &mut State) {
        match state.generation.checked_add(1) {
            Some(next) => state.generation = next,
            None => {
                state.closed = true;
                self.semaphore.close();
            }
        }
        self.changed.send_replace(state.generation);
    }

    pub(crate) fn max(&self) -> usize {
        self.max
    }

    pub(crate) fn available_permits(&self) -> usize {
        self.semaphore.available_permits()
    }

    pub(crate) fn owns(self: &Arc<Self>, permit: &TrackedPoolPermit) -> bool {
        Arc::ptr_eq(self, &permit.core)
    }

    pub(crate) fn acquire_ordinary(self: &Arc<Self>) -> Result<TrackedPoolPermit, AdmissionError> {
        let state = self.state();
        if state.closed {
            return Err(AdmissionError::Closed);
        }
        let protected = state.queue.front().map_or(0, |(_, count)| *count as usize);
        if self.available_permits() <= protected {
            return Err(AdmissionError::Full);
        }
        let owned = self
            .semaphore
            .clone()
            .try_acquire_owned()
            .map_err(|_| AdmissionError::Full)?;
        drop(state);
        Ok(TrackedPoolPermit {
            owned: Some(owned),
            core: self.clone(),
            group: None,
        })
    }

    pub(crate) async fn reserve_group(
        self: &Arc<Self>,
        count: usize,
        overall_deadline: Instant,
        cancel: CancellationToken,
    ) -> Result<Vec<TrackedPoolPermit>, AdmissionError> {
        if count == 0 || count > self.max || count > u32::MAX as usize {
            return Err(AdmissionError::InvalidCount);
        }
        let deadline = overall_deadline.min(Instant::now() + Duration::from_secs(30));
        let mut changed = self.changed.subscribe();
        let waiter = {
            let mut state = self.state();
            if state.closed {
                return Err(AdmissionError::Closed);
            }
            if cancel.is_cancelled() {
                return Err(AdmissionError::Cancelled);
            }
            if Instant::now() >= deadline {
                return Err(AdmissionError::Deadline);
            }
            if state.queue.len() == 16 {
                return Err(AdmissionError::QueueFull);
            }
            let id = state.next_id;
            state.next_id = match id.checked_add(1) {
                Some(next) => next,
                None => {
                    state.closed = true;
                    self.semaphore.close();
                    self.changed.send_replace(state.generation);
                    return Err(AdmissionError::Closed);
                }
            };
            state.queue.push_back((id, count as u32));
            Waiter {
                core: self.clone(),
                id,
            }
        };
        loop {
            if cancel.is_cancelled() {
                return Err(AdmissionError::Cancelled);
            }
            if Instant::now() >= deadline {
                return Err(AdmissionError::Deadline);
            }
            let acquired = {
                let mut state = self.state();
                if state.closed {
                    return Err(AdmissionError::Closed);
                }
                if state.queue.front().is_some_and(|(id, _)| *id == waiter.id) {
                    // The mutex may have delayed this waiter after its outer checks.
                    if cancel.is_cancelled() {
                        return Err(AdmissionError::Cancelled);
                    }
                    if Instant::now() >= deadline {
                        return Err(AdmissionError::Deadline);
                    }
                    match self.semaphore.clone().try_acquire_many_owned(count as u32) {
                        Ok(owned) => {
                            state.queue.pop_front();
                            self.notify(&mut state);
                            Some(owned)
                        }
                        Err(_) => None,
                    }
                } else {
                    None
                }
            };
            if let Some(mut owned) = acquired {
                if self.state().closed {
                    drop(owned);
                    return Err(AdmissionError::Closed);
                }
                // Split outside the book lock: tracked destructors reenter it.
                let permits = (0..count)
                    .map(|_| TrackedPoolPermit {
                        owned: Some(owned.split(1).expect("whole group owns every slot")),
                        core: self.clone(),
                        group: None,
                    })
                    .collect();
                return Ok(permits);
            }
            tokio::select! {
                biased;
                _ = cancel.cancelled() => return Err(AdmissionError::Cancelled),
                _ = tokio::time::sleep_until(deadline) => return Err(AdmissionError::Deadline),
                result = changed.changed() => {
                    if result.is_err() { return Err(AdmissionError::Closed); }
                }
            }
        }
    }
}

impl Drop for Waiter {
    fn drop(&mut self) {
        let mut state = self.core.state();
        if let Some(index) = state.queue.iter().position(|(id, _)| *id == self.id) {
            state.queue.remove(index);
            self.core.notify(&mut state);
        }
    }
}

impl Drop for TrackedPoolPermit {
    fn drop(&mut self) {
        // Capacity is observable before the generation is published.
        drop(self.owned.take());
        self.core.notify(&mut self.core.state());
        if let Some(group) = &self.group {
            if group.pending.fetch_sub(1, Ordering::AcqRel) == 1 {
                group.changed.notify_waiters();
            }
        }
    }
}

#[cfg(test)]
#[path = "capacity/tests.rs"]
mod tests;
