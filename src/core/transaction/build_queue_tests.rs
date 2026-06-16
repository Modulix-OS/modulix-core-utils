//! Tests for the FIFO build queue.
//!
//! These exercise the real `/tmp/mx-build-queue/` directory. A process-wide
//! mutex serializes them so concurrent test threads don't interleave on the
//! shared queue state.

use super::*;
use std::sync::{Mutex, OnceLock};

/// Serializes queue tests: they all share the single on-disk queue dir.
fn test_guard() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// Removes the whole queue dir so each test starts from a clean slate.
fn reset_queue() {
    let _ = fs::remove_dir_all(QUEUE_DIR);
}

#[test]
fn enqueue_assigns_monotonic_increasing_seq() {
    let _g = test_guard();
    reset_queue();

    let t1 = BuildQueue::enqueue().unwrap();
    let t2 = BuildQueue::enqueue().unwrap();
    let t3 = BuildQueue::enqueue().unwrap();

    assert!(t1.seq < t2.seq);
    assert!(t2.seq < t3.seq);
}

#[test]
fn head_ticket_can_proceed_immediately() {
    let _g = test_guard();
    reset_queue();

    let head = BuildQueue::enqueue().unwrap();
    // Nothing in front → wait_turn returns at once.
    head.wait_turn().unwrap();
}

#[test]
fn drop_dequeues_and_removes_file() {
    let _g = test_guard();
    reset_queue();

    let ticket = BuildQueue::enqueue().unwrap();
    let path = ticket.path.clone();
    assert!(path.exists());
    drop(ticket);
    assert!(!path.exists(), "Drop must remove the ticket file");
}

#[test]
fn waiter_blocks_until_head_drops() {
    let _g = test_guard();
    reset_queue();

    let head = BuildQueue::enqueue().unwrap();
    let waiter = BuildQueue::enqueue().unwrap();

    // While `head` is alive and locked, the waiter is not at the head.
    {
        let _meta = lock_meta().unwrap();
        assert!(
            !waiter.is_head().unwrap(),
            "waiter must wait while head is alive"
        );
    }

    // Releasing the head lets the waiter reach the head of the queue.
    drop(head);
    waiter.wait_turn().unwrap();
}

#[test]
fn stale_orphan_ticket_is_cleaned_up() {
    let _g = test_guard();
    reset_queue();
    fs::create_dir_all(QUEUE_DIR).unwrap();

    // Forge an orphan ticket file with a low number and NO held lock,
    // simulating a process that crashed while queued.
    let orphan = std::path::Path::new(QUEUE_DIR).join("1");
    fs::write(&orphan, b"").unwrap();

    // A higher-numbered live waiter must detect the orphan as dead, remove it,
    // and become the head.
    let waiter = Ticket {
        seq: 5,
        file: fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(std::path::Path::new(QUEUE_DIR).join("5"))
            .unwrap(),
        path: std::path::Path::new(QUEUE_DIR).join("5"),
    };
    waiter.file.lock().unwrap();

    waiter.wait_turn().unwrap();
    assert!(!orphan.exists(), "orphan ticket must be cleaned up");
}
