use super::*;

fn setup() -> Lua {
    let lua = Lua::new();
    let cook = lua.create_table().unwrap();
    register_platform_api(&lua, &cook).unwrap();
    lua.globals().set("cook", cook).unwrap();
    lua
}

#[test]
fn os_matches_host() {
    let lua = setup();
    let os: String = lua.load("return cook.platform.os").eval().unwrap();
    assert_eq!(os, std::env::consts::OS);
}

#[test]
fn arch_matches_host() {
    let lua = setup();
    let arch: String = lua.load("return cook.platform.arch").eval().unwrap();
    assert_eq!(arch, std::env::consts::ARCH);
}
