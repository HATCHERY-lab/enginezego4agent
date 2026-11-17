// Portions derived from pi-durable 1.5.2 (packages/durable/src/session/writer.ts, packages/durable/src/session/keys.ts), MIT, © 2025 Mario Zechner / Earendil Works. See LICENSES/pi-MIT.txt.

//! Dedicated OS thread that owns `Storage`. Commits are grouped while no call is in flight.

use std::collections::HashMap;
use std::sync::mpsc::{self, RecvError, TryRecvError};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;

use ez4a_store::Storage;
use ez4a_types::{CommitResult, StorageWrite};
use tokio::sync::oneshot;

use crate::keys::ALL_KEYS;
use crate::Error;

type Job = Box<dyn FnOnce(&dyn Storage) + Send>;

struct Gate {
    done: Mutex<bool>,
    cv: Condvar,
}

impl Gate {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            done: Mutex::new(false),
            cv: Condvar::new(),
        })
    }

    fn wait(&self) -> Result<(), Error> {
        let mut done = self
            .done
            .lock()
            .map_err(|_| Error::message("publication gate poisoned"))?;
        while !*done {
            done = self
                .cv
                .wait(done)
                .map_err(|_| Error::message("publication gate poisoned"))?;
        }
        Ok(())
    }

    fn signal(&self) {
        if let Ok(mut done) = self.done.lock() {
            *done = true;
            self.cv.notify_all();
        }
    }
}

struct Part {
    id: u64,
    writes: Vec<StorageWrite>,
    keys: Vec<String>,
    done: Arc<Gate>,
    reply: oneshot::Sender<Result<i64, Error>>,
    publish: Box<dyn FnOnce(i64) -> Result<(), Error> + Send>,
}

enum StoreOp {
    Job(Job),
    Admit(Part),
    Close(oneshot::Sender<Result<(), Error>>),
}

struct Gates {
    next_part: u64,
    keys: HashMap<String, u64>,
    parts: HashMap<u64, Arc<Gate>>,
    newest: Option<u64>,
}

impl Gates {
    fn new() -> Self {
        Self {
            next_part: 1,
            keys: HashMap::new(),
            parts: HashMap::new(),
            newest: None,
        }
    }

    fn admit(&mut self, keys: &[String]) -> (u64, Arc<Gate>) {
        let id = self.next_part;
        self.next_part += 1;
        let gate = Gate::new();
        for key in keys {
            self.keys.insert(key.clone(), id);
        }
        self.parts.insert(id, Arc::clone(&gate));
        self.newest = Some(id);
        (id, gate)
    }

    fn gate(&self, key: &str) -> Option<Arc<Gate>> {
        let id = *self.keys.get(key)?;
        self.parts.get(&id).cloned()
    }

    fn newest(&self) -> Option<Arc<Gate>> {
        let id = self.newest?;
        self.parts.get(&id).cloned()
    }

    fn published(&mut self, id: u64, keys: &[String]) {
        for key in keys {
            if self.keys.get(key) == Some(&id) {
                self.keys.remove(key);
            }
        }
        if self.newest == Some(id) {
            self.newest = None;
        }
        self.parts.remove(&id);
    }
}

struct Inner {
    tx: mpsc::Sender<StoreOp>,
    gates: Arc<Mutex<Gates>>,
}

pub(crate) struct StoreHandle {
    inner: Arc<Inner>,
}

impl Clone for StoreHandle {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl StoreHandle {
    pub(crate) fn spawn<S, F>(factory: F) -> Result<Self, Error>
    where
        S: Storage + Send + 'static,
        F: FnOnce() -> Result<S, ez4a_store::Error> + Send + 'static,
    {
        let (tx, rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let gates = Arc::new(Mutex::new(Gates::new()));
        let thread_gates = Arc::clone(&gates);
        thread::Builder::new()
            .name("ez4a-session-store".into())
            .spawn(move || match factory() {
                Ok(storage) => {
                    let _ = ready_tx.send(Ok(()));
                    run_loop(storage, rx, thread_gates);
                }
                Err(error) => {
                    let _ = ready_tx.send(Err(Error::from(error)));
                }
            })
            .map_err(|error| Error::message(format!("store thread: {error}")))?;
        ready_rx
            .recv()
            .map_err(|_| Error::message("store thread stopped before ready"))??;
        Ok(Self {
            inner: Arc::new(Inner { tx, gates }),
        })
    }

    pub(crate) fn call<T, F>(&self, job: F) -> Result<T, Error>
    where
        T: Send + 'static,
        F: FnOnce(&dyn Storage) -> Result<T, Error> + Send + 'static,
    {
        let (tx, rx) = mpsc::sync_channel(1);
        self.send(StoreOp::Job(Box::new(move |storage| {
            let _ = tx.send(job(storage));
        })))?;
        rx.recv()
            .map_err(|_| Error::message("store thread dropped reply"))?
    }

    pub(crate) fn admit(
        &self,
        writes: Vec<StorageWrite>,
        keys: Vec<String>,
        publish: Box<dyn FnOnce(i64) -> Result<(), Error> + Send>,
    ) -> Result<(u64, oneshot::Receiver<Result<i64, Error>>), Error> {
        let (id, done) = {
            let mut gates = lock_gates(self.inner.gates.as_ref())?;
            gates.admit(&keys)
        };
        let (reply, rx) = oneshot::channel();
        self.send(StoreOp::Admit(Part {
            id,
            writes,
            keys,
            done,
            reply,
            publish,
        }))?;
        Ok((id, rx))
    }

    pub(crate) fn touch(&self, key: &str) -> Result<(), Error> {
        let gate = {
            let gates = lock_gates(self.inner.gates.as_ref())?;
            if key == ALL_KEYS {
                gates.newest()
            } else {
                gates.gate(key)
            }
        };
        match gate {
            Some(gate) => gate.wait(),
            None => Ok(()),
        }
    }

    pub(crate) fn drained(&self) -> Result<(), Error> {
        let gate = {
            let gates = lock_gates(self.inner.gates.as_ref())?;
            gates.newest()
        };
        match gate {
            Some(gate) => gate.wait(),
            None => Ok(()),
        }
    }

    pub(crate) async fn close(&self) -> Result<(), Error> {
        let (tx, rx) = oneshot::channel();
        self.send(StoreOp::Close(tx))?;
        rx.await
            .map_err(|_| Error::message("store thread dropped close"))?
    }

    fn send(&self, op: StoreOp) -> Result<(), Error> {
        self.inner
            .tx
            .send(op)
            .map_err(|_| Error::message("store thread stopped"))
    }
}

fn lock_gates(gates: &Mutex<Gates>) -> Result<std::sync::MutexGuard<'_, Gates>, Error> {
    gates
        .lock()
        .map_err(|_| Error::message("publication gates poisoned"))
}

fn run_loop<S: Storage>(storage: S, rx: mpsc::Receiver<StoreOp>, gates: Arc<Mutex<Gates>>) {
    loop {
        match rx.recv() {
            Ok(StoreOp::Job(job)) => job(&storage),
            Ok(StoreOp::Admit(part)) => {
                let mut parts = vec![part];
                if collect_group(&rx, &storage, &mut parts, &gates) {
                    return;
                }
                flush(&storage, parts, &gates);
            }
            Ok(StoreOp::Close(reply)) => {
                let result = storage.close().map_err(Error::from);
                let _ = reply.send(result);
                return;
            }
            Err(RecvError) => {
                let _ = storage.close();
                return;
            }
        }
    }
}

/// Returns true when the store has closed.
fn collect_group<S: Storage>(
    rx: &mpsc::Receiver<StoreOp>,
    storage: &S,
    parts: &mut Vec<Part>,
    gates: &Arc<Mutex<Gates>>,
) -> bool {
    loop {
        match rx.try_recv() {
            Ok(StoreOp::Admit(part)) => parts.push(part),
            Ok(StoreOp::Job(job)) => job(storage),
            Ok(StoreOp::Close(reply)) => {
                flush(storage, std::mem::take(parts), gates);
                let result = storage.close().map_err(Error::from);
                let _ = reply.send(result);
                return true;
            }
            Err(TryRecvError::Empty) => return false,
            Err(TryRecvError::Disconnected) => return false,
        }
    }
}

fn flush<S: Storage>(storage: &S, parts: Vec<Part>, gates: &Arc<Mutex<Gates>>) {
    if parts.is_empty() {
        return;
    }
    let batches: Vec<Vec<StorageWrite>> = parts.iter().map(|part| part.writes.clone()).collect();
    let mut seqs = vec![0; parts.len()];
    let result = storage.commit(&batches, &mut |index, seq| {
        if let Some(slot) = seqs.get_mut(index) {
            *slot = seq;
        }
    });
    match result {
        Ok(CommitResult { stored, refused }) => {
            for (index, part) in parts.into_iter().enumerate() {
                if index < stored {
                    settle_part(part, Ok(seqs[index]), gates);
                } else {
                    let message = refused
                        .clone()
                        .unwrap_or_else(|| "Storage refused the batch".into());
                    settle_part(part, Err(Error::message(message)), gates);
                }
            }
        }
        Err(error) => {
            let error = Error::from(error);
            for part in parts {
                settle_part(part, Err(error.clone()), gates);
            }
        }
    }
}

fn settle_part(part: Part, outcome: Result<i64, Error>, gates: &Arc<Mutex<Gates>>) {
    match outcome {
        Ok(seq) => {
            let published = (part.publish)(seq);
            unregister(gates, part.id, &part.keys);
            part.done.signal();
            match published {
                Ok(()) => {
                    let _ = part.reply.send(Ok(seq));
                }
                Err(error) => {
                    let _ = part.reply.send(Err(error));
                }
            }
        }
        Err(error) => {
            unregister(gates, part.id, &part.keys);
            part.done.signal();
            let _ = part.reply.send(Err(error));
        }
    }
}

fn unregister(gates: &Arc<Mutex<Gates>>, id: u64, keys: &[String]) {
    if let Ok(mut locked) = lock_gates(gates.as_ref()) {
        locked.published(id, keys);
    }
}
