// engine4agent notify watch.

//! Path-event channel. [`WatchHub::spawn_watch`] watches one existing path.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};

use notify::{EventKind, RecursiveMode, Watcher};

use crate::watch::{coalesce, WatchEvent, WatchKind};
use crate::Error;

/// Sender for [`WatchEvent`] values.
///
/// [`WatchHub::new`] does not touch the filesystem. The caller keeps that
/// [`Receiver`] and passes it to [`WatchHub::drain`]. [`WatchHub::spawn_watch`]
/// stores its receiver in the hub; take it with [`WatchHub::receiver`].
///
/// [`Clone`] copies the sender only. Dropping a clone does not stop a watch
/// owned by the hub it was cloned from.
#[derive(Debug)]
pub struct WatchHub {
    tx: Sender<WatchEvent>,
    rx: Option<Receiver<WatchEvent>>,
    shutdown: Option<WatchStop>,
}

struct WatchStop {
    stop: Sender<WatchMsg>,
    handle: JoinHandle<()>,
}

impl fmt::Debug for WatchStop {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WatchStop").finish_non_exhaustive()
    }
}

enum WatchMsg {
    Event(Result<notify::Event, notify::Error>),
    Stop,
}

impl WatchHub {
    /// Open a hub and the receiver that observes it.
    pub fn new() -> (Self, Receiver<WatchEvent>) {
        let (tx, rx) = mpsc::channel();
        (
            Self {
                tx,
                rx: None,
                shutdown: None,
            },
            rx,
        )
    }

    /// Watch `root` until this hub is dropped.
    ///
    /// `root` must already exist. This does not create it. The watch is not
    /// recursive. Create, modify, and remove are pushed as [`WatchEvent`].
    /// Other notifications are ignored.
    ///
    /// The watcher is created on the watch thread. Drop sends shutdown and
    /// joins that thread so the watcher is dropped there.
    pub fn spawn_watch(root: &Path) -> Result<Self, Error> {
        match root.try_exists() {
            Ok(true) => {}
            Ok(false) => {
                return Err(Error::message(format!(
                    "watch root {} does not exist",
                    root.display()
                )));
            }
            Err(err) => {
                return Err(Error::message(format!(
                    "watch root {}: {err}",
                    root.display()
                )));
            }
        }
        let root = root.to_path_buf();
        let (tx, rx) = mpsc::channel();
        let thread_tx = tx.clone();
        let (msg_tx, msg_rx) = mpsc::channel();
        let stop_tx = msg_tx.clone();
        let (ready_tx, ready_rx) = mpsc::channel();
        let handle = thread::Builder::new()
            .name("e4a-env-watch".into())
            .spawn(move || serve(root, thread_tx, msg_tx, msg_rx, ready_tx))
            .map_err(|err| Error::message(format!("cannot start watch: {err}")))?;
        let started = match ready_rx.recv() {
            Ok(result) => result,
            Err(_) => Err(Error::message(
                "watch thread stopped before it started",
            )),
        };
        if let Err(err) = started {
            let _ = stop_tx.send(WatchMsg::Stop);
            let _ = handle.join();
            return Err(err);
        }
        Ok(Self {
            tx,
            rx: Some(rx),
            shutdown: Some(WatchStop {
                stop: stop_tx,
                handle,
            }),
        })
    }

    /// Receiver owned by [`Self::spawn_watch`].
    ///
    /// [`Self::new`] returns its receiver to the caller, so this is [`None`].
    /// A clone does not own the receiver either.
    pub fn receiver(&self) -> Option<&Receiver<WatchEvent>> {
        self.rx.as_ref()
    }

    /// Send `event`. A disconnected receiver drops it.
    pub fn push(&self, event: WatchEvent) {
        let _ = self.tx.send(event);
    }

    /// Append every queued event to `pending` and return [`coalesce`] of it.
    ///
    /// Uses [`Receiver::try_recv`] only. An empty channel and a disconnected
    /// channel both return without blocking. `pending` is not cleared, so
    /// events already in it stay part of the coalesced result.
    pub fn drain(rx: &Receiver<WatchEvent>, pending: &mut Vec<WatchEvent>) -> Vec<WatchEvent> {
        while let Ok(event) = rx.try_recv() {
            pending.push(event);
        }
        coalesce(pending)
    }
}

impl Clone for WatchHub {
    fn clone(&self) -> Self {
        Self {
            tx: self.tx.clone(),
            rx: None,
            shutdown: None,
        }
    }
}

impl Drop for WatchHub {
    fn drop(&mut self) {
        let Some(shutdown) = self.shutdown.take() else {
            return;
        };
        let _ = shutdown.stop.send(WatchMsg::Stop);
        let _ = shutdown.handle.join();
    }
}

fn serve(
    root: PathBuf,
    tx: Sender<WatchEvent>,
    msg_tx: Sender<WatchMsg>,
    msg_rx: Receiver<WatchMsg>,
    ready_tx: Sender<Result<(), Error>>,
) {
    // `RecommendedWatcher` is not `Send` on every platform, so it stays here.
    let mut watcher = match notify::recommended_watcher(move |result| {
        let _ = msg_tx.send(WatchMsg::Event(result));
    }) {
        Ok(watcher) => watcher,
        Err(err) => {
            let _ = ready_tx.send(Err(cannot_watch(&root, err)));
            return;
        }
    };
    if let Err(err) = watcher.watch(&root, RecursiveMode::NonRecursive) {
        let _ = ready_tx.send(Err(cannot_watch(&root, err)));
        return;
    }
    if ready_tx.send(Ok(())).is_err() {
        return;
    }
    let hub = WatchHub {
        tx,
        rx: None,
        shutdown: None,
    };
    while let Ok(message) = msg_rx.recv() {
        match message {
            WatchMsg::Stop => break,
            WatchMsg::Event(Ok(event)) => push_event(&hub, event),
            WatchMsg::Event(Err(_)) => {}
        }
    }
    drop(watcher);
}

fn push_event(hub: &WatchHub, event: notify::Event) {
    let Some(kind) = kind_of(event.kind) else {
        return;
    };
    for path in event.paths {
        hub.push(WatchEvent {
            path: path.to_string_lossy().into_owned(),
            kind,
        });
    }
}

fn kind_of(kind: EventKind) -> Option<WatchKind> {
    match kind {
        EventKind::Create(_) => Some(WatchKind::Create),
        EventKind::Modify(_) => Some(WatchKind::Change),
        EventKind::Remove(_) => Some(WatchKind::Remove),
        _ => None,
    }
}

fn cannot_watch(root: &Path, err: impl fmt::Display) -> Error {
    Error::message(format!("cannot watch {}: {err}", root.display()))
}

#[cfg(test)]
mod tests {
    use super::WatchHub;
    use crate::watch::{WatchEvent, WatchKind};

    fn event(path: &str, kind: WatchKind) -> WatchEvent {
        WatchEvent {
            path: path.to_owned(),
            kind,
        }
    }

    #[test]
    fn drain_appends_then_coalesces() {
        let (hub, rx) = WatchHub::new();
        hub.push(event("a", WatchKind::Create));
        hub.push(event("b", WatchKind::Change));
        hub.push(event("a", WatchKind::Change));
        let mut pending = vec![event("c", WatchKind::Create)];
        let kept = WatchHub::drain(&rx, &mut pending);
        assert_eq!(
            pending,
            vec![
                event("c", WatchKind::Create),
                event("a", WatchKind::Create),
                event("b", WatchKind::Change),
                event("a", WatchKind::Change),
            ]
        );
        assert_eq!(
            kept,
            vec![
                event("c", WatchKind::Create),
                event("a", WatchKind::Change),
                event("b", WatchKind::Change),
            ]
        );
        assert!(WatchHub::drain(&rx, &mut Vec::new()).is_empty());
    }

    #[test]
    fn push_after_receiver_drop_does_not_panic() {
        let (hub, rx) = WatchHub::new();
        drop(rx);
        hub.push(event("a", WatchKind::Remove));
    }

    #[test]
    fn drain_reads_events_queued_before_sender_drop() {
        let (hub, rx) = WatchHub::new();
        hub.push(event("a", WatchKind::Create));
        drop(hub);
        let mut pending = Vec::new();
        assert_eq!(
            WatchHub::drain(&rx, &mut pending),
            vec![event("a", WatchKind::Create)]
        );
    }
}
