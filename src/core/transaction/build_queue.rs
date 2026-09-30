//! Cross-process FIFO queue that serializes NixOS rebuilds.
//!
//! Guarantees that **only one operation is rebuilt at a time**, in arrival
//! order (strict FIFO by monotonic ticket number). Designed to be called
//! synchronously: `wait_turn` blocks the caller until its ticket reaches the
//! head of the queue.
//!
//! # Mechanism
//! Everything lives under [`queue_dir`]:
//! * [`meta_lock`] — short-lived lock held during ticket allocation and each
//!   scan, to serialize those critical sections across processes.
//! * [`seq_file`] — monotonic counter, read+incremented under the meta lock.
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
///
/// Fixed at `/tmp/mx-build-queue` in every build that matters: the queue is a
/// **cross-process** rendezvous between `mx-daemon`, the `mx` CLI and
/// `mx-init`, so a path that varied per process would let two rebuilds run at
/// once. The unit tests are the one exception — they run unprivileged while
/// the deployed directory is created `root:root 0755` by
/// `systemd.tmpfiles.rules` (the daemon runs with `PrivateTmp`, and this path
/// is bind-mounted back in), so they get their own per-process directory
/// instead of being unable to write at all.
#[cfg(not(test))]
fn queue_dir() -> &'static Path {
    Path::new("/tmp/mx-build-queue")
}

#[cfg(test)]
fn queue_dir() -> &'static Path {
    use std::sync::OnceLock;
    static DIR: OnceLock<std::path::PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        std::env::temp_dir().join(format!("mx-build-queue-test-{}", std::process::id()))
    })
}

/// Meta lock serializing ticket allocation and scans, inside [`queue_dir`].
fn meta_lock() -> std::path::PathBuf {
    queue_dir().join(".meta.lock")
}

/// Monotonic counter of ticket numbers, inside [`queue_dir`].
fn seq_file() -> std::path::PathBuf {
    queue_dir().join(".seq")
}

/// Polling interval between two head-of-queue checks.
const POLL_INTERVAL: Duration = Duration::from_millis(150);

/// Entry point of the FIFO rebuild queue.
pub struct BuildQueue;

impl BuildQueue {
    /// Takes a FIFO ticket. Combine with [`Ticket::wait_turn`], then let `Drop`
    /// leave the queue.
    ///
    /// The whole section (counter increment + ticket file creation/locking) is
    /// protected by [`meta_lock`], so no concurrent scan can observe an
    /// allocated number whose file does not exist yet.
    ///
    /// # Returns
    /// The caller's [`Ticket`], already locked; it does *not* mean the turn has
    /// come, only that the place in line is held.
    ///
    /// # Post-conditions
    /// [`queue_dir`] exists and holds the ticket file. The meta lock is
    /// released before returning, so other processes can enqueue in turn.
    ///
    /// # Errors
    /// [`mx::ErrorKind::IOError`] if the queue directory or the ticket file
    /// cannot be created, and [`mx::ErrorKind::FailToLock`] if the ticket's own
    /// lock cannot be taken.
    pub fn enqueue() -> mx::Result<Ticket> {
        let dir = queue_dir();
        fs::create_dir_all(dir).map_err(|e| io_error_at(&dir.to_string_lossy(), e))?;
        let _meta = lock_meta()?;

        let seq = next_seq()?;
        let path = dir.join(seq.to_string());
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
    /// On each iteration, under [`meta_lock`], it scans the lower-numbered
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
    /// Must be called while holding [`meta_lock`], otherwise a concurrent
    /// allocation could be missed by the scan.
    ///
    /// # Returns
    /// `true` when no live waiter has a lower number.
    ///
    /// # Post-conditions
    /// Scans every entry of [`queue_dir`], skipping [`meta_lock`], [`seq_file`]
    /// and any other non-numeric name, since only ticket files use a plain
    /// integer. Tickets whose owner died are deleted along the way (crash
    /// recovery), so the call has a cleanup side effect. A ticket file that
    /// vanishes mid-scan (removed concurrently by its own owner) is treated as
    /// nothing to wait for, not as an error.
    fn is_head(&self) -> mx::Result<bool> {
        for entry in fs::read_dir(queue_dir()).map_err(mx::ErrorKind::IOError)? {
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

/// Opens and locks [`meta_lock`].
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
    let path = meta_lock();
    let file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .map_err(|e| io_error_at(&path.to_string_lossy(), e))?;
    file.lock().map_err(|_| mx::ErrorKind::FailToLock)?;
    Ok(file)
}

/// Allocates the next ticket number.
///
/// # Pre-conditions
/// Must be called while holding [`meta_lock`], since the read-increment-write
/// is not atomic on its own.
///
/// # Returns
/// The new number, one above the stored one; 1 when [`seq_file`] is missing,
/// empty or unparseable - a corrupted counter restarts the numbering instead of
/// failing.
///
/// # Post-conditions
/// [`seq_file`] holds the number just returned.
///
/// # Errors
/// [`mx::ErrorKind::IOError`] if the counter cannot be opened, read or
/// rewritten.
fn next_seq() -> mx::Result<u64> {
    let path = seq_file();
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .map_err(|e| io_error_at(&path.to_string_lossy(), e))?;

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
