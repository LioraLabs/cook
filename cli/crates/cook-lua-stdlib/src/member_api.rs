//! `cook.member_to_string(value)` — a data member's canonical string form.
//!
//! Phase: Both (§9.3, §24). A member-fanout recipe's codegen emits
//! `member = cook.member_to_string(item)` for the register VM to evaluate,
//! and the same call is reachable from an execute-phase body, so the two VMs
//! answer the same question about the same value.
//!
//! Until COOK-439 they answered it with two copies. The pure law underneath
//! was already one function — `json_codec::lua_to_json` here, then
//! `cook_contracts::member::member_to_string` a stratum down — but the mlua
//! wrapper around it was written twice, and it carried the door's NAME and
//! the door's DIAGNOSTIC with it. The constitution's clone rule saw the
//! bodies and its literal rule saw the name and the message, which is three
//! findings for one door.
//!
//! What a copy cost here is worth stating, because "the law is already
//! shared" is exactly the argument that makes a wrapper look harmless. The
//! member string is a cache determinant: it names the unit a fan-out member
//! registers, and it is what a `$<in>` placeholder resolves to. Two wrappers
//! that stopped agreeing — one of them widened to accept a value the other
//! rejects, say — would file the same member under two identities, and the
//! failure would look like a cache that stopped hitting rather than like a
//! door with two implementations.

/// Install `cook.member_to_string` on `cook`.
///
/// One implementation for both VMs: `cook-register` installs it while
/// building the register VM's `cook` table, `cook-execute` while building a
/// worker's. The name comes from
/// [`cook_contracts::registration::MEMBER_TO_STRING_NAME`], which is also
/// what `cook-luagen` emits, so emitter and both installers read one
/// constant.
pub fn install_member_to_string(lua: &mlua::Lua, cook: &mlua::Table) -> mlua::Result<()> {
    let member_fn = lua.create_function(|_, value: mlua::Value| {
        let json = crate::json_codec::lua_to_json(&value)
            .map_err(|e| mlua::Error::runtime(format!("cook.member_to_string: {e}")))?;
        Ok(cook_contracts::member::member_to_string(&json))
    })?;
    cook.set(
        cook_contracts::registration::MEMBER_TO_STRING_NAME,
        member_fn,
    )?;
    Ok(())
}

#[cfg(test)]
#[path = "tests/member_api_tests.rs"]
mod tests;
