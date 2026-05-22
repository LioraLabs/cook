Param names MUST NOT contain '.' (§7.1.1). Without this check, the bare-ident
scan stops at the dot and the trailing `.bar` is silently dropped, surfacing
later as a runtime "unknown placeholder '$<bar>'" error far from the source.
