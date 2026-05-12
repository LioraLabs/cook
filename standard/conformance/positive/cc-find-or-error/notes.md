# cc-find-or-error

Locks §9.2.3.13 `cc.find_or_error` surface — parses identically to `cc.find`
at the Cookfile level. Runtime miss-raises behaviour is exercised by busted
in cook-modules/cook_cc/spec/ and would be re-locked here once SHI-210 lands.
