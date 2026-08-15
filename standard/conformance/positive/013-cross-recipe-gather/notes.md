Pins cross-recipe accessor iteration: `headers` gathers and copies each header,
then `list_headers` fans out over those declared outputs through `$<headers.stem>`.

The parser dump pins the shape; the codegen corpus pins the cross-recipe edge.
