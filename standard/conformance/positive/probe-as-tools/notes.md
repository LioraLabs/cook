Pins §22.5.2: `produce as tools { cc, ld }` parses the brace content as a LIST
of bare tool names (not a shell body), yielding `ProbeProduce::Tools`. COOK-164.
