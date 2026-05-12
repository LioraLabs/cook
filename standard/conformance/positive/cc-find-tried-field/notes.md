# cc-find-tried-field

Locks §9.2.3.8 `FindResult.tried` shape at the parse level. The runtime
assertion is `type(r.tried) == "table"` — adequate for parse-fixture surface
checks. Full Attempt-record shape is verified by the busted suite (each
finder spec) and will be re-locked here once SHI-210 lands.
