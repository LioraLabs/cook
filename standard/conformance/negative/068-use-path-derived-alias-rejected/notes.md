Pins CS-0206. `9lives.lua` is a well-formed `use_path`; what fails is the
alias derived from its basename. The diagnostic must name the DERIVED
identifier and direct the author to the explicit form (`use nine_lives
./build/9lives.lua`) — the author never typed `9lives`, so a diagnostic that
complains about a bad `use` name would be complaining about something nobody
wrote (§12.1).
