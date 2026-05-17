Asserts the toolchain override path: `cook_cc.toolchain({compiler="g++"})`
selects probe key `cc:compiler:g++` (vs the default `cc:compiler:auto`).
Per Standard §28.3.10, the override is encoded into the probe key suffix.
