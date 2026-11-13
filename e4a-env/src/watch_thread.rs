// engine4agent watch hub.

//! Path-event channel. Nothing here watches the filesystem or starts a thread.

use std::sync::mpsc::{self, Receiver, Sender};

use crate::watch::{coalesce, WatchEvent};

/// Sender for [`WatchEvent`] values.
///
/// The caller keeps the [`Receiver`] and passes it to [`WatchHub::drain`].
/// This type does not watch the filesystem and does not start a thread.
#[derive(Clone, Debug)]
pub struct WatchHub {
    tx: Sender<WatchEvent>,
}

impl WatchHub {
    /// Open a hub and the receiver that observes it.
    pub fn new() -> (Self, Receiver<WatchEvent>) {
        let (tx, rx) = mpsc::channel();
        (Self { tx }, rx)
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
