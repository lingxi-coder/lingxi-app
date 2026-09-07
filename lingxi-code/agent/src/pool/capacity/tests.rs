use super::*;
use futures::poll;
use std::task::Poll;

#[tokio::test]
async fn zero_capacity_disables_allocations_without_constructor_failure() {
    let core = CapacityCore::new(0).unwrap();
    assert!(matches!(core.acquire_ordinary(), Err(AdmissionError::Full)));
    for count in [0, 1] {
        assert!(matches!(
            core.reserve_group(count, deadline(), CancellationToken::new())
                .await,
            Err(AdmissionError::InvalidCount)
        ));
    }
    if Semaphore::MAX_PERMITS > u32::MAX as usize {
        let count = (u32::MAX as usize).checked_add(1).unwrap();
        let large = CapacityCore::new(count).unwrap();
        assert!(large.acquire_ordinary().is_ok());
        assert!(matches!(
            large
                .reserve_group(count, deadline(), CancellationToken::new())
                .await,
            Err(AdmissionError::InvalidCount)
        ));
    }
}

#[tokio::test]
async fn waiter_id_overflow_and_poison_fail_closed() {
    let core = CapacityCore::new(1).unwrap();
    core.state().next_id = u64::MAX;
    assert!(matches!(
        core.reserve_group(1, deadline(), CancellationToken::new())
            .await,
        Err(AdmissionError::Closed)
    ));
    assert!(core.state().queue.is_empty());
    assert!(matches!(
        core.acquire_ordinary(),
        Err(AdmissionError::Closed)
    ));

    let core = CapacityCore::new(1).unwrap();
    let held = core.acquire_ordinary().unwrap();
    let mut waiting = Box::pin(core.reserve_group(1, deadline(), CancellationToken::new()));
    assert!(poll!(&mut waiting).is_pending());
    let poisoned = core.clone();
    assert!(
        std::thread::spawn(move || {
            let _guard = poisoned.state.lock().unwrap();
            panic!("poison capacity book");
        })
        .join()
        .is_err()
    );
    drop(held);
    assert!(matches!(waiting.await, Err(AdmissionError::Closed)));
    assert!(matches!(
        core.acquire_ordinary(),
        Err(AdmissionError::Closed)
    ));
    assert_eq!(core.available_permits(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ordinary_contention_cannot_take_queued_group_headroom() {
    let core = CapacityCore::new(4).unwrap();
    let held: Vec<_> = (0..4).map(|_| core.acquire_ordinary().unwrap()).collect();
    let mut group = Box::pin(core.reserve_group(3, deadline(), CancellationToken::new()));
    assert!(poll!(&mut group).is_pending());
    drop(held);
    let barrier = Arc::new(tokio::sync::Barrier::new(3));
    let mut contenders = Vec::new();
    for _ in 0..2 {
        let core = core.clone();
        let barrier = barrier.clone();
        contenders.push(tokio::spawn(async move {
            barrier.wait().await;
            core.acquire_ordinary()
        }));
    }
    barrier.wait().await;
    let group = group.await.unwrap();
    let first = contenders.remove(0).await.unwrap();
    let second = contenders.remove(0).await.unwrap();
    assert_eq!(usize::from(first.is_ok()) + usize::from(second.is_ok()), 1);
    assert_eq!(group.len(), 3);
    assert_eq!(core.available_permits(), 0);
    drop((first, second, group));
    assert_eq!(core.available_permits(), 4);
}

fn deadline() -> Instant {
    Instant::now() + Duration::from_secs(100)
}

#[tokio::test]
async fn producer_ticket_waits_for_every_physical_owner_without_holding_capacity() {
    use platform_api::panel_pool::PanelPoolDrain;
    let core = CapacityCore::new(2).unwrap();
    let mut permits = core.reserve_group(2, deadline(), CancellationToken::new()).await.unwrap();
    let ticket = PoolGroupDrain::new(2);
    for permit in &mut permits { permit.track_group(ticket.clone()); }
    let first = Arc::new(permits.pop().unwrap());
    let runner = first.clone();
    let second = permits.pop().unwrap();
    let mut waiting = Box::pin(ticket.wait());
    assert!(poll!(&mut waiting).is_pending());
    drop(first);
    drop(second);
    assert_eq!(core.available_permits(), 1);
    assert!(poll!(&mut waiting).is_pending());
    drop(runner);
    waiting.await;
    assert_eq!(core.available_permits(), 2);
    // Waiting repeatedly neither consumes nor owns physical capacity.
    ticket.wait().await;
    assert!(core.acquire_ordinary().is_ok());
}

#[tokio::test]
async fn fifo_whole_group_and_surplus_headroom() {
    let core = CapacityCore::new(4).unwrap();
    let held: Vec<_> = (0..4).map(|_| core.acquire_ordinary().unwrap()).collect();
    let mut head = Box::pin(core.reserve_group(3, deadline(), CancellationToken::new()));
    let mut tail = Box::pin(core.reserve_group(1, deadline(), CancellationToken::new()));
    assert!(poll!(&mut head).is_pending());
    assert!(poll!(&mut tail).is_pending());
    drop(held);
    let surplus = core.acquire_ordinary().unwrap();
    assert!(matches!(core.acquire_ordinary(), Err(AdmissionError::Full)));
    assert!(poll!(&mut tail).is_pending());
    let group = match poll!(&mut head) {
        Poll::Ready(Ok(group)) => group,
        _ => panic!("head ready"),
    };
    assert_eq!(group.len(), 3);
    assert_eq!(core.available_permits(), 0);
    assert!(poll!(&mut tail).is_pending());
    drop(surplus);
    let tail_group = tail.await.unwrap();
    assert_eq!(tail_group.len(), 1);
    drop(group);
    drop(tail_group);
    assert_eq!(core.available_permits(), 4);
}

#[tokio::test]
async fn failed_bulk_is_atomic_and_dropped_head_unblocks_tail() {
    let core = CapacityCore::new(3).unwrap();
    let held = core.acquire_ordinary().unwrap();
    let mut head = Box::pin(core.reserve_group(3, deadline(), CancellationToken::new()));
    let mut tail = Box::pin(core.reserve_group(1, deadline(), CancellationToken::new()));
    assert!(poll!(&mut head).is_pending());
    assert_eq!(core.available_permits(), 2);
    assert!(poll!(&mut tail).is_pending());
    drop(head);
    let group = tail.await.unwrap();
    assert_eq!(core.available_permits(), 1);
    drop(group);
    drop(held);
}

#[tokio::test]
async fn queue_sixteen_rejects_seventeenth_and_cancel_cleans_entry() {
    let core = CapacityCore::new(1).unwrap();
    let held = core.acquire_ordinary().unwrap();
    let cancel = CancellationToken::new();
    let mut waiters: Vec<_> = (0..16)
        .map(|_| Box::pin(core.reserve_group(1, deadline(), cancel.clone())))
        .collect();
    for waiter in &mut waiters {
        assert!(poll!(waiter).is_pending());
    }
    assert!(matches!(
        core.reserve_group(1, deadline(), cancel.clone()).await,
        Err(AdmissionError::QueueFull)
    ));
    cancel.cancel();
    for waiter in waiters {
        assert!(matches!(waiter.await, Err(AdmissionError::Cancelled)));
    }
    assert!(core.state().queue.is_empty());
    drop(held);
}

#[tokio::test(start_paused = true)]
async fn fixed_deadline_does_not_reset_on_release_notifications() {
    let core = CapacityCore::new(2).unwrap();
    let held = core.acquire_ordinary().unwrap();
    let mut group = Box::pin(core.reserve_group(2, deadline(), CancellationToken::new()));
    assert!(poll!(&mut group).is_pending());
    tokio::time::advance(Duration::from_secs(20)).await;
    core.notify(&mut core.state());
    assert!(poll!(&mut group).is_pending());
    tokio::time::advance(Duration::from_secs(10)).await;
    assert!(matches!(group.await, Err(AdmissionError::Deadline)));
    let mut short = Box::pin(core.reserve_group(
        2,
        Instant::now() + Duration::from_secs(2),
        CancellationToken::new(),
    ));
    assert!(poll!(&mut short).is_pending());
    tokio::time::advance(Duration::from_secs(2)).await;
    assert!(matches!(short.await, Err(AdmissionError::Deadline)));
    drop(held);
}

#[tokio::test]
async fn subscribed_generation_retains_release_before_wait_and_coalesces() {
    let core = CapacityCore::new(2).unwrap();
    let first = core.acquire_ordinary().unwrap();
    let second = core.acquire_ordinary().unwrap();
    let mut changed = core.changed.subscribe();
    assert!(core.semaphore.clone().try_acquire_many_owned(2).is_err());
    drop(first);
    drop(second);
    // Both notifications happened between failed try and the first changed poll.
    assert!(poll!(Box::pin(changed.changed())).is_ready());
    assert_eq!(*changed.borrow(), 2);
    let group = core
        .reserve_group(2, deadline(), CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(group.len(), 2);
}

#[tokio::test]
async fn generation_overflow_fails_closed_after_actual_release() {
    let core = CapacityCore::new(1).unwrap();
    let held = core.acquire_ordinary().unwrap();
    let mut group = Box::pin(core.reserve_group(1, deadline(), CancellationToken::new()));
    assert!(poll!(&mut group).is_pending());
    core.state().generation = u64::MAX;
    drop(held);
    assert_eq!(core.available_permits(), 1);
    assert!(matches!(group.await, Err(AdmissionError::Closed)));
    assert!(matches!(
        core.acquire_ordinary(),
        Err(AdmissionError::Closed)
    ));
}

#[tokio::test]
async fn invalid_count_identity_and_unpolled_drop() {
    let core = CapacityCore::new(2).unwrap();
    let other = CapacityCore::new(2).unwrap();
    assert_eq!(core.max(), 2);
    for count in [0, 3] {
        assert!(matches!(
            core.reserve_group(count, deadline(), CancellationToken::new())
                .await,
            Err(AdmissionError::InvalidCount)
        ));
    }
    let permit = core.acquire_ordinary().unwrap();
    assert!(core.owns(&permit));
    assert!(!other.owns(&permit));
    drop(core.reserve_group(2, deadline(), CancellationToken::new()));
    assert!(core.state().queue.is_empty());
    drop(permit);
    assert_eq!(core.available_permits(), 2);
    assert_eq!(other.available_permits(), 2);
}
