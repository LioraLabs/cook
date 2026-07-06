Pins §9.2.3.2 cc.lib + §9.2.3.1 cc.bin with `links` cross-reference;
exercises the parse shape of multi-recipe dependency declarations
and the `export_includes` option. Parse-only per Task 20's harness scope.
Runtime is covered by examples/modules/cpp-project (which uses the same pattern
end-to-end via cook_cc 0.1.2-1's lib_path + export_includes fixes).
