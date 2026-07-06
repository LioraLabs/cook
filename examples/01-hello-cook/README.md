# 01 — hello, cook

The smallest useful Cookfile: one `recipe`, an `ingredients` glob, and one
`cook` step that fans out — one unit of work per input file.

```
recipe build
    ingredients "notes/*.md"
    cook "out/$<in.stem>.html" { sed ... $<in> > $<out> }
```

- `ingredients "notes/*.md"` declares the recipe's inputs.
- `cook "out/$<in.stem>.html"` declares one output **per input** —
  `$<in.stem>` is the input's basename without extension.
- The `{ ... }` body is shell; `$<in>` and `$<out>` are substituted per unit.

## The point

```
$ cook
  build  done (2/2)          # both notes built

$ cook
  build  done (2/2 cached)   # nothing changed → no work

$ echo "- more" >> notes/monday.md
$ cook
  ... monday.html rebuilt, tuesday.html cached
```

Cook keys each unit on the content of its inputs, its command, and its
declared environment — not on timestamps. That key is the whole caching
model; every other example builds on it.
