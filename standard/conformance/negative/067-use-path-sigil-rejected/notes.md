Pins CS-0206. `import` admits the `//` workspace-root sigil; `use` does not,
so `use` is strictly MORE confined than the declaration its path form is
modelled on. That inversion is the entry's central refusal and it is pinned
here so a future "make it consistent with import" change has to argue with a
red test.

The diagnostic must name the sigil rather than call it an absolute path.
`//x` is also `/x`-shaped, and reporting it as absolute would tell an author
to make it relative when the real answer is that `use` does not reach outside
the declaring Cookfile's subtree at all (§12.2.1).
