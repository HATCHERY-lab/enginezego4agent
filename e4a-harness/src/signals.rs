// Portions derived from pi-durable unreleased main after v1.5.2, MIT, © 2025 Mario Zechner / Earendil Works. See LICENSES/pi-MIT.txt.

//! engine4agent suspendable signals, issue 12090.
//!
//! Named signal, park and resume phase names, and the cancel-wait decision.
//! No runtime, no waiter object, and no process.

/// Named signal. `value` stays absent until a later writer sets it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signal {
    pub name: String,
    pub value: Option<String>,
}

/// Signal with no value. An empty `name` is refused.
pub fn signal(name: &str) -> Result<Signal, String> {
    if name.is_empty() {
        return Err("signal name is empty".to_owned());
    }
    Ok(Signal {
        name: name.to_owned(),
        value: None,
    })
}

/// `call` becomes `parked`. Any other phase is an error.
pub fn park(phase: &str) -> Result<&'static str, String> {
    if phase == "call" {
        Ok("parked")
    } else {
        Err(format!("park accepts call, got {phase}"))
    }
}

/// `parked` becomes `call`. Any other phase is an error.
pub fn resume(phase: &str) -> Result<&'static str, String> {
    if phase == "parked" {
        Ok("call")
    } else {
        Err(format!("resume accepts parked, got {phase}"))
    }
}

/// Issue #12090. Cancel does not drop the waiter.
///
/// Waiting and not ready stays `wait`. Waiting and ready is `cancel`.
/// Not waiting is `idle`, whether or not a signal is ready.
pub fn cancel_waits(waiting: bool, signal_ready: bool) -> &'static str {
    match (waiting, signal_ready) {
        (true, true) => "cancel",
        (true, false) => "wait",
        (false, _) => "idle",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signal_refuses_an_empty_name() {
        assert_eq!(signal("").unwrap_err(), "signal name is empty");
        let made = signal("gate").unwrap();
        assert_eq!(made.name, "gate");
        assert_eq!(made.value, None);
    }

    #[test]
    fn park_and_resume_only_swap_call_and_parked() {
        assert_eq!(park("call").unwrap(), "parked");
        assert_eq!(resume("parked").unwrap(), "call");
        assert_eq!(park("parked").unwrap_err(), "park accepts call, got parked");
        assert_eq!(park("done").unwrap_err(), "park accepts call, got done");
        assert_eq!(resume("call").unwrap_err(), "resume accepts parked, got call");
        assert_eq!(
            resume("").unwrap_err(),
            "resume accepts parked, got "
        );
    }

    #[test]
    fn cancel_stays_wait_until_the_signal_is_ready() {
        assert_eq!(cancel_waits(true, false), "wait");
        assert_eq!(cancel_waits(true, true), "cancel");
        assert_eq!(cancel_waits(false, false), "idle");
        assert_eq!(cancel_waits(false, true), "idle");
    }
}
