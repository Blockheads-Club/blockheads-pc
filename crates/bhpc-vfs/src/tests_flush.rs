//! Background fsync worker.

use crate::flush::{begin, done, queue, wait};
use std::fs::File;
use std::io::Write;
use std::time::{Duration, Instant};

#[test]
fn queued_syncs_run_on_the_worker_and_wait_returns_after_them() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let mut file = File::create(&path).unwrap();
    file.write_all(b"page").unwrap();
    for _ in 0..3 {
        queue(file.try_clone().unwrap());
    }
    wait();
    assert_eq!(std::fs::read(&path).unwrap(), b"page");
}

#[test]
fn wait_blocks_until_the_last_sync_is_done() {
    begin();
    let t0 = Instant::now();
    let finisher = std::thread::spawn(|| {
        std::thread::sleep(Duration::from_millis(100));
        done();
    });
    wait();
    assert!(t0.elapsed() >= Duration::from_millis(100));
    finisher.join().unwrap();
}
