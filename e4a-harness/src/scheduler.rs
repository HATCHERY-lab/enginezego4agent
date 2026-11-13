// Portions derived from pi-durable 1.5.2 (packages/durable/src/harness), MIT, © 2025 Mario Zechner / Earendil Works. See LICENSES/pi-MIT.txt.

//! In-memory ready queue of task ids, plus waiters keyed by the task they watch.
//!
//! No model call and no async. The queue does not run task code.
//!
//! Unreleased main after tag v1.5.2 is not ported: suspendable work (signals,
//! tools that park, gates, hook tasks) and the code-mode namespace break stay
//! out. The wait-promise failure is recorded here. [`Scheduler::wait`] stores
//! that string before it returns. [`Scheduler::take_wait_error`] polls one copy
//! off. Nothing else removes it. A caller that never polls leaves the string.
//! A waiter dropped before [`Waiter::observe`] still appends a copy to
//! [`Scheduler::dropped_errors`] and does not delete the polled one.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::rc::Rc;

use crate::Error;

/// Single-threaded ready queue. Clones share the same queue and waiter lists.
///
/// Not `Send`. Nothing here waits on a timer or a model.
#[derive(Clone, Debug)]
pub struct Scheduler {
    shared: Rc<Shared>,
}

#[derive(Debug)]
struct Shared {
    inner: RefCell<Inner>,
    /// Drops that ran while `inner` was borrowed. Flushed by the next method.
    late: RefCell<Vec<LateDrop>>,
}

#[derive(Debug, Default)]
struct Inner {
    ready: VecDeque<i64>,
    queued: HashSet<i64>,
    by_task: HashMap<i64, Vec<u64>>,
    records: HashMap<u64, WaiterRecord>,
    next_id: u64,
    dropped_errors: Vec<String>,
    /// Failure strings keyed by task or submission id, in record order.
    ///
    /// `wait` and `admit` push before they return. [`Scheduler::take_wait_error`]
    /// pops one. Drop, with or without [`Waiter::observe`], does not.
    wait_errors: HashMap<i64, VecDeque<String>>,
}

#[derive(Debug)]
struct WaiterRecord {
    task_id: i64,
    error: String,
}

#[derive(Debug)]
struct LateDrop {
    id: u64,
    error: String,
    store: bool,
}

/// One watcher of a task id.
///
/// Dropping the guard before [`Waiter::observe`] appends its error string to the
/// scheduler and leaves the copy [`Scheduler::take_wait_error`] reads.
/// `observe` consumes the wait and stores nothing on the drop list.
#[derive(Debug)]
pub struct Waiter {
    shared: Rc<Shared>,
    id: u64,
    task_id: i64,
    error: String,
    observed: bool,
}

impl Default for Scheduler {
    fn default() -> Self {
        Self::new()
    }
}

impl Scheduler {
    pub fn new() -> Self {
        Self {
            shared: Rc::new(Shared {
                inner: RefCell::new(Inner::default()),
                late: RefCell::new(Vec::new()),
            }),
        }
    }

    /// Enqueue `task_id`. An id already in the queue stays where it is.
    pub fn push(&self, task_id: i64) {
        flush(&self.shared);
        self.shared.inner.borrow_mut().enqueue(task_id);
    }

    /// Pop the oldest ready task id.
    pub fn pop_ready(&self) -> Option<i64> {
        flush(&self.shared);
        self.shared.inner.borrow_mut().pop_ready()
    }

    /// Waiter ids watching `task_id`, in registration order.
    pub fn waiters(&self, task_id: i64) -> Vec<u64> {
        flush(&self.shared);
        self.shared
            .inner
            .borrow()
            .by_task
            .get(&task_id)
            .cloned()
            .unwrap_or_default()
    }

    /// Error strings of waiters dropped before observe, in drop order.
    pub fn dropped_errors(&self) -> Vec<String> {
        flush(&self.shared);
        self.shared.inner.borrow().dropped_errors.clone()
    }

    /// Poll one failure string recorded for `id` before `wait` or `admit` returned.
    ///
    /// `None` when that id has nothing left. Not calling this leaves the string.
    pub fn take_wait_error(&self, id: i64) -> Option<String> {
        flush(&self.shared);
        self.shared.inner.borrow_mut().take_wait_error(id)
    }

    /// Store `error` for [`Scheduler::take_wait_error`] before the caller returns it.
    pub(crate) fn record_wait_failure(&self, id: i64, error: String) {
        flush(&self.shared);
        self.shared.inner.borrow_mut().push_wait_error(id, error);
    }

    /// Watch `task_id`.
    ///
    /// `error` is recorded before this returns. Dropping the waiter before
    /// [`Waiter::observe`] appends it to [`Scheduler::dropped_errors`] and does
    /// not remove the recorded copy. [`Scheduler::take_wait_error`] polls it.
    /// An empty `error` is rejected after that rejection text is recorded.
    pub fn wait(&self, task_id: i64, error: impl Into<String>) -> Result<Waiter, Error> {
        flush(&self.shared);
        let error = error.into();
        if error.is_empty() {
            let failure = "waiter error must not be empty".to_owned();
            self.shared
                .inner
                .borrow_mut()
                .push_wait_error(task_id, failure.clone());
            return Err(Error::message(failure));
        }
        let mut inner = self.shared.inner.borrow_mut();
        let id = match inner.alloc_id() {
            Ok(id) => id,
            Err(failure) => {
                let message = failure.to_string();
                inner.push_wait_error(task_id, message);
                return Err(failure);
            }
        };
        inner.push_wait_error(task_id, error.clone());
        inner.records.insert(
            id,
            WaiterRecord {
                task_id,
                error: error.clone(),
            },
        );
        inner.by_task.entry(task_id).or_default().push(id);
        drop(inner);
        Ok(Waiter {
            shared: Rc::clone(&self.shared),
            id,
            task_id,
            error,
            observed: false,
        })
    }
}

impl Waiter {
    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn task_id(&self) -> i64 {
        self.task_id
    }

    /// The wait was seen. Drop stores nothing.
    pub fn observe(mut self) {
        self.observed = true;
        if let Ok(mut inner) = self.shared.inner.try_borrow_mut() {
            inner.detach(self.id, false);
            return;
        }
        self.shared.late.borrow_mut().push(LateDrop {
            id: self.id,
            error: String::new(),
            store: false,
        });
    }
}

impl Drop for Waiter {
    fn drop(&mut self) {
        if self.observed {
            return;
        }
        self.observed = true;
        if let Ok(mut inner) = self.shared.inner.try_borrow_mut() {
            inner.detach(self.id, true);
            return;
        }
        // `inner` is borrowed on this thread. Keep the string for the next flush.
        self.shared.late.borrow_mut().push(LateDrop {
            id: self.id,
            error: std::mem::take(&mut self.error),
            store: true,
        });
    }
}

impl Inner {
    fn enqueue(&mut self, task_id: i64) {
        if self.queued.insert(task_id) {
            self.ready.push_back(task_id);
        }
    }

    fn pop_ready(&mut self) -> Option<i64> {
        let id = self.ready.pop_front()?;
        self.queued.remove(&id);
        Some(id)
    }

    fn alloc_id(&mut self) -> Result<u64, Error> {
        let id = self.next_id;
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or_else(|| Error::message("waiter ids exhausted"))?;
        Ok(id)
    }

    fn detach(&mut self, id: u64, store_error: bool) {
        let Some(record) = self.records.remove(&id) else {
            return;
        };
        let remove_key = if let Some(list) = self.by_task.get_mut(&record.task_id) {
            list.retain(|waiter| *waiter != id);
            list.is_empty()
        } else {
            false
        };
        if remove_key {
            self.by_task.remove(&record.task_id);
        }
        if store_error {
            self.dropped_errors.push(record.error);
        }
    }

    fn push_wait_error(&mut self, id: i64, error: String) {
        self.wait_errors.entry(id).or_default().push_back(error);
    }

    fn take_wait_error(&mut self, id: i64) -> Option<String> {
        let error = self.wait_errors.get_mut(&id)?.pop_front()?;
        if self
            .wait_errors
            .get(&id)
            .is_some_and(VecDeque::is_empty)
        {
            self.wait_errors.remove(&id);
        }
        Some(error)
    }
}

fn flush(shared: &Shared) {
    let pending = {
        let mut late = shared.late.borrow_mut();
        std::mem::take(&mut *late)
    };
    if pending.is_empty() {
        return;
    }
    let mut inner = shared.inner.borrow_mut();
    for pending_drop in pending {
        inner.detach(pending_drop.id, false);
        if pending_drop.store {
            inner.dropped_errors.push(pending_drop.error);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_is_fifo_and_ignores_a_duplicate() {
        let sched = Scheduler::new();
        sched.push(1);
        sched.push(1);
        sched.push(2);
        assert_eq!(sched.pop_ready(), Some(1));
        assert_eq!(sched.pop_ready(), Some(2));
        assert_eq!(sched.pop_ready(), None);
        sched.push(1);
        assert_eq!(sched.pop_ready(), Some(1));
    }

    #[test]
    fn clones_share_the_queue() {
        let sched = Scheduler::new();
        let other = sched.clone();
        sched.push(3);
        assert_eq!(other.pop_ready(), Some(3));
    }

    #[test]
    fn drop_before_observe_stores_the_error_string() {
        let sched = Scheduler::new();
        let first = sched.wait(5, "gone").unwrap();
        let second = sched.wait(5, "stay").unwrap();
        let first_id = first.id();
        let second_id = second.id();
        assert_eq!(first.task_id(), 5);
        assert_eq!(sched.waiters(5), vec![first_id, second_id]);
        drop(first);
        assert_eq!(sched.dropped_errors(), vec!["gone".to_string()]);
        assert_eq!(sched.waiters(5), vec![second_id]);
        second.observe();
        assert_eq!(sched.dropped_errors(), vec!["gone".to_string()]);
        assert!(sched.waiters(5).is_empty());
        assert_eq!(sched.take_wait_error(5).as_deref(), Some("gone"));
        assert_eq!(sched.take_wait_error(5).as_deref(), Some("stay"));
    }

    #[test]
    fn observe_discards_the_error_string() {
        let sched = Scheduler::new();
        let waiter = sched.wait(4, "seen").unwrap();
        waiter.observe();
        assert!(sched.dropped_errors().is_empty());
        assert!(sched.waiters(4).is_empty());
        assert_eq!(sched.take_wait_error(4).as_deref(), Some("seen"));
    }

    #[test]
    fn empty_waiter_error_is_rejected() {
        let sched = Scheduler::new();
        let error = sched.wait(1, "").unwrap_err();
        assert_eq!(error.to_string(), "waiter error must not be empty");
        assert!(sched.waiters(1).is_empty());
        assert_eq!(
            sched.take_wait_error(1).as_deref(),
            Some("waiter error must not be empty")
        );
    }

    #[test]
    fn drop_while_the_queue_is_borrowed_still_stores_the_error() {
        let sched = Scheduler::new();
        let waiter = sched.wait(1, "late").unwrap();
        {
            let _hold = sched.shared.inner.borrow();
            drop(waiter);
        }
        assert_eq!(sched.dropped_errors(), vec!["late".to_string()]);
        assert!(sched.waiters(1).is_empty());
        assert_eq!(sched.take_wait_error(1).as_deref(), Some("late"));
    }

    #[test]
    fn failed_wait_keeps_its_string() {
        let sched = Scheduler::new();
        drop(sched.wait(9, "").unwrap_err());
        let waiter = sched.wait(9, "session aborted").unwrap();
        drop(waiter);
        assert_eq!(sched.pop_ready(), None);
        assert_eq!(
            sched.take_wait_error(9).as_deref(),
            Some("waiter error must not be empty")
        );
        assert_eq!(
            sched.take_wait_error(9).as_deref(),
            Some("session aborted")
        );
        assert!(sched.take_wait_error(9).is_none());
    }
}
