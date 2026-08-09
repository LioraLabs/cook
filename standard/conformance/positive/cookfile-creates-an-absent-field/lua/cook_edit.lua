-- Standard §22.13 / CS-0221: an absent field is created only when the caller
-- asks for it, and creating one is still an insertion of bytes.
--
-- The fixture asserts inside the module rather than diffing against a golden
-- file, so a failure names WHICH property broke. Same reasoning as
-- `cookfile-splice-preserves-comments`, whose shape this follows.

local M = {}

-- A no-op target maker, so the recipe body is a real module call that also
-- runs. The edit is what is under test; the maker does nothing.
function M.bin(_) end

-- Referenced by the recipe body deliberately: a re-rendering editor would
-- resolve it to a literal. Nothing evaluates it, so it need only exist.
cxx_std = "c++20"

-- Edit a COPY, never the tracked Cookfile. The corpus is run repeatedly, and a
-- fixture that edits a tracked file in place is dirty from its first failure
-- onward.
local SCRATCH = "Cookfile.create-fixture"
local original = fs.read("Cookfile")
fs.write(SCRATCH, original)

local ok, err = pcall(function()
    -- 1. The default is unchanged: an absent field is a total failure, and the
    --    file is byte-identical afterwards. This is the property the create
    --    mode had to be opt-in to preserve.
    local refused = pcall(cook.cookfile.splice_field, SCRATCH, "game", "links", '"math"')
    if refused then
        error("cookfile-create: an absent field must still refuse by default", 0)
    end
    if fs.read(SCRATCH) ~= original then
        error("cookfile-create: a refused edit changed the file", 0)
    end

    -- 2. `field_entries` answers the same question without writing.
    if cook.cookfile.field_entries(SCRATCH, "game", "links") ~= nil then
        error("cookfile-create: field_entries must be nil for an absent field", 0)
    end

    -- 3. Asked to create, it writes the field.
    cook.cookfile.splice_field(SCRATCH, "game", "links", '"math"', { create_if_absent = true })
    local after = fs.read(SCRATCH)

    -- The author writes one field per line, so the created field takes its own
    -- line at the same indentation and repeats the trailing comma. Appending
    -- `, links = { … }` after `cxx_std,` would restyle the call.
    if not after:find('\n        links = { "math" },\n', 1, true) then
        error("cookfile-create: the created field did not match the author's layout; got:\n"
            .. after, 0)
    end

    -- Everything a decode/re-encode destroys is still here, and the file grew
    -- by exactly the inserted bytes — the general form of that claim.
    if not after:find("-- entry point", 1, true) then
        error("cookfile-create: the comment did not survive the edit", 0)
    end
    if not after:find("standard = cxx_std,", 1, true) then
        error("cookfile-create: non-literal Lua was evaluated away", 0)
    end
    if #after ~= #original + #'\n        links = { "math" },' then
        error("cookfile-create: edit changed bytes outside the insertion", 0)
    end

    -- 4. What it created is ordinary input to the next edit: the second
    --    `cc.link` takes the append path, not the create path.
    local entries = cook.cookfile.field_entries(SCRATCH, "game", "links")
    if #entries ~= 1 or entries[1] ~= '"math"' then
        error("cookfile-create: field_entries did not read back the created entry", 0)
    end
    cook.cookfile.splice_field(SCRATCH, "game", "links", '"sound"', { create_if_absent = true })
    if not fs.read(SCRATCH):find('links = { "math", "sound" },', 1, true) then
        error("cookfile-create: the second entry did not append into the created field", 0)
    end

    -- 5. Creating relaxes nothing else. A field that is there and is not a
    --    list is still refused, because creating a second key of that name
    --    would discard the author's value without a word.
    local before = fs.read(SCRATCH)
    local mangled = pcall(cook.cookfile.splice_field, SCRATCH, "game", "standard", '"c++23"',
        { create_if_absent = true })
    if mangled then
        error("cookfile-create: a non-list field must be refused even when creating", 0)
    end
    if fs.read(SCRATCH) ~= before then
        error("cookfile-create: a refusal under the create policy changed the file", 0)
    end
end)

-- Unconditional (CS-0180): a failing assertion must still leave the tree clean.
fs.remove(SCRATCH)

if not ok then error(err, 0) end

return M
