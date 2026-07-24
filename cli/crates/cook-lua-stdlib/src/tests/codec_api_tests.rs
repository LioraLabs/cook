use super::*;

fn make_lua() -> Lua {
    let lua = Lua::new();
    let cook = lua.create_table().unwrap();
    register_codec_api(&lua, &cook).unwrap();
    lua.globals().set("cook", cook).unwrap();
    lua
}

#[test]
fn test_json_decode_object() {
    let lua = make_lua();
    lua.load(r#"
            local t = cook.json_decode('{"name":"foo","version":1,"active":true,"items":[1,2,3]}')
            assert(t.name == "foo")
            assert(t.version == 1)
            assert(t.active == true)
            assert(t.items[1] == 1)
            assert(t.items[2] == 2)
            assert(t.items[3] == 3)
        "#)
    .exec()
    .unwrap();
}

#[test]
fn test_json_decode_null() {
    let lua = make_lua();
    lua.load(r#"
            local t = cook.json_decode('{"a":null}')
            assert(t.a == nil)
        "#)
    .exec()
    .unwrap();
}

#[test]
fn test_json_decode_nested() {
    let lua = make_lua();
    lua.load(r#"
            local t = cook.json_decode('{"scripts":{"build":"tsc","test":"jest"}}')
            assert(t.scripts.build == "tsc")
            assert(t.scripts.test == "jest")
        "#)
    .exec()
    .unwrap();
}

#[test]
fn test_json_decode_error() {
    let lua = make_lua();
    let result = lua.load(r#"cook.json_decode("not json")"#).exec();
    assert!(result.is_err());
}

#[test]
fn test_yaml_decode_workspace() {
    let lua = make_lua();
    lua.load(r#"
            local t = cook.yaml_decode([[
packages:
  - "packages/*"
catalog:
  typescript: "^5.4.0"
catalogs:
  internal:
    shared-utils: "workspace:*"
    ui: "workspace:*"
]])
            assert(t.packages[1] == "packages/*")
            assert(t.catalog.typescript == "^5.4.0")
            assert(t.catalogs.internal["shared-utils"] == "workspace:*")
            assert(t.catalogs.internal.ui == "workspace:*")
        "#)
    .exec()
    .unwrap();
}

#[test]
fn test_yaml_decode_error() {
    let lua = make_lua();
    let result = lua
        .load(r#"cook.yaml_decode(":\n  :\n    - :")"#)
        .exec();
    assert!(result.is_err());
}
