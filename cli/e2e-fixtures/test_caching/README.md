# `examples/test_caching/`

Fixture pinning the test-result caching contract. Walkthrough verifies
that passing tests cache, second runs hit cache, source-file touches
bust the affected test only, and `--rerun` busts everything.
