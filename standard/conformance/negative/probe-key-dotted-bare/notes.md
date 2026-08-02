CS-0201. `.` is member access in a probe reference, so it is not part of a
probe key: with `.` inside a segment, `$<demo:cc-version.ver>` is either field
`ver` of `demo:cc-version` or the key `demo:cc-version.ver`, and nothing in the
text separates them.

CS-0131 admitted `.` to `PROBE_SEG` before probe references had member access.
The two features are incompatible and member access is worth more, so the dot
left the key grammar. It stays in `TOOL_NAME`, which was the same production
until CS-0201 split them: an executable name may carry a dot (`python3.11`) and
is never member-accessed.

The diagnostic must SAY this. "Malformed key" over a spelling the declaration
used to accept reads as a typo rather than a rule, so the message names member
access and offers the quoted form.
