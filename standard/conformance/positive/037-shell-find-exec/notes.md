# 037 — find -exec `{}` in a chore body

Pins that the POSIX `find -exec {} \;` idiom inside a `chore` body
passes through the parser as a bare shell command. The `{}` placeholder
used by find is distinct from Cook's `$<IDENT>` substitution syntax
(§{lexical.placeholders}): it does not start with `$<`, so the
substitution layer is never consulted. The brace pair is balanced
(delta = 0), so it does not interfere with the shell-block depth
tracker either. The step is emitted as an interactive shell command,
as all chore steps are.
