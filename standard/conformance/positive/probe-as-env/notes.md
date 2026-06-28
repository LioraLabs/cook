Pins §22.5.2: `envs { SDKROOT, CC }` parses the brace content as a
LIST of bare env-var names (not a shell body), yielding `ProbeProduce::Envs`.
COOK-164.
