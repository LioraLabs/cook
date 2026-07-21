CS-0159 rules 4 and 9: the first test's effective set is {b, c} — baseline
{a, b}, plus trailing `seal c`, minus trailing `unseal a`. The `unseal` is
per-unit: the sibling test keeps the full baseline {a, b}.
