local M = {}

local function parse_int(s) return tonumber(s, 10) end

function M.parse(s)
    if type(s) ~= "string" or s == "" then return nil end
    local trunc = s:match("^([^+]+)") or s
    local core, prerelease = trunc:match("^([^-]+)%-?(.*)$")
    if not core then return nil end
    if prerelease == "" then prerelease = nil end
    local major, minor, patch = core:match("^(%d+)%.?(%d*)%.?(%d*)$")
    if not major then return nil end
    local M_, m_, p_ = parse_int(major), parse_int(minor or "") or 0, parse_int(patch or "") or 0
    if not M_ then return nil end
    return { major = M_, minor = m_, patch = p_, prerelease = prerelease }
end

local OPERATORS = { ">=", "<=", ">", "<", "=" }

local function strip_whitespace(s) return (s or ""):gsub("%s+", "") end

local function split_clauses(constraint)
    if constraint == "" then return {} end
    local clauses = {}
    for piece in constraint:gmatch("[^,]+") do
        clauses[#clauses + 1] = piece
    end
    return clauses
end

local function parse_clause(piece)
    for _, op in ipairs(OPERATORS) do
        if piece:sub(1, #op) == op then
            local v = M.parse(piece:sub(#op + 1))
            if not v then error("[cc.find] unparseable version in clause: " .. piece) end
            return op, v
        end
    end
    local v = M.parse(piece)
    if not v then error("[cc.find] unparseable version in clause: " .. piece) end
    return "=", v
end

local function cmp_core(a, b)
    if a.major ~= b.major then return a.major - b.major end
    if a.minor ~= b.minor then return a.minor - b.minor end
    return a.patch - b.patch
end

local function matches_eq(detected, constraint_str)
    local maj, min, pat = constraint_str:match("^(%d+)%.?(%d*)%.?(%d*)$")
    if not maj then return false end
    if tonumber(maj) ~= detected.major then return false end
    if min ~= "" and tonumber(min) ~= detected.minor then return false end
    if pat ~= "" and tonumber(pat) ~= detected.patch then return false end
    return true
end

local function check_clause(detected, op, constraint_v, raw_v_string)
    local c = cmp_core(detected, constraint_v)
    if detected.prerelease and not constraint_v.prerelease and c == 0 then return false end
    if op == ">=" then return c >= 0
    elseif op == ">"  then return c >  0
    elseif op == "<=" then return c <= 0
    elseif op == "<"  then return c <  0
    elseif op == "="  then return matches_eq(detected, raw_v_string)
    end
    return false
end

function M.satisfies(detected_str, constraint_str)
    constraint_str = strip_whitespace(constraint_str)
    local clauses = split_clauses(constraint_str)
    if #clauses == 0 then return true end
    local detected = M.parse(detected_str)
    if not detected then return false end
    for _, piece in ipairs(clauses) do
        local op, v = parse_clause(piece)
        local raw
        for _, candidate in ipairs(OPERATORS) do
            if piece:sub(1, #candidate) == candidate then raw = piece:sub(#candidate + 1); break end
        end
        raw = raw or piece
        if not check_clause(detected, op, v, raw) then return false end
    end
    return true
end

return M
