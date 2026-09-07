//! Output scopes share the existing reservation book. These APIs are inert
//! until the host wires a scope provider into its turn/workflow lifecycle.
use super::*;
use platform_api::{
    BudgetError, WorkflowOutputAccount, WorkflowOutputEventId, WorkflowOutputScope,
    WorkflowOutputScopes,
};
use protocol::{MessageId, SessionId};
use std::sync::Weak;

pub(super) struct OutputScopeState {
    binding_id: MessageId,
    pub(super) spent: u64,
    pub(super) max_output_tokens: Option<u64>,
    generation_order: u64,
    legacy_events: HashMap<WorkflowOutputEventId, u64>,
    pub(super) snapshot: Arc<AtomicU64>,
    pub(super) owner: Weak<BudgetOutputAccount>,
    pub(super) attempts: HashMap<String, super::attempts::PublishedAttemptOutput>,
}

pub(super) struct BudgetOutputAccount {
    pub(super) session_id: SessionId,
    pub(super) generation_id: MessageId,
    pub(super) binding_id: MessageId,
    generation_order: u64,
    pub(super) session: Arc<BudgetSessionState>,
    pub(super) tracker: Arc<CostTracker>,
    snapshot: Arc<AtomicU64>,
}

fn output_error(gate: &crate::CostDurabilityGate, reason: &'static str) -> BudgetError {
    gate.freeze(reason);
    BudgetError::Internal(reason.into())
}

impl WorkflowOutputAccount for BudgetOutputAccount {
    fn session_id(&self) -> SessionId {
        self.session_id
    }
    fn generation_id(&self) -> MessageId {
        self.generation_id
    }
    fn spent(&self) -> u64 {
        self.snapshot.load(Ordering::Acquire)
    }

    fn record_legacy(
        &self,
        event: WorkflowOutputEventId,
        output_tokens: u64,
    ) -> Result<(), BudgetError> {
        let gate = self.tracker.durability_gate();
        let mut book = self.session.lock_reservations(&gate)?;
        let state = book
            .output_scopes
            .get_mut(&self.generation_id)
            .ok_or_else(|| output_error(&gate, "captured output scope disappeared"))?;
        if let Some(previous) = state.legacy_events.get(&event) {
            return if *previous == output_tokens {
                Ok(())
            } else {
                Err(output_error(&gate, "output event identity conflict"))
            };
        }
        let spent = state
            .spent
            .checked_add(output_tokens)
            .ok_or_else(|| output_error(&gate, "output accounting overflow"))?;
        // Actual output is retained even when it exceeds the turn target or
        // the session is already frozen; recording is not authorization.
        // All fallible checks precede these mutations and the derived store.
        state.legacy_events.insert(event, output_tokens);
        state.spent = spent;
        state.snapshot.store(spent, Ordering::Release);
        Ok(())
    }
}

struct BudgetOutputScopes {
    enforcer: BudgetEnforcer,
}

impl BudgetEnforcer {
    /// Resolve the exact process-local account before entering a persistence
    /// turn. Metadata equality alone never authenticates an output scope.
    pub(crate) async fn bind_attempt_budget(
        &self,
        scope: &WorkflowOutputScope,
    ) -> Result<super::BoundAttemptBudget, BudgetError> {
        // A custom account can execute code here; no accounting lock is held.
        let session_id = scope.session_id();
        let generation = scope.generation_id();
        if self
            .session_scope
            .is_some_and(|session| session != session_id)
        {
            return Err(BudgetError::Internal(
                "attempt output belongs to another session".into(),
            ));
        }
        let session = self.session_state_for(session_id).await;
        let tracker = self.cost_tracker.scoped(session_id);
        let account = {
            let book = session.lock_reservations(&tracker.durability_gate())?;
            book.output_scopes
                .get(&generation)
                .and_then(|state| state.owner.upgrade())
                .ok_or_else(|| {
                    BudgetError::Internal("attempt output account is not registered".into())
                })?
        };
        if !scope.shares_account(&WorkflowOutputScope::new(account.clone())) {
            return Err(BudgetError::Internal(
                "attempt output account authority mismatch".into(),
            ));
        }
        // Use the account's original tracker, not a newly hydrated cell with
        // the same session ID. Its writer lease remains pinned by the account.
        account
            .tracker
            .preflight_durable()
            .map_err(|error| BudgetError::Internal(error.to_string()))?;
        Ok(super::BoundAttemptBudget::new(account))
    }

    /// Build a neutral factory over this exact money/output reservation book.
    /// Multiple factory handles share current scopes; they do not create a
    /// separate budget ledger. Hosts must explicitly begin each turn.
    #[must_use]
    pub fn workflow_output_scopes(&self) -> Arc<dyn WorkflowOutputScopes> {
        Arc::new(BudgetOutputScopes {
            enforcer: Self {
                config: self.config.clone(),
                cost_tracker: self.cost_tracker.clone(),
                sessions: self.sessions.clone(),
                session_scope: self.session_scope,
            },
        })
    }
}

#[async_trait::async_trait]
impl WorkflowOutputScopes for BudgetOutputScopes {
    async fn begin_turn(
        &self,
        session_id: SessionId,
        generation_id: MessageId,
        max_output_tokens: Option<u64>,
    ) -> Result<WorkflowOutputScope, BudgetError> {
        if self
            .enforcer
            .session_scope
            .is_some_and(|session| session != session_id)
        {
            return Err(BudgetError::Internal(
                "output factory belongs to another session".into(),
            ));
        }
        let tracker = self.enforcer.cost_tracker.scoped(session_id);
        tracker
            .preflight_durable()
            .map_err(|error| BudgetError::Internal(error.to_string()))?;
        // This is a publication gate only, not a second accounting book. It
        // orders concurrent begin calls across their async session lookup.
        let _publication = self.enforcer.sessions.output_publication.lock().await;
        let session = self.enforcer.session_state_for(session_id).await;
        let account = {
            let mut book = session.lock_reservations(&tracker.durability_gate())?;
            if let Some(existing) = book.output_scopes.get(&generation_id) {
                if existing.max_output_tokens != max_output_tokens {
                    return Err(BudgetError::Internal(
                        "output generation changed its limit".into(),
                    ));
                }
            } else {
                let order = book.next_output_generation.checked_add(1).ok_or_else(|| {
                    BudgetError::Internal("output generation sequence exhausted".into())
                })?;
                let state = OutputScopeState {
                    binding_id: MessageId::new(),
                    spent: 0,
                    max_output_tokens,
                    generation_order: order,
                    legacy_events: HashMap::new(),
                    snapshot: Arc::new(AtomicU64::new(0)),
                    owner: Weak::new(),
                    attempts: HashMap::new(),
                };
                book.output_scopes.insert(generation_id, state);
                book.next_output_generation = order;
            }
            let state = book
                .output_scopes
                .get_mut(&generation_id)
                .expect("scope installed");
            if let Some(account) = state.owner.upgrade() {
                account
            } else {
                let account = Arc::new(BudgetOutputAccount {
                    session_id,
                    generation_id,
                    binding_id: state.binding_id,
                    generation_order: state.generation_order,
                    session: session.clone(),
                    tracker: tracker.clone(),
                    snapshot: state.snapshot.clone(),
                });
                state.owner = Arc::downgrade(&account);
                account
            }
        };
        let retired = {
            let mut current = self.enforcer.sessions.current_outputs.lock().map_err(|_| {
                output_error(&tracker.durability_gate(), "output scope registry poisoned")
            })?;
            if current
                .get(&session_id)
                .is_none_or(|previous| previous.generation_order <= account.generation_order)
            {
                current.insert(session_id, account.clone())
            } else {
                None
            }
        };
        // Dropping a final account may release a captured tracker/lease. Never
        // do that while holding the book or current-scope registry lock.
        drop(_publication);
        drop(retired);
        Ok(WorkflowOutputScope::new(account))
    }

    fn capture(&self, session_id: SessionId) -> Result<WorkflowOutputScope, BudgetError> {
        if self
            .enforcer
            .session_scope
            .is_some_and(|session| session != session_id)
        {
            return Err(BudgetError::Internal(
                "output factory belongs to another session".into(),
            ));
        }
        self.enforcer
            .sessions
            .current_outputs
            .lock()
            .map_err(|_| BudgetError::Internal("output scope registry poisoned".into()))?
            .get(&session_id)
            .cloned()
            .map(|account| WorkflowOutputScope::new(account))
            .ok_or_else(|| BudgetError::Internal("session has no published output turn".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enforcer() -> BudgetEnforcer {
        let (tx, _) = tokio::sync::mpsc::channel(4);
        let tracker = Arc::new(CostTracker::new(
            SessionId::new(),
            Arc::new(crate::PricingCatalog::empty()),
            tx,
        ));
        BudgetEnforcer::new(
            BudgetConfig {
                max_session_nano_usd: None,
                max_turn_nano_usd: None,
                max_turn_tokens: None,
                warning_thresholds: vec![],
                on_exceed: BudgetExceedPolicy::Halt,
            },
            tracker,
        )
    }

    #[tokio::test]
    async fn output_scope_factories_share_one_turn_and_identical_events_are_idempotent() {
        let enforcer = enforcer();
        let session = enforcer.session_id().await;
        let factory = enforcer.workflow_output_scopes();
        assert!(factory.capture(session).is_err());
        let generation = MessageId::new();
        let first = factory
            .begin_turn(session, generation, Some(100))
            .await
            .unwrap();
        let other = enforcer.workflow_output_scopes().capture(session).unwrap();
        assert!(first.shares_account(&other));
        let event = WorkflowOutputEventId::MainResponse(MessageId::new());
        first.record_legacy(event.clone(), 12).unwrap();
        other.record_legacy(event, 12).unwrap();
        assert_eq!(other.spent(), 12);
        let reused = factory
            .begin_turn(session, generation, Some(100))
            .await
            .unwrap();
        assert!(first.shares_account(&reused));
        assert_eq!(reused.spent(), 12);
        assert!(factory
            .begin_turn(session, generation, Some(200))
            .await
            .is_err());
        assert_eq!(factory.capture(session).unwrap().spent(), 12);
    }

    #[tokio::test]
    async fn output_scope_late_old_turn_and_session_never_mutate_the_new_scope() {
        let enforcer = enforcer();
        let session_a = enforcer.session_id().await;
        let factory = enforcer.workflow_output_scopes();
        let generation_a = MessageId::new();
        let a1 = factory
            .begin_turn(session_a, generation_a, None)
            .await
            .unwrap();
        let a2 = factory
            .begin_turn(session_a, MessageId::new(), None)
            .await
            .unwrap();
        let session_b = SessionId::new();
        let b = factory
            .begin_turn(session_b, MessageId::new(), None)
            .await
            .unwrap();
        a1.record_legacy(
            WorkflowOutputEventId::LegacyFusion(platform_api::FusionRunId::generated()),
            30,
        )
        .unwrap();
        assert_eq!(a1.spent(), 30);
        assert_eq!(a2.spent(), 0);
        assert_eq!(b.spent(), 0);
        let a1_again = factory
            .begin_turn(session_a, generation_a, None)
            .await
            .unwrap();
        assert!(a1_again.shares_account(&a1));
        assert!(factory.capture(session_a).unwrap().shares_account(&a2));
        assert!(enforcer
            .scoped_for_session(session_a)
            .workflow_output_scopes()
            .capture(session_b)
            .is_err());
    }

    #[tokio::test]
    async fn output_scope_conflict_and_overflow_freeze_without_partial_publication() {
        let enforcer = enforcer();
        let session = enforcer.session_id().await;
        let scope = enforcer
            .workflow_output_scopes()
            .begin_turn(session, MessageId::new(), None)
            .await
            .unwrap();
        let event = WorkflowOutputEventId::MainResponse(MessageId::new());
        scope.record_legacy(event.clone(), u64::MAX).unwrap();
        assert!(scope.record_legacy(event.clone(), 1).is_err());
        assert_eq!(scope.spent(), u64::MAX);
        let overflow = WorkflowOutputEventId::MainResponse(MessageId::new());
        assert!(scope.record_legacy(overflow.clone(), 1).is_err());
        assert_eq!(scope.spent(), u64::MAX);
        scope.record_legacy(overflow, 0).unwrap();
        scope.record_legacy(event, u64::MAX).unwrap();
        assert!(enforcer
            .cost_tracker
            .durability_gate()
            .frozen_reason()
            .is_some());
    }

    #[tokio::test]
    async fn output_scope_records_actual_over_target_and_does_not_include_money_holds() {
        let enforcer = enforcer();
        let scope = enforcer
            .workflow_output_scopes()
            .begin_turn(enforcer.session_id().await, MessageId::new(), Some(10))
            .await
            .unwrap();
        let hold = enforcer.reserve_nano_usd(100).await.unwrap();
        assert_eq!(scope.spent(), 0);
        scope
            .record_legacy(
                WorkflowOutputEventId::WorkflowAgent {
                    run_id: "workflow".into(),
                    call_index: 0,
                },
                12,
            )
            .unwrap();
        assert_eq!(scope.spent(), 12);
        enforcer.release_reservation(hold).await;
        assert_eq!(scope.spent(), 12);
    }

    #[tokio::test]
    async fn output_scope_generation_exhaustion_keeps_the_published_account() {
        let enforcer = enforcer();
        let session_id = enforcer.session_id().await;
        let factory = enforcer.workflow_output_scopes();
        let original = factory
            .begin_turn(session_id, MessageId::new(), None)
            .await
            .unwrap();
        let session = enforcer.session_state_for(session_id).await;
        session.reservations.lock().unwrap().next_output_generation = u64::MAX;
        assert!(factory
            .begin_turn(session_id, MessageId::new(), None)
            .await
            .is_err());
        assert!(factory
            .capture(session_id)
            .unwrap()
            .shares_account(&original));
        assert_eq!(session.reservations.lock().unwrap().output_scopes.len(), 1);
    }

    #[tokio::test]
    async fn output_scope_final_owner_releases_its_captured_cost_state() {
        let enforcer = enforcer();
        let weak_state = Arc::downgrade(&enforcer.cost_tracker.selected_state_cell().await);
        let factory = enforcer.workflow_output_scopes();
        let scope = factory
            .begin_turn(enforcer.session_id().await, MessageId::new(), None)
            .await
            .unwrap();
        drop(enforcer);
        drop(factory);
        assert!(
            weak_state.upgrade().is_some(),
            "live scope retains its origin"
        );
        drop(scope);
        assert!(
            weak_state.upgrade().is_none(),
            "book must not own a strong backedge to its scope"
        );
    }
}
