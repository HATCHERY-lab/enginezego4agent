//! Daily paid-spend guard. Free models never touch it.
//!
//! The owner's cap: paid models no more than `EZ4A_BUDGET_USD` US dollars a
//! day (default 10, `none` disables the guard, `0` blocks every paid model).
//! The ledger is one small JSON file — `EZ4A_SPEND_LEDGER`, else
//! `%LOCALAPPDATA%/ez4a/spend.json` — shared by every ez4a process on the
//! host, manager and subagents alike, so the cap binds the whole roster.
//! The day is UTC; costs come from the provider's per-turn usage.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::Error;

const DEFAULT_CAP: f64 = 10.0;

/// Refuse a paid call past the cap. A `:free` model always passes and never
/// reads the ledger.
pub fn check(model: &str) -> Result<(), Error> {
    if !is_paid(model) {
        return Ok(());
    }
    let Some(cap) = cap() else {
        return Ok(());
    };
    let spent = spent_today(&ledger_path(), today_day());
    if cap == 0.0 {
        return Err(Error::message(
            "ez4a daily budget is 0: paid models are off, use a :free model",
        ));
    }
    if spent >= cap {
        return Err(Error::message(format!(
            "ez4a daily budget exhausted: {spent:.4} of {cap:.2} USD spent today — \
use a :free model or raise EZ4A_BUDGET_USD"
        )));
    }
    Ok(())
}

/// Add a paid turn's cost to today's ledger. A free model or a zero cost is a
/// no-op. A ledger that cannot be written charges nothing: the guard fails
/// open, like every other best-effort accounting here.
pub fn charge(model: &str, cost_usd: f64) {
    charge_at(&ledger_path(), today_day(), model, cost_usd);
}

fn charge_at(path: &Path, today: i64, model: &str, cost_usd: f64) {
    if !is_paid(model) || !(cost_usd > 0.0 && cost_usd.is_finite()) {
        return;
    }
    let spent = spent_today(path, today);
    let _ = write(path, today, ((spent + cost_usd) * 1e6).round() / 1e6);
}

/// Paid means: a named model that does not end in `:free`. The
/// `openrouter::` routing prefix is not part of the name.
fn is_paid(model: &str) -> bool {
    let wire = model.trim().strip_prefix("openrouter::").unwrap_or(model.trim());
    !wire.is_empty() && !wire.ends_with(":free")
}

/// Cap in USD: `EZ4A_BUDGET_USD` = number | `0` | `none`. Unparsable falls
/// back to the default, never to an unbounded surprise.
fn cap() -> Option<f64> {
    match std::env::var("EZ4A_BUDGET_USD") {
        Ok(raw) if raw.trim().eq_ignore_ascii_case("none") => None,
        Ok(raw) => Some(raw.trim().parse::<f64>().unwrap_or(DEFAULT_CAP)),
        Err(_) => Some(DEFAULT_CAP),
    }
}

fn ledger_path() -> PathBuf {
    if let Ok(path) = std::env::var("EZ4A_SPEND_LEDGER") {
        if !path.trim().is_empty() {
            return PathBuf::from(path);
        }
    }
    let mut path = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    path.push("ez4a");
    path.push("spend.json");
    path
}

/// What today has already spent. A ledger naming another day, or none at all,
/// has spent nothing.
fn spent_today(path: &Path, today: i64) -> f64 {
    let (day, spent) = read(path);
    if day == today {
        spent
    } else {
        0.0
    }
}

/// `(day, spent_usd)`; a missing or corrupt ledger is day 0, nothing spent.
fn read(path: &Path) -> (i64, f64) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return (0, 0.0);
    };
    let Ok(value) = serde_json::from_str::<Value>(&text) else {
        return (0, 0.0);
    };
    (
        value.get("day").and_then(Value::as_i64).unwrap_or(0),
        value
            .get("spent_usd")
            .and_then(Value::as_f64)
            .filter(|spent| spent.is_finite() && *spent >= 0.0)
            .unwrap_or(0.0),
    )
}

fn write(path: &Path, day: i64, spent_usd: f64) -> Result<(), Error> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|err| Error::message(format!("budget ledger: {err}")))?;
    }
    let body = json!({ "day": day, "spent_usd": spent_usd });
    std::fs::write(path, body.to_string())
        .map_err(|err| Error::message(format!("budget ledger: {err}")))
}

/// UTC day as YYYYMMDD — the ledger's date. Civil-from-days arithmetic
/// (Howard Hinnant's algorithm), no date crate.
fn day_from_unix(secs: i64) -> i64 {
    let days = secs.div_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    y * 10_000 + m * 100 + (doy - (153 * mp + 2) / 5 + 1)
}

fn today_day() -> i64 {
    day_from_unix(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_secs() as i64)
            .unwrap_or(0),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_models_are_never_paid() {
        assert!(!is_paid("cohere/north-mini-code:free"));
        assert!(!is_paid("openrouter::google/gemma-4-31b-it:free"));
        assert!(is_paid("openai/gpt-5.1-mini"));
        assert!(is_paid("openrouter::openai/gpt-5.1-mini"));
        assert!(!is_paid("  "));
    }

    #[test]
    fn the_calendar_math_matches_known_days() {
        assert_eq!(day_from_unix(0), 1970_01_01);
        assert_eq!(day_from_unix(951_782_400), 2000_02_29);
        assert_eq!(day_from_unix(1_763_712_000), 2025_11_21);
        assert_eq!(day_from_unix(1_763_712_000 + 86_400 * 3 - 1), 2025_11_24);
    }

    #[test]
    fn charge_accumulates_and_a_new_day_resets() {
        let dir = std::env::temp_dir().join(format!("ez4a-budget-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let ledger = dir.join("spend.json");

        charge_at(&ledger, 2026_11_17, "openai/gpt-5.1-mini", 2.5);
        charge_at(&ledger, 2026_11_17, "openai/gpt-5.1-mini", 1.5);
        assert_eq!(read(&ledger), (2026_11_17, 4.0));

        // Free models and zero costs charge nothing.
        charge_at(&ledger, 2026_11_17, "cohere/north-mini-code:free", 9.0);
        charge_at(&ledger, 2026_11_17, "openai/gpt-5.1-mini", 0.0);
        assert_eq!(read(&ledger).1, 4.0);

        // Yesterday's spend does not count against today.
        assert_eq!(spent_today(&ledger, 2026_11_18), 0.0);
        // A ledger that is not there has spent nothing either.
        assert_eq!(spent_today(&dir.join("nothing.json"), 2026_11_17), 0.0);

        std::fs::write(dir.join("bad.json"), "{not json").unwrap();
        assert_eq!(read(&dir.join("bad.json")), (0, 0.0));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
