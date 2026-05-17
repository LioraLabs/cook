Exercises the imperative escape hatch from §28.3.14: `cc.find_or_error`
returns a sigil record where each field is a `$<cc:find:<name>.<field>>`
placeholder string. The register-phase assertions verify the contract
without needing the probe to actually execute.
