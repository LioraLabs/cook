Pins the § 5.4 accessor-driven iteration. `compile_protos`'s output pattern `"build/obj/{protos.stem}.pb.cc"` names `protos` as the iteration driver, so at register time `compile_protos` fans out to one work unit per item in `protos`'s output list (here, the two `.proto` ingredients).

The parser preserves the placeholder tokens verbatim; the iteration-driver semantics belong to the codegen / runtime layers.
