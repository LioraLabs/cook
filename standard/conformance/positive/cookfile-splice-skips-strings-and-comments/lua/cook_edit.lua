-- Standard §22.13 / CS-0208: a field is a table key at the top level of the
-- call's argument, and a `}` inside a string literal — of ANY spelling —
-- does not close a list.
--
-- Asserted inside the module rather than diffed against a golden file, so a
-- failure names WHICH rule broke. A golden comparison reports all three as
-- "bytes differ", the least useful thing to say about a layer whose whole
-- contract is which bytes are allowed to differ.

local M = {}

-- A no-op target maker. The edit is under test; the maker does nothing.
function M.bin(_) end

local SCRATCH = "Cookfile.splice-keys-fixture"
local original = fs.read("Cookfile")
fs.write(SCRATCH, original)

local ok, err = pcall(function()
    -- The real `links` is the third thing in the file that looks like one.
    cook.cookfile.splice_field(SCRATCH, "app", "links", '"physlib"')
    -- The `}` in `[[src/a}b.cpp]]` must not have closed this list.
    cook.cookfile.splice_field(SCRATCH, "app", "sources", '"src/c.cpp"')
    local after = fs.read(SCRATCH)

    if not after:find('{ "mathlib", "physlib" }', 1, true) then
        error("cookfile-splice: the top-level links list was not edited; got:\n" .. after, 0)
    end
    if not after:find('{ [[src/a}b.cpp]], "src/c.cpp" }', 1, true) then
        error("cookfile-splice: the entry did not land after the long-bracket string:\n" .. after, 0)
    end

    -- The two lookalikes, byte-identical.
    if not after:find('-- links = { "stale" },', 1, true) then
        error("cookfile-splice: the commented-out field was edited", 0)
    end
    if not after:find('opts    = { links = { "nested" } },', 1, true) then
        error("cookfile-splice: the NESTED links list was edited", 0)
    end

    -- The general form: the file grew by exactly the two insertions, so
    -- nothing outside them moved at all.
    local grew = #', "physlib"' + #', "src/c.cpp"'
    if #after ~= #original + grew then
        error("cookfile-splice: edit changed bytes outside the insertions", 0)
    end
end)

-- Unconditional (CS-0180): a failing assertion must still leave the tree clean.
fs.remove(SCRATCH)

if not ok then error(err, 0) end

return M
