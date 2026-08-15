Pins §22.5.2 and A-grammar §A.5 (CS-0222): the `files { … }` probe producer is
removed, and the rejection MUST name the top-level `files NAME` declaration as its
replacement.

The twin of `probe-producer-tools-removed`. Both spellings get their own fixture
because A-grammar requires that *each* retired spelling receive a diagnostic; the
parser happens to answer both from one message today, and a fixture for only one
of them could not tell a shared message from a half-implemented rejection.

Distinct from `files-lua-block` and `probe-files-unquoted-entry`, which constrain
the surviving top-level `files NAME` declaration. This one is about the position
the declaration may no longer occupy.

COOK-485.
