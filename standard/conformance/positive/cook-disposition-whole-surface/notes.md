# cook-disposition-whole-surface

Pins Example 8.4.3.1 ("the whole surface once") verbatim, so the Standard's
flagship disposition example cannot stop parsing without a test going red.

It had stopped. The example annotated its `gather`, `seal` and `cook` lines with
trailing `#` comments, which §3.6 forbids — a `#` is a comment only at the head
of a line — so every one of the four declarative line kinds rejected it, each
with its own diagnostic. Underneath that, the `cc …` bodies named neither
`$<in>` nor `$<out>`, so the example also tripped the hard error CS-0224 added
to the very chapter it illustrates.

Nothing else reads an MDX fence. This fixture is why the composite stays honest:
the individual features are each pinned by a `cook-disposition-*` sibling, and
what rotted was the one place they appear together. (COOK-486, CS-0231.)
