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
    pub fn enqueue() -> mx::Result<Ticket> {
        fs::create_dir_all(QUEUE_DIR).map_err(mx::ErrorKind::IOError)?;
        let _meta = lock_meta()?;

        let seq = next_seq()?;
        let path = Path::new(QUEUE_DIR).join(seq.to_string());
        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)
            .map_err(mx::ErrorKind::IOError)?;
        file.lock().map_err(|_| mx::ErrorKind::FailToLock)?;

        Ok(Ticket { seq, file, path })
        // `_meta` is dropped here → meta lock released.
    }
}

/// A process's slot in the queue. While it lives, its ticket file stays locked;
/// its `Drop` leaves the queue (unlock + remove the file).
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

    /// Returns `true` if no live waiter precedes this ticket.
    /// Must be called while holding [`META_LOCK`].
    fn is_head(&self) -> mx::Result<bool> {
        for entry in fs::read_dir(QUEUE_DIR).map_err(mx::ErrorKind::IOError)? {
            let entry = entry.map_err(mx::ErrorKind::IOError)?;
            let name = entry.file_name();
            // Ignore `.meta.lock`, `.seq` and any non-numeric name.
            let Ok(n) = name.to_string_lossy().parse::<u64>() else {
                continue;
            };
            if n >= self.seq {
                continue;
            }

            let path = entry.path();
            match File::open(&path) {
                Ok(f) => match f.try_lock() {
                    // Lockable → owner is dead → stale ticket, clean it up.
                    Ok(()) => {
                        let _ = f.unlock();
                        drop(f);
                        let _ = fs::remove_file(&path);
                    }
                    // Already locked → live waiter ahead of us.
                    Err(_) => return Ok(false),
                },
                // File vanished in the meantime → nothing to wait for.
                Err(_) => {
                    let _ = fs::remove_file(&path);
                }
            }
        }
        Ok(true)
    }
}

impl Drop for Ticket {
    fn drop(&mut self) {
        let _ = self.file.unlock();
        let _ = fs::remove_file(&self.path);
    }
}

/// Opens and locks [`META_LOCK`]. The lock is released when the `File` is dropped.
fn lock_meta() -> mx::Result<File> {
    let file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(META_LOCK)
        .map_err(mx::ErrorKind::IOError)?;
    file.lock().map_err(|_| mx::ErrorKind::FailToLock)?;
    Ok(file)
}

/// Reads, increments and rewrites the monotonic counter. Must be called while
/// holding [`META_LOCK`].
fn next_seq() -> mx::Result<u64> {
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(SEQ_FILE)
        .map_err(mx::ErrorKind::IOError)?;

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
