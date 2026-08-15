COOK-190 / CS-0125: a two-segment probe key (`ns:name`, the canonical probe
naming) in gather position lands in the AST verbatim —
`MemberSource source=GatherKey("cards:list")` — not truncated at the colon.
Key-vs-field-selector resolution happens against the declared-probe set at
registration time (§22.5.10): the exact whole-ref key match wins.
