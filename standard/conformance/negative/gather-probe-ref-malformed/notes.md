CS-0201. An empty segment is still malformed at every site that names a probe
key. This fixture previously pinned `a:b:c:d`, a four-segment key, when the cap
was two; CS-0201 removed the cap (it was enforced on the surface declaration
and ignored by `cook.probe()`, so modules mint `cc:find:raylib` as their
ordinary case) and this case moved to the rule that survived.
