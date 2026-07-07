COOK-190: an `ingredients <probe>` source ref is `probe_ref (":" IDENT)?` —
at most three `:`-separated ident segments (two-segment probe key per
§22.5.2 plus an optional `:field` selector, §22.5.10). Four segments is
malformed and MUST be rejected at parse time.
