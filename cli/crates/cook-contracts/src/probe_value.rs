//! Shared msgpack encode/decode helpers for the probe-value store (§22.5.4).
//!
//! These functions are the canonical serialisation layer used by every crate
//! that reads from or writes to the per-run probe-value store:
//!
//!  - `cook-register` (Lua→msgpack at registration time via `lua_to_msgpack`)
//!  - `cook-luaotp`   (worker dispatch + execute-VM `cook.cache.get` decode)
//!  - `cook-engine`   (scheduler populate: no direct encode/decode needed)
//!
//! The Lua→msgpack walker (`lua_to_msgpack`) is NOT here because it requires
//! an mlua dependency that cook-contracts intentionally avoids. Each crate
//! that needs that function provides its own copy (cook-register/probe_value.rs,
//! cook-luaotp/probe_value.rs).

use rmpv::Value as MsgPackValue;

/// Encode an rmpv::Value to msgpack bytes.
pub fn encode_msgpack(v: &MsgPackValue) -> Vec<u8> {
    let mut buf = Vec::new();
    rmpv::encode::write_value(&mut buf, v).expect("rmpv encode never fails for in-memory");
    buf
}

/// Decode msgpack bytes into an rmpv::Value.
pub fn decode_msgpack(bytes: &[u8]) -> Result<MsgPackValue, String> {
    let mut cursor = std::io::Cursor::new(bytes);
    rmpv::decode::read_value(&mut cursor).map_err(|e| format!("msgpack decode: {}", e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmpv::Value;

    #[test]
    fn encode_decode_bool() {
        let v = Value::Boolean(true);
        let bytes = encode_msgpack(&v);
        assert_eq!(decode_msgpack(&bytes).unwrap(), v);
    }

    #[test]
    fn encode_decode_integer() {
        let v = Value::Integer(42.into());
        let bytes = encode_msgpack(&v);
        assert_eq!(decode_msgpack(&bytes).unwrap(), v);
    }

    #[test]
    fn encode_decode_map() {
        let v = Value::Map(vec![
            (Value::String("found".into()), Value::Boolean(true)),
            (
                Value::String("cflags".into()),
                Value::Array(vec![Value::String("-I/usr/include".into())]),
            ),
        ]);
        let bytes = encode_msgpack(&v);
        assert_eq!(decode_msgpack(&bytes).unwrap(), v);
    }

    #[test]
    fn encode_decode_nil() {
        let v = Value::Nil;
        let bytes = encode_msgpack(&v);
        assert_eq!(decode_msgpack(&bytes).unwrap(), v);
    }

    #[test]
    fn decode_invalid_bytes_returns_error() {
        let bad = vec![0xC1]; // 0xC1 is never-used in msgpack
        assert!(decode_msgpack(&bad).is_err());
    }
}
