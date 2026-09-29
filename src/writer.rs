//! Writes incident-transaction links to SQLite on a dedicated thread, in
//! batches, so a failure storm on a busy program never blocks ingest on disk
//! I/O or JSON serialization while the engine lock is held.

use crate::model::TxSummary;
use crate::store::Store;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::Arc;
use std::time::{Duration, Instant};
use vortex::events::VortexTransaction;

const MAX_BATCH: usize = 500;
const MAX_WAIT: Duration = Duration::from_millis(100);

enum Job {
    Link(i64, TxSummary, Arc<VortexTransaction>),
    Flush(mpsc::Sender<()>),
}

pub struct Writer {
    tx: mpsc::Sender<Job>,
}

impl Writer {
    pub fn spawn(store: Arc<Store>) -> Self {
        let (tx, rx) = mpsc::channel::<Job>();
        std::thread::Builder::new()
            .name("sentinel-writer".into())
            .spawn(move || {
                let mut batch = Vec::with_capacity(MAX_BATCH);
                let mut waiters = Vec::new();
                while let Ok(first) = rx.recv() {
                    let deadline = Instant::now() + MAX_WAIT;
                    let mut next = Some(first);
                    // Collect until the batch is full, a flush is requested,
                    // or the wait window closes.
                    loop {
                        match next.take() {
                            Some(Job::Link(id, s, t)) => batch.push((id, s, t)),
                            Some(Job::Flush(done)) => {
                                waiters.push(done);
                                break;
                            }
                            None => {}
                        }
                        if batch.len() >= MAX_BATCH {
                            break;
                        }
                        let left = deadline.saturating_duration_since(Instant::now());
                        match rx.recv_timeout(left) {
                            Ok(job) => next = Some(job),
                            Err(RecvTimeoutError::Timeout) => break,
                            Err(RecvTimeoutError::Disconnected) => break,
                        }
                    }
                    if !batch.is_empty() {
                        if let Err(e) = store.link_transactions(&batch) {
                            tracing::error!(error = %e, n = batch.len(), "failed to store incident transactions");
                        }
                        batch.clear();
                    }
                    for w in waiters.drain(..) {
                        let _ = w.send(());
                    }
                }
            })
            .expect("spawn writer thread");
        Self { tx }
    }

    pub fn link(&self, incident_id: i64, summary: TxSummary, tx: Arc<VortexTransaction>) {
        let _ = self.tx.send(Job::Link(incident_id, summary, tx));
    }

    /// Blocks until everything queued so far is on disk.
    pub fn flush(&self) {
        let (done, wait) = mpsc::channel();
        if self.tx.send(Job::Flush(done)).is_ok() {
            let _ = wait.recv();
        }
    }
}
