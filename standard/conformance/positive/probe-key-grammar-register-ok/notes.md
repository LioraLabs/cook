§{cat.probes.decl}, CS-0201. Five sites named a probe key and no two agreed:

| key | `probe` decl | `cook.probe()` | `$<ref>` | `seal` | `ingredients` |
|---|---|---|---|---|---|
| `plain` | yes | yes | yes | yes | yes |
| `demo:ver` | yes | yes | yes | yes | yes |
| `cc-version` | yes | yes | yes | NO | NO |
| `cc.version` | yes | yes | yes | NO | NO |
| `cc:find:raylib` | NO | yes | yes | NO | yes |

So `probe cc-version` declared a key that could be referenced from a command
and then neither sealed nor consumed, and `cc:find:raylib` — three segments,
and `cook_cc`'s ordinary case, minted through the unvalidated `cook.probe()`
path — could not be sealed at all. Sealing a discovered library is exactly the
pin a cache-trust story exists to offer.

The two rejection sites also blamed different things for the same cause:
`seal` reported "malformed probe ref" while `ingredients` reported "unexpected
trailing content '-version'", because its scanner stopped at the hyphen and
called the remainder a trailer. One grammar and one diagnostic now.

Carried `register_ok.txt` rather than `parse.txt` alone: the point is that
these keys survive registration and reach the probe registry, and a parse-only
case would pass against an implementation that accepted the spelling and then
failed to resolve it.
