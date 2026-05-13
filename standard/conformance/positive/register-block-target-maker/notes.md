Pins CS-0072 + cook_cc 0.4.0: a `register` block calling `cook_cc.bin(…)` is one of two canonical idioms under Cook Standard v0.9 (the other being a top-level `module_call` after a `config` block). The target-maker creates its own recipe header internally (`cook.recipe("game", …)`); the Cookfile author no longer writes `recipe game`.

Execute-mode verification (actually invoking `cook` and asserting on the compiled binary) is deferred to SHI-210; this fixture is parse-only.
