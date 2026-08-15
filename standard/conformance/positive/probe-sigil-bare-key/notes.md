Pins CS-0240 §{xref.resolution} step 3: a sigil whose IDENT names a declared
probe key is a probe-value reference even with no colon in it.

Under CS-0074 the colon WAS the dispatch, so this Cookfile registered
`$<keyed_obs>` as a declared-variable lookup and the register phase refused it
with `no config block declares 'keyed_obs'` — for a probe declared four lines
above, never mentioned by the diagnostic. `register_ok.txt` is what grades it:
the resolution happens at codegen and the refusal fires when the recipe body
runs, so a register-phase pass over this file is a genuine before/after gate,
not a parse formality.
