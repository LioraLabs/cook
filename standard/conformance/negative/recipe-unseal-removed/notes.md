Pins §8.3 step-dispatch rule 3 (CS-0225): `unseal` + separator in recipe-body
position is rejected as removed, with the migration advice that the ref is simply
omitted from the recipe's `seal` set.

The fixture places `unseal cc` after a `seal cc` step so the rejection is reached
in the shape a migrating Cookfile actually has — a recipe that seals a set and then
subtracts from it. That is the construct CS-0225 deleted, and the diagnostic's
value is the sentence telling the author what replaces the subtraction. Pinning
only "was removed" would let the advice rot.

Positive counterparts (the seal set that survives) are
`positive/cook-disposition-seal-set` and `positive/cook-disposition-seal-all-units`.

COOK-485.
