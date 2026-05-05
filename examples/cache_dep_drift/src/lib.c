/* The dep-output value the consumer compiles against.
   Mutated by verify.sh between runs to prove dep-output drift
   propagates into the consumer's cache key. */
int lib_value(void) { return 1; }
