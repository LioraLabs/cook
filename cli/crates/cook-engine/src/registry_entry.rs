use std::collections::BTreeMap;
use std::path::PathBuf;

/// A per-Cookfile registry bundle: the runtime registry, the Lua source generated
/// from the Cookfile, and the importer-relative alias paths for `cook.dep_output`
/// substitution (Phase 7).
pub struct RegistryEntry {
    pub registry: cook_register::RegisterSessionBuilder,
    pub lua_source: String,
    pub alias_dirs: BTreeMap<String, PathBuf>,
}
