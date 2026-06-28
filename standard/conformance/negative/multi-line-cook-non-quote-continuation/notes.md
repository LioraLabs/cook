Pins the negative case for multi-line `cook` outputs: after collecting quoted
output patterns across continuation lines, a non-quote token that is not a body
opener (`{` / `>{`) is parsed as a trailing `cook_mods` token (COOK-171). An
unrecognised token there (`garbage_line_here`) is rejected as an unexpected
modifier. (Trailing modifiers follow the body; a token before the body opener is
not a valid modifier and not a body.)

Exercises App. A.4 (cook_step production, multi-line whitespace rule, cook_mods).
