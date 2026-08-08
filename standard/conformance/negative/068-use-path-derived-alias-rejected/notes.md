Pins CS-0206. `9lives.lua` is a well-formed `use_path`; what fails is the
alias derived from its basename. The diagnostic must name the DERIVED
identifier and direct the author to the explicit form (`use nine_lives
./build/9lives.lua`) — the author never typed `9lives`, so a diagnostic that
complains about a bad `use` name would be complaining about something nobody
wrote (§12.1).

The recipe body is a well-formed `cook` step rather than the bare
`echo unreachable` this corpus usually writes, and deliberately. A loose shell
command in a recipe body is itself a CS-0134 error, which would make this
fixture "rejected" whether or not the rule it names exists — review caught
exactly that, and the mutation is easy to run: deleting the derived-alias check
left the fixture green off the CS-0134 diagnostic. With a valid body, the only
thing that can reject this Cookfile is the rule it is here for.
