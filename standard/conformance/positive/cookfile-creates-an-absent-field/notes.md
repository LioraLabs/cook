Pins Standard §{lua.cook-cookfile} / CS-0221: an absent field is created only
when the caller asks for it, and creating one changes nothing else.

The sibling fixture `cookfile-splice-preserves-comments` pins what an edit
preserves. This one pins what the create mode is allowed to relax, which is
exactly one case, and it asserts the boundary in both directions:

  - the same call without the options table still refuses and still leaves the
    file byte-identical, which is the reason the mode is opt-in rather than the
    new default;
  - `field_entries` answers "is there a field here" without writing, which is
    how a verb decides between creating and appending — and how it stays
    idempotent, since the alternative is searching the call text for the entry
    and matching inside `sources = { "src/math/main.cpp" }`;
  - a field that IS there and is not a `{ … }` list is still refused under the
    create policy. That is the case where creating would look like it worked:
    the call would carry two keys of the same name, the later one winning, and
    the author's value discarded silently.

The layout assertion is the part a spec cannot state as "it looks right". The
Cookfile writes one field per line, so the created field takes its own line at
the same indentation and repeats the trailing comma. Appending
`, links = { "math" }` after `standard = cxx_std,` would be a legal edit that
quietly restyles the author's call, which is the same class of damage §22.13
forbids elsewhere.

The length check is the general form of the preservation claim: the file grew
by exactly the inserted bytes, so nothing outside the insertion moved even in a
way the specific checks would miss.

The fixture edits a copy and removes it unconditionally, so the corpus stays
idempotent under repeated runs and under its own failure.
