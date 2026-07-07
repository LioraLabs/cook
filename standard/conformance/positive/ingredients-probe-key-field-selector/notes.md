COOK-190 / CS-0125: the three-segment `ingredients` source-ref form — a
two-segment probe key (`ns:cards`) plus one trailing `:items` field
selector. Lands in the AST verbatim as `ForEach source=ProbeKey("ns:cards:items")`;
§22.5.10 resolution binds the final segment as the field selector because
no probe named `ns:cards:items` is declared.
