//! Cross-process FIFO queue that serializes NixOS rebuilds.
//!
//! Guarantees that **only one operation is rebuilt at a time**, in arrival
//! order (strict FIFO by monotonic ticket number). Designed to be called
//! synchronously: `wait_turn` blocks the caller until its ticket reaches the
//! head of the queue.
//!
//! # Mechanism
//! Everything lives under [`QUEUE_DIR`]:
//! * [`META_LOCK`] — short-lived lock held during ticket allocation and each
//!   scan, to serialize those critical sections across processes.
//! * [`SEQ_FILE`] — monotonic counter, read+incremented under the meta lock.
//! * `<N>` — one file per waiter, whose **exclusive flock is held for the whole
//!   lifetime of the [`Ticket`]**. That lock acts as a liveness proof: a ticket
//!   whose flock is free belongs to a dead process and can be cleaned up
//!   (crash recovery).

use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    thread,
    time::Duration,
};

use crate::error::io_error_at;
use crate::mx;

/// Root directory of the queue.
const QUEUE_DIR: &str = "/tmp/mx-build-queue";

/// Meta lock serializing ticket allocation and scans.
const META_LOCK: &str = "/tmp/mx-build-queue/.meta.lock";

/// Monotonic counter of ticket numbers.
const SEQ_FILE: &str = "/tmp/mx-build-queue/.seq";

/// Polling interval between two head-of-queue checks.
const POLL_INTERVAL: Duration = Duration::from_millis(150);

/// Entry point of the FIFO rebuild queue.
pub struct BuildQueue;

impl BuildQueue {
    /// Takes a FIFO ticket. Combine with [`Ticket::wait_turn`], then let `Drop`
    /// leave the queue.
    ///
    /// The whole section (counter increment + ticket file creation/locking) is
    /// protected by [`META_LOCK`], so no concurrent scan can observe an
    /// allocated number whose file does not exist yet.
    ///
    /// # Returns
    /// The caller's [`Ticket`], already locked; it does *not* mean the turn has
    /// come, only that the place in line is held.
    ///
    /// # Post-conditions
    /// [`QUEUE_DIR`] exists and holds the ticket file. The meta lock is
    /// released before returning, so other processes can enqueue in turn.
    ///
    /// # Errors
    /// [`mx::ErrorKind::IOError`] if the queue directory or the ticket file
    /// cannot be created, and [`mx::ErrorKind::FailToLock`] if the ticket's own
    /// lock cannot be taken.
    pub fn enqueue() -> mx::Result<Ticket> {
        fs::create_dir_all(QUEUE_DIR).map_err(|e| io_error_at(QUEUE_DIR, e))?;
        let _meta = lock_meta()?;

        let seq = next_seq()?;
        let path = Path::new(QUEUE_DIR).join(seq.to_string());
        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)
            .map_err(|e| io_error_at(&path.to_string_lossy(), e))?;
        file.lock().map_err(|_| mx::ErrorKind::FailToLock)?;

        Ok(Ticket { seq, file, path })
    }
}

/// A process's slot in the queue. While it lives, its ticket file stays locked;
/// its `Drop` leaves the queue (unlock + remove the file).
///
/// # Fields
/// * `seq` - the ticket number, which is the position in the FIFO order.
/// * `file` - the ticket file, whose held flock proves the owner is alive.
/// * `path` - that file's path, removed on drop.
pub struct Ticket {
    seq: u64,
    file: File,
    path: PathBuf,
}

impl Ticket {
    /// Blocks until this ticket is at the head of the queue, then returns.
    ///
    /// On each iteration, under [`META_LOCK`], it scans the lower-numbered
    /// tickets: a ticket still locked is a live waiter ahead of us; a ticket
    /// whose flock is free belongs to a dead process and is cleaned up. Once no
    /// live waiter remains ahead, we are at the head.
    ///
    /// # Post-conditions
    /// Returns only once the turn has come, so the wait is unbounded: it lasts
    /// as long as the rebuilds queued ahead. Blocks the calling thread, sleeping
    /// [`POLL_INTERVAL`] between two checks. The place in line is only released
    /// when the [`Ticket`] is dropped, not here.
    ///
    /// # Errors
    /// [`mx::ErrorKind::FailToLock`] or [`mx::ErrorKind::IOError`] if the meta
    /// lock or the queue directory becomes unusable.
    pub fn wait_turn(&self) -> mx::Result<()> {
        loop {
            {
                let _meta = lock_meta()?;
                if self.is_head()? {
                    return Ok(());
                }
            }
            thread::sleep(POLL_INTERVAL);
        }
    }

    /// Tells whether this ticket is at the head of the queue.
    ///
    /// # Pre-conditions
    /// Must be called while holding [`META_LOCK`], otherwise a concurrent
    /// allocation could be missed by the scan.
    ///
    /// # Returns
    /// `true` when no live waiter has a lower number.
    ///
    /// # Post-conditions
    /// Scans every entry of [`QUEUE_DIR`], skipping [`META_LOCK`], [`SEQ_FILE`]
    /// and any other non-numeric name, since only ticket files use a plain
    /// integer. Tickets whose owner died are deleted along the way (crash
    /// recovery), so the call has a cleanup side effect. A ticket file that
    /// vanishes mid-scan (removed concurrently by its own owner) is treated as
    /// nothing to wait for, not as an error.
    fn is_head(&self) -> mx::Result<bool> {
        for entry in fs::read_dir(QUEUE_DIR).map_err(mx::ErrorKind::IOError)? {
            let entry = entry.map_err(mx::ErrorKind::IOError)?;
            let name = entry.file_name();
            let Ok(n) = name.to_string_lossy().parse::<u64>() else {
                continue;
            };
            if n >= self.seq {
                continue;
            }

            let path = entry.path();
            match File::open(&path) {
                Ok(f) => match f.try_lock() {
                    Ok(()) => {
                        let _ = f.unlock();
                        drop(f);
                        let _ = fs::remove_file(&path);
                    }
                    Err(_) => return Ok(false),
                },
                Err(_) => {
                    let _ = fs::remove_file(&path);
                }
            }
        }
        Ok(true)
    }
}

impl Drop for Ticket {
    /// Leaves the queue: releases the ticket's lock and deletes its file.
    ///
    /// # Post-conditions
    /// The next waiter can become the head. Failures are ignored - a leftover
    /// file is picked up as a stale ticket by the next scan anyway.
    fn drop(&mut self) {
        let _ = self.file.unlock();
        let _ = fs::remove_file(&self.path);
    }
}

/// Opens and locks [`META_LOCK`].
///
/// # Returns
/// The locked file; the lock lasts exactly as long as the returned handle, so
/// the caller keeps it alive for its critical section.
///
/// # Post-conditions
/// Blocks until the lock is free, since another process may be allocating a
/// ticket or scanning the queue.
///
/// # Errors
/// [`mx::ErrorKind::IOError`] if the lock file cannot be opened, and
/// [`mx::ErrorKind::FailToLock`] if locking fails.
fn lock_meta() -> mx::Result<File> {
    let file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(META_LOCK)
        .map_err(|e| io_error_at(META_LOCK, e))?;
    file.lock().map_err(|_| mx::ErrorKind::FailToLock)?;
    Ok(file)
}

/// Allocates the next ticket number.
///
/// # Pre-conditions
/// Must be called while holding [`META_LOCK`], since the read-increment-write
/// is not atomic on its own.
///
/// # Returns
/// The new number, one above the stored one; 1 when [`SEQ_FILE`] is missing,
/// empty or unparseable - a corrupted counter restarts the numbering instead of
/// failing.
///
/// # Post-conditions
/// [`SEQ_FILE`] holds the number just returned.
///
/// # Errors
/// [`mx::ErrorKind::IOError`] if the counter cannot be opened, read or
/// rewritten.
fn next_seq() -> mx::Result<u64> {
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(SEQ_FILE)
        .map_err(|e| io_error_at(SEQ_FILE, e))?;

    let mut content = String::new();
    file.read_to_string(&mut content)
        .map_err(mx::ErrorKind::IOError)?;
    let next = content.trim().parse::<u64>().unwrap_or(0) + 1;

    file.seek(SeekFrom::Start(0))
        .map_err(mx::ErrorKind::IOError)?;
    file.set_len(0).map_err(mx::ErrorKind::IOError)?;
    file.write_all(next.to_string().as_bytes())
        .map_err(mx::ErrorKind::IOError)?;
    Ok(next)
}

#[cfg(test)]
#[path = "build_queue_tests.rs"]
mod tests;
