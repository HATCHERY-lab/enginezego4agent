// Portions derived from pi-durable unreleased main after v1.5.2, issue #12090, MIT, © 2025 Mario Zechner / Earendil Works. See LICENSES/pi-MIT.txt.

//! Named gate over a suspendable signal, issue #12090.
//!
//! Park stores the waiter error. Cancel does not drop it. The string stays
//! until [`Gate::take_error`]. No async and no store.

/// One parked id and the error stored for it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Waiter {
    id: i64,
    error: String,
}

/// Named gate. The signal value starts absent.
///
/// Waiters stay, error string included, until [`Gate::take_error`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Gate {
    signal: crate::signals::Signal,
    waiters: Vec<Waiter>,
}

impl Gate {
    /// Gate with no value. An empty `name` is refused.
    pub fn open(name: &str) -> Result<Gate, String> {
        Ok(Gate {
            signal: crate::signals::signal(name)?,
            waiters: Vec::new(),
        })
    }

    /// Store `error` for `id`. An empty error is refused.
    ///
    /// The waiter stays until [`Self::take_error`], including across cancel.
    pub fn park(&mut self, id: i64, error: &str) -> Result<(), String> {
        if error.is_empty() {
            return Err("error is empty".to_owned());
        }
        self.waiters.push(Waiter {
            id,
            error: error.to_owned(),
        });
        Ok(())
    }

    /// Set the signal value. An empty value is refused.
    pub fn signal(&mut self, value: &str) -> Result<(), String> {
        if value.is_empty() {
            return Err("signal value is empty".to_owned());
        }
        self.signal.value = Some(value.to_owned());
        Ok(())
    }

    /// Issue #12090. Cancel does not delete the stored error.
    ///
    /// Waiting and not yet signaled stays `wait`. Waiting and signaled is
    /// `cancel`. An id that is not waiting is `idle`.
    pub fn cancel(&mut self, id: i64) -> &'static str {
        let waiting = self.waiters.iter().any(|waiter| waiter.id == id);
        crate::signals::cancel_waits(waiting, self.signal.value.is_some())
    }

    /// Poll one stored error for `id`.
    ///
    /// `None` when that id has nothing left. Not calling this leaves the
    /// string, including after [`Self::cancel`].
    pub fn take_error(&mut self, id: i64) -> Option<String> {
        let index = self.waiters.iter().position(|waiter| waiter.id == id)?;
        Some(self.waiters.remove(index).error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn park_before_signal_stays_wait() {
        let mut gate = Gate::open("gate").expect("name is set");
        gate.park(4, "blocked").expect("error is set");
        assert_eq!(gate.cancel(4), "wait");
        assert_eq!(gate.cancel(9), "idle");
    }

    #[test]
    fn signal_then_cancel_is_cancel_and_the_error_stays() {
        let mut gate = Gate::open("gate").expect("name is set");
        gate.park(4, "blocked").expect("error is set");
        gate.signal("go").expect("value is set");
        assert_eq!(gate.cancel(4), "cancel");
        assert_eq!(gate.cancel(9), "idle");
        assert_eq!(gate.take_error(4), Some("blocked".to_owned()));
        assert_eq!(gate.cancel(4), "idle");
    }

    #[test]
    fn empty_name_error_and_value_are_refused() {
        assert_eq!(Gate::open("").unwrap_err(), "signal name is empty");
        let mut gate = Gate::open("gate").expect("name is set");
        assert_eq!(gate.park(1, "").unwrap_err(), "error is empty");
        assert_eq!(gate.take_error(1), None);
        assert_eq!(gate.signal("").unwrap_err(), "signal value is empty");
        gate.park(1, "blocked").expect("error is set");
        assert_eq!(gate.cancel(1), "wait");
    }
}
