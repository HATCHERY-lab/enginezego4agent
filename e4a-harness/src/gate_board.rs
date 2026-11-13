//! Parked turn gates. The tool signals the gate; the model call does not.

use std::collections::BTreeMap;
use std::sync::Mutex;

use crate::gate::Gate;
use crate::Error;

static BOARD: Mutex<BTreeMap<i64, Gate>> = Mutex::new(BTreeMap::new());

fn lock_board() -> Result<std::sync::MutexGuard<'static, BTreeMap<i64, Gate>>, Error> {
    BOARD
        .lock()
        .map_err(|_| Error::message("gate board mutex is poisoned"))
}

/// Open a `turn` gate, park `task_id` with error `tool`, and keep the gate.
pub fn park_turn(task_id: i64) -> Result<(), Error> {
    let mut gate = Gate::open("turn").map_err(Error::message)?;
    gate.park(task_id, "tool").map_err(Error::message)?;
    lock_board()?.insert(task_id, gate);
    Ok(())
}

/// Signal the parked turn gate from outside the model call.
///
/// Missing gate is `gate is not parked`. A poisoned board lock is an error.
/// `cancel` must be `cancel` and the stored error must still be there.
pub fn signal_turn(task_id: i64) -> Result<(), Error> {
    let mut board = lock_board()?;
    let Some(mut gate) = board.remove(&task_id) else {
        return Err(Error::message("gate is not parked"));
    };
    gate.signal("done").map_err(Error::message)?;
    let phase = gate.cancel(task_id);
    let stored = gate.take_error(task_id);
    if phase != "cancel" || stored.is_none() {
        return Err(Error::message(format!("gate phase is {phase}")));
    }
    Ok(())
}
