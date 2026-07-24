//! Register-phase `cook.on_register_complete(fn)` binding (Standard §22.9,
//! CS-0149).
//!
//! `cook.on_register_complete` only queues `fn` here — it does not run it.
//! The queue is drained by `engine.rs`'s `register_cookfile`, at step 12c,
//! once every recipe body of the pass has been evaluated and the pass's
//! whole-graph validation has completed. `list_names` installs this same
//! API over its own queue but never drains it (see the comment at its call
//! site in `engine.rs`).

use mlua::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

/// Queue of callbacks registered via `cook.on_register_complete`, in
/// registration order.
///
/// `mlua::Function` (mlua 0.10) is lifetime-free and storable across calls,
/// so the queue can hold callbacks past the `cook.on_register_complete`
/// call that registered them, for the draining loop in `engine.rs` to
/// invoke later. `Rc<RefCell<..>>` because that install-time closure and
/// the later drain both need to reach the same queue, and the register VM
/// is single-threaded.
pub type SharedFinalizerQueue = Rc<RefCell<Vec<mlua::Function>>>;

/// Uniform register-phase type error for `cook.on_register_complete`'s sole
/// argument (Standard §22.9, CS-0149): a non-function value is a hard error
/// naming the API, the accepted type, and the received Lua type — never
/// silently coerced or discarded. Mirrors `test_api::type_err`.
fn type_err(got: &str) -> LuaError {
    LuaError::runtime(format!(
        "cook.on_register_complete: `fn` must be a function, got {got} (Standard \u{00a7}22.9, CS-0149)"
    ))
}

/// Register `cook.on_register_complete(fn)` on the cook global table
/// (Standard §22.9, CS-0149).
///
/// `fn` MUST be a Lua function; anything else is rejected via [`type_err`].
/// A conforming `fn` is appended to `queue` and returns — no other work
/// happens here. Ordering, the outside-a-recipe-body guarantee, the
/// recipe/probe-registration ban, and the fires-exactly-once contract all
/// come from how and where `engine.rs` drains the queue, not from this
/// installer.
pub fn register_on_register_complete(lua: &Lua, queue: SharedFinalizerQueue) -> LuaResult<()> {
    let cook: LuaTable = lua.globals().get("cook")?;
    let on_register_complete_fn = lua.create_function(move |_, value: LuaValue| match value {
        LuaValue::Function(f) => {
            queue.borrow_mut().push(f);
            Ok(())
        }
        other => Err(type_err(other.type_name())),
    })?;
    cook.set("on_register_complete", on_register_complete_fn)?;
    Ok(())
}

#[cfg(test)]
#[path = "tests/on_register_api_tests.rs"]
mod tests;
