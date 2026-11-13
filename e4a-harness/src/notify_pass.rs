// Portions derived from pi-durable unreleased main after v1.5.2, issue #11892, MIT, © 2025 Mario Zechner / Earendil Works. See LICENSES/pi-MIT.txt.

//! Notify sets for one scheduler pass, issue #11892.
//!
//! The pass takes its ids out of the shared set before planning. A notify
//! during the pass lands in the fresh set. The stale drop list from the
//! snapshot does not erase that id. No async and no store.

use std::collections::HashSet;

/// Shared runnable and finalize-check ids.
///
/// `runnable` is the waiting set. `finalize` is the completing set.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PendingPass {
    runnable: HashSet<i64>,
    finalize: HashSet<i64>,
}

/// Runnable ids taken out of [`PendingPass`] for one planning pass.
#[derive(Debug)]
pub struct RunnablePass {
    /// Ids that were pending when the pass began.
    pub ids: HashSet<i64>,
}

/// Finalize-check ids taken out of [`PendingPass`] for one planning pass.
#[derive(Debug)]
pub struct FinalizePass {
    /// Ids that were pending when the pass began.
    pub ids: HashSet<i64>,
}

impl PendingPass {
    /// Swap `runnable` for an empty set and return the taken ids.
    pub fn begin_runnable(&mut self) -> RunnablePass {
        RunnablePass {
            ids: std::mem::take(&mut self.runnable),
        }
    }

    /// Record a runnable task. During an open pass this hits the fresh set.
    pub fn notify_runnable(&mut self, id: i64) {
        self.runnable.insert(id);
    }

    /// Merge taken ids that are not in `dropped`.
    ///
    /// An id notified after [`Self::begin_runnable`] stays even when `dropped`
    /// names it. `dropped` never deletes from the live set.
    pub fn finish_runnable(&mut self, pass: RunnablePass, dropped: &[i64]) {
        merge_kept(&mut self.runnable, pass.ids, dropped);
    }

    /// Swap `finalize` for an empty set and return the taken ids.
    pub fn begin_finalize(&mut self) -> FinalizePass {
        FinalizePass {
            ids: std::mem::take(&mut self.finalize),
        }
    }

    /// Record a finalize check. During an open pass this hits the fresh set.
    pub fn notify_finalize(&mut self, id: i64) {
        self.finalize.insert(id);
    }

    /// Merge taken ids that are not in `dropped`.
    ///
    /// An id notified after [`Self::begin_finalize`] stays even when `dropped`
    /// names it. `dropped` never deletes from the live set.
    pub fn finish_finalize(&mut self, pass: FinalizePass, dropped: &[i64]) {
        merge_kept(&mut self.finalize, pass.ids, dropped);
    }
}

/// Put snapshot ids back unless this pass dropped them.
///
/// Ids already in `live` were notified after the snapshot and stay.
fn merge_kept(live: &mut HashSet<i64>, taken: HashSet<i64>, dropped: &[i64]) {
    let dropped: HashSet<i64> = dropped.iter().copied().collect();
    for id in taken {
        if !dropped.contains(&id) {
            live.insert(id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn re_notified_waiting_id_survives_the_stale_drop() {
        let mut pending = PendingPass::default();
        pending.notify_runnable(7);
        pending.notify_runnable(3);
        let pass = pending.begin_runnable();
        assert!(pass.ids.contains(&7));
        assert!(pass.ids.contains(&3));
        assert!(!pending.runnable.contains(&7));
        pending.notify_runnable(7);
        pending.finish_runnable(pass, &[7]);
        assert!(pending.runnable.contains(&7));
        assert!(pending.runnable.contains(&3));
    }

    #[test]
    fn finalize_check_queued_during_the_pass_stays() {
        let mut pending = PendingPass::default();
        let pass = pending.begin_finalize();
        assert!(pass.ids.is_empty());
        pending.notify_finalize(9);
        pending.finish_finalize(pass, &[9]);
        assert!(pending.finalize.contains(&9));
    }
}
