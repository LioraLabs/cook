# 02 — pipeline

One recipe, three `cook` steps, three iteration modes:

| step | pattern | mode |
|---|---|---|
| `cook "build/counts/$<in.stem>.count"` | per-input | **fan-out** — one unit per chapter |
| `cook "build/total.txt"` with `$<all>` | collects the previous step's outputs | **many-to-one** |
| `cook "build/report.txt" "build/report.csv"` | `$<out_1>`, `$<out_2>` | **multi-output** |

The rule that makes the caching honest: each step consumes the previous one
**through declared placeholders**. `$<all>` isn't just convenient — it's how
cook knows the total depends on every count, so a one-chapter edit rebuilds
exactly `two.count → total → report` and nothing else:

```
$ cook report
  report  done (5/5)

$ cook report
  report  done (5/5 cached)

$ sed -i 's/moon/sun and stars/' chapters/two.txt
$ cook report
  report  done (2/5 cached)      # one.count, three.count untouched
```

If a body instead read a file cook doesn't know about (`$(cat some/file)`
with no placeholder), cook couldn't see it change — declare what you read.

`cook why report` prints every unit's cache key with each determinant
(inputs, command, env) attributed — the tool to reach for whenever a
rebuild (or a cache hit) surprises you.
