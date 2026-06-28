COOK-171: trailing per-unit `seal`/`unseal` adjust the recipe-level baseline for
one cook unit: effective(unit) = (base ∪ step_seals) − step_unseals.
base={a,b}, +seal c, −unseal a  →  {b, c} (sorted, de-duplicated).
