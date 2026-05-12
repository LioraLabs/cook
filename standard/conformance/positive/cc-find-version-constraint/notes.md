# cc-find-version-constraint

Locks §9.2.3.8 `FindOpts.version` syntax: comma-separated AND, mixed operators.
Parse-only: this fixture does not verify behaviour, only that the Cookfile's
Lua step parses cleanly with the new opts form.
Runtime conformance for this surface is filed under SHI-210.
