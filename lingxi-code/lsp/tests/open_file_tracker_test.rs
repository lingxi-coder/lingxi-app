//! `OpenFileTracker` — dedup of `textDocument/didOpen` per (server, uri).

use lsp::OpenFileTracker;
use lsp_types::Url;

#[tokio::test]
async fn mark_then_check_returns_true() {
    let tracker = OpenFileTracker::new();
    let uri = Url::parse("file:///tmp/foo.rs").unwrap();

    assert!(!tracker.is_open("rust-analyzer", &uri).await);
    tracker.mark_open("rust-analyzer", uri.clone()).await;
    assert!(tracker.is_open("rust-analyzer", &uri).await);
}

#[tokio::test]
async fn different_servers_dedup_independently() {
    let tracker = OpenFileTracker::new();
    let uri = Url::parse("file:///tmp/foo.ts").unwrap();

    tracker.mark_open("typescript", uri.clone()).await;
    assert!(tracker.is_open("typescript", &uri).await);
    assert!(!tracker.is_open("eslint", &uri).await);

    tracker.mark_open("eslint", uri.clone()).await;
    assert!(tracker.is_open("eslint", &uri).await);
}

#[tokio::test]
async fn clear_server_resets_dedup_for_that_server_only() {
    let tracker = OpenFileTracker::new();
    let uri1 = Url::parse("file:///tmp/a.rs").unwrap();
    let uri2 = Url::parse("file:///tmp/b.rs").unwrap();
    tracker.mark_open("rust-analyzer", uri1.clone()).await;
    tracker.mark_open("rust-analyzer", uri2.clone()).await;
    tracker
        .mark_open("gopls", Url::parse("file:///tmp/x.go").unwrap())
        .await;

    let removed = tracker.clear_server("rust-analyzer").await;
    assert_eq!(removed, 2);
    assert!(!tracker.is_open("rust-analyzer", &uri1).await);
    assert!(!tracker.is_open("rust-analyzer", &uri2).await);
    assert!(
        tracker
            .is_open("gopls", &Url::parse("file:///tmp/x.go").unwrap())
            .await
    );
}

#[tokio::test]
async fn clear_single_pair_only() {
    let tracker = OpenFileTracker::new();
    let uri1 = Url::parse("file:///tmp/a.rs").unwrap();
    let uri2 = Url::parse("file:///tmp/b.rs").unwrap();
    tracker.mark_open("rust-analyzer", uri1.clone()).await;
    tracker.mark_open("rust-analyzer", uri2.clone()).await;

    tracker.clear("rust-analyzer", &uri1).await;
    assert!(!tracker.is_open("rust-analyzer", &uri1).await);
    assert!(tracker.is_open("rust-analyzer", &uri2).await);
    assert_eq!(tracker.len().await, 1);
}

#[tokio::test]
async fn len_and_is_empty_track_total() {
    let tracker = OpenFileTracker::new();
    assert!(tracker.is_empty().await);
    assert_eq!(tracker.len().await, 0);

    tracker
        .mark_open("rust-analyzer", Url::parse("file:///tmp/a.rs").unwrap())
        .await;
    tracker
        .mark_open("gopls", Url::parse("file:///tmp/x.go").unwrap())
        .await;
    assert_eq!(tracker.len().await, 2);
    assert!(!tracker.is_empty().await);
}
