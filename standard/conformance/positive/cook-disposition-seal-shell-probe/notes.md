# cook-disposition-seal-shell-probe

Pins the Session-3 decision (§8.4.3): a seal ref is always a probe name, and
an unconsumed-environment determinant folds by declaring an ordinary shell probe
and sealing it BY NAME (no inline environment ref form). The named `flags` probe
groups both variables and is sealed via `seal flags`, folding its value into the
cook's key. (COOK-172, CS-0117, CS-0226.)
