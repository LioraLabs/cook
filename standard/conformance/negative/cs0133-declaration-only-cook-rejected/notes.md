Pins the CS-0133 removal of declaration-only `cook` steps: a body-less
`cook "bin/app"` (the former "declare the output, build it via the following
shell_command" idiom, ex-Example 8.4.2) is now a parse error. The form
registered zero work units and never executed its follow-on command; the
diagnostic MUST name CS-0133 so migrating users fold the shell command into a
`cook "out" { … }` body. §8.4 / App. A.4 ("Cook body is mandatory").
