//! Background `fsync`: the guest's syncs of host files run on one worker thread instead of the caller's.
//!
//! The game's LMDB world database is opened without `MDB_NOSYNC`, so every commit syncs the file (data, then meta
//! page). The game commits from `-[World preUpdate:]`, which runs once per frame: a full save every 61 frames and a
//! light-block commit every 10 or 100. A real `fsync` on the game thread stalls the frame for as long as the host
//! file system takes (tens to hundreds of milliseconds on btrfs, `FlushFileBuffers` on Windows), which showed as a
//! stutter every second or two (blockheads-pc#1).
//!
//! [`queue`] hands a duplicate of the file handle to the worker and returns at once. The written data is already in
//! the host's page cache, so other readers (and the game itself) see it immediately, and an app crash loses nothing.
//! Only a power loss or OS crash in the short window before the worker syncs can lose the last writes, the same as
//! on a phone that has not flushed yet. [`wait`] blocks until every queued sync has run (clean shutdown, tests).

use std::fs::File;
use std::sync::mpsc::{channel, Sender};
use std::sync::{Condvar, Mutex, OnceLock, PoisonError};

struct Flusher {
    tx: Mutex<Sender<File>>,
    /// Syncs queued but not finished yet.
    pending: Mutex<usize>,
    idle: Condvar,
}

fn flusher() -> &'static Flusher {
    static F: OnceLock<Flusher> = OnceLock::new();
    F.get_or_init(|| {
        let (tx, rx) = channel::<File>();
        std::thread::Builder::new()
            .name("bhpc-fsync".into())
            .spawn(move || {
                for file in rx {
                    // Nothing is waiting for the result: a failed sync cannot be reported to the guest any more.
                    let _ = file.sync_data();
                    drop(file);
                    done();
                }
            })
            .expect("spawn the fsync thread");
        Flusher { tx: Mutex::new(tx), pending: Mutex::new(0), idle: Condvar::new() }
    })
}

/// One more sync is on its way.
pub(crate) fn begin() {
    *flusher().pending.lock().unwrap_or_else(PoisonError::into_inner) += 1;
}

/// One sync has finished: wake [`wait`] when it was the last.
pub(crate) fn done() {
    let f = flusher();
    let mut n = f.pending.lock().unwrap_or_else(PoisonError::into_inner);
    *n -= 1;
    if *n == 0 {
        f.idle.notify_all();
    }
}

/// Sync `file` (a duplicate of the guest's handle) on the worker thread.
pub fn queue(file: File) {
    let f = flusher();
    begin();
    // The receiver lives as long as the process (the worker never exits), so the send cannot fail.
    let _ = f.tx.lock().unwrap_or_else(PoisonError::into_inner).send(file);
}

/// Block until every sync queued so far has run.
pub fn wait() {
    let f = flusher();
    let mut n = f.pending.lock().unwrap_or_else(PoisonError::into_inner);
    while *n > 0 {
        n = f.idle.wait(n).unwrap_or_else(PoisonError::into_inner);
    }
}
