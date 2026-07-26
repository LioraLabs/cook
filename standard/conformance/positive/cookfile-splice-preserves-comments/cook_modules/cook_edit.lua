-- Standard §22.12 / CS-0179: a module edits a Cookfile structurally, without
-- re-rendering it.
--
-- The fixture asserts inside the module rather than diffing against a golden
-- file, so a failure names WHICH preservation property broke. A golden
-- comparison catches the same failures and reports every one of them as
-- "bytes differ" — the least useful thing to say about a layer whose entire
-- contract is which bytes are allowed to differ.

local M = {}

-- A no-op target maker, so the recipe body is a real module call that also
-- runs. The edit is what is under test; the maker does nothing.
function M.bin(_) end

-- The recipe body references `cxx_std` deliberately: a re-rendering editor
-- would resolve it to a literal. Nothing evaluates it, so it need only exist.
cxx_std = "c++20"

-- Edit a COPY, never the tracked Cookfile. The corpus is run repeatedly, so a
-- fixture that edits a tracked file in place is dirty from its first failure
-- onward — and it accumulates, since the next run splices into an already
-- spliced file. An earlier draft did exactly that; it surfaced when the
-- fixture was deliberately broken to check it could fail at all.
local SCRATCH = "Cookfile.splice-fixture"
local original = fs.read("Cookfile")
fs.write(SCRATCH, original)

local ok, err = pcall(function()
    cook.cookfile.splice_field(SCRATCH, "app", "links", '"physlib"')
    local after = fs.read(SCRATCH)

    if not after:find('{ "mathlib", "physlib" }', 1, true) then
        error("cookfile-splice: entry not spliced into links; got:\n" .. after, 0)
    end

    -- The three things a decode/re-encode round trip destroys, silently.
    if not after:find("-- entry point", 1, true) then
        error("cookfile-splice: the comment did not survive the edit", 0)
    end
    if not after:find("standard = cxx_std,", 1, true) then
        error("cookfile-splice: non-literal Lua was evaluated away", 0)
    end
    if not after:find('sources = { "src/main.cpp" },', 1, true) then
        error("cookfile-splice: the author's column alignment was reflowed", 0)
    end

    -- The general form of the same claim: the file grew by exactly the
    -- inserted bytes, so nothing outside the insertion moved — including in
    -- ways the three checks above would not notice.
    if #after ~= #original + #', "physlib"' then
        error("cookfile-splice: edit changed bytes outside the insertion", 0)
    end
end)

-- Unconditional (CS-0180): a failing assertion must still leave the tree clean.
fs.remove(SCRATCH)

if not ok then error(err, 0) end

return M
