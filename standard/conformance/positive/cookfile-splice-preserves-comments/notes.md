Pins Standard §{lua.cook-cookfile} / CS-0179: an edit inserts bytes and changes
nothing else.

The module asserts each preservation property separately and errors with the
one that broke, rather than comparing against a golden file. A golden
comparison would catch the same failures and report all of them as "bytes
differ", which is the least useful thing to say about a layer whose entire
contract is *which* bytes are allowed to differ.

The three positive assertions are the three things a decode/re-encode
implementation destroys, and it destroys them silently:

  - the `-- entry point` comment, which has nowhere to live in a Lua table;
  - `standard = cxx_std`, which round-trips as whatever the variable evaluated
    to on the editing run;
  - the author's column alignment in `sources`, which a printer reflows.

The length check is the general form: the file grew by exactly the inserted
bytes, so nothing outside the insertion moved even in a way the three specific
checks would miss.

The fixture restores the original text before returning, so it is idempotent —
a conformance corpus is run repeatedly and a fixture that edits a tracked file
in place would pass once and then diff dirty forever.
