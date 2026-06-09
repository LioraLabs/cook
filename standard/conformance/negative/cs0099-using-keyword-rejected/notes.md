Pins the CS-0099 migration diagnostic: the `using` keyword between a `cook`
step's output pattern(s) and its body was removed. The body opener (`{` or
`>{`) follows the pattern(s) directly. The diagnostic MUST name CS-0099 and
say the keyword was removed, so migrating users get a sharp pointer rather
than a generic parse error. §8.4 / App. A.4.
