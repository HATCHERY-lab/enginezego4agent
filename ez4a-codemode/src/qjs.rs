//! Optional QuickJS evaluation. No animation host and no network.

#[cfg(feature = "quickjs")]
use rquickjs::{Coerced, Context, Ctx, Error, FromJs, Runtime, Value};

/// Evaluate `code` and return its text.
///
/// Without the `quickjs` feature the engine is not linked.
/// With the feature, an empty `code` is `empty` and a thrown exception is its text.
/// A string result is that string. Any other value is its JavaScript string form.
pub fn eval_source(code: &str) -> Result<String, String> {
    #[cfg(not(feature = "quickjs"))]
    {
        let _ = code;
        Err("js runtime not linked".to_owned())
    }
    #[cfg(feature = "quickjs")]
    {
        eval_linked(code)
    }
}

#[cfg(feature = "quickjs")]
fn eval_linked(code: &str) -> Result<String, String> {
    if code.is_empty() {
        return Err("empty".to_owned());
    }
    let runtime = Runtime::new().map_err(|e| e.to_string())?;
    // Declared after the runtime so it drops first and detaches before the runtime.
    let _jit = attach_jit(&runtime);
    let ctx = Context::full(&runtime).map_err(|e| e.to_string())?;
    ctx.with(|c| match c.eval::<Value, _>(code) {
        Ok(value) => value_text(&c, value),
        Err(error) => Err(exception_text(&c, error)),
    })
}

/// JavaScript `ToString`. A string stays its contents; other values use the engine's string form.
#[cfg(feature = "quickjs")]
fn value_text<'js>(ctx: &Ctx<'js>, value: Value<'js>) -> Result<String, String> {
    match Coerced::<String>::from_js(ctx, value) {
        Ok(text) => Ok(text.0),
        Err(error) => Err(exception_text(ctx, error)),
    }
}

/// Native codegen when this target has it. A failed attach leaves the interpreter.
#[cfg(feature = "quickjs")]
fn attach_jit(rt: &Runtime) -> Option<rquickjs_jit::Jit> {
    let jit = rquickjs_jit::Jit::attach(rt, rquickjs_jit::JitConfig::default()).ok()?;
    match jit.require_native() {
        Ok(()) => Some(jit),
        Err(rquickjs_jit::JitError::UnsupportedPlatform) => Some(jit),
        Err(_) => None,
    }
}

#[cfg(feature = "quickjs")]
fn exception_text(ctx: &Ctx<'_>, error: Error) -> String {
    if let Error::Exception = error {
        let ex = ctx.catch();
        return ex
            .as_exception()
            .and_then(|caught| caught.message())
            .or_else(|| ex.as_string().and_then(|s| s.to_string().ok()))
            .unwrap_or_else(|| format!("{ex:?}"));
    }
    error.to_string()
}

#[cfg(all(test, feature = "quickjs"))]
mod tests {
    use super::eval_source;

    #[test]
    fn script_result_is_text() {
        assert_eq!(eval_source("1+1"), Ok("2".to_owned()));
        assert_eq!(eval_source("\"hi\""), Ok("hi".to_owned()));
        assert_eq!(eval_source(""), Err("empty".to_owned()));
    }
}
