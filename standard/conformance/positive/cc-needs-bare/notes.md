Exercises `needs = {...}` with a library that has no pkg-config file
(libm — always present, no .pc). The bare-probe strategy is what resolves
it via `cc:linker-search-dirs`.
