//! A fixed-size thread pool, the design from the Rust book's final project
//! ("Building a Multithreaded Web Server"):
//!
//! ```text
//! execute(job) ──► Sender<Job> ══ channel ══ Arc<Mutex<Receiver<Job>>> ──► worker 0..N
//! ```
//!
//! An `mpsc` channel has exactly one receiver, so the workers share it
//! behind a `Mutex`: whoever holds the lock takes the next job. Shutdown
//! needs no extra message. Dropping the `Sender` closes the channel, every
//! `recv()` then fails, the worker loops end, and `Drop` joins the threads.

use std::panic::{self, AssertUnwindSafe};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;

/// A closure that runs once (`FnOnce`), may move to another thread
/// (`Send`), and borrows nothing from the caller's stack (`'static`).
type Job = Box<dyn FnOnce() + Send + 'static>;

pub struct ThreadPool {
    workers: Vec<Worker>,
    // An Option only so that `drop` can `take()` it and drop it first.
    sender: Option<mpsc::Sender<Job>>,
}

impl ThreadPool {
    /// # Panics
    /// If `size` is zero: a pool with no threads would never run anything.
    pub fn new(size: usize) -> ThreadPool {
        assert!(size > 0, "a thread pool needs at least one thread");
        let (sender, receiver) = mpsc::channel();
        let receiver = Arc::new(Mutex::new(receiver));
        let workers = (0..size)
            .map(|id| Worker::new(id, Arc::clone(&receiver)))
            .collect();
        ThreadPool {
            workers,
            sender: Some(sender),
        }
    }

    /// Queues `f` to run on the next free worker.
    pub fn execute<F>(&self, f: F)
    where
        F: FnOnce() + Send + 'static,
    {
        let sender = self.sender.as_ref().expect("sender is only taken in drop");
        sender
            .send(Box::new(f))
            .expect("workers live as long as the pool");
    }
}

impl Drop for ThreadPool {
    fn drop(&mut self) {
        // Close the channel. Workers finish the jobs already queued, then
        // their `recv()` returns Err and they exit.
        drop(self.sender.take());
        for worker in self.workers.drain(..) {
            if worker.thread.join().is_err() {
                eprintln!("worker {} panicked", worker.id);
            }
        }
    }
}

struct Worker {
    id: usize,
    thread: thread::JoinHandle<()>,
}

impl Worker {
    fn new(id: usize, receiver: Arc<Mutex<mpsc::Receiver<Job>>>) -> Worker {
        let thread = thread::spawn(move || {
            loop {
                // The MutexGuard is a temporary, dropped at the end of this
                // `let` statement, so the lock is released *before* the job
                // runs. `while let Ok(job) = receiver.lock()….recv()` would
                // hold it for the whole job and serialise the pool.
                let message = receiver.lock().expect("pool lock poisoned").recv();
                match message {
                    // If a job panics, catch it here so this worker survives
                    // and the pool doesn't shrink. The panic is still printed.
                    Ok(job) => {
                        let _ = panic::catch_unwind(AssertUnwindSafe(job));
                    }
                    Err(_) => break, // channel closed: shutting down
                }
            }
        });
        Worker { id, thread }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    #[test]
    fn runs_every_queued_job_before_drop_returns() {
        let done = Arc::new(AtomicUsize::new(0));
        let pool = ThreadPool::new(3);
        for _ in 0..20 {
            let done = Arc::clone(&done);
            pool.execute(move || {
                done.fetch_add(1, Ordering::SeqCst);
            });
        }
        drop(pool); // joins the workers
        assert_eq!(done.load(Ordering::SeqCst), 20);
    }

    #[test]
    fn a_panicking_job_does_not_kill_its_worker() {
        let pool = ThreadPool::new(1);
        pool.execute(|| panic!("job panicked on purpose"));
        let (tx, rx) = mpsc::channel();
        pool.execute(move || tx.send(42).unwrap());
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)), Ok(42));
    }
}
