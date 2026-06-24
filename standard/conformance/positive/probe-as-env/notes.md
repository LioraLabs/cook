Pins §22.5.2: `produce as env { SDKROOT, CC }` parses the brace content as a
LIST of bare env-var names (not a shell body), yielding `ProbeProduce::Env`.
COOK-164.
