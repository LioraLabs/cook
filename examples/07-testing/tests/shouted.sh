#!/bin/sh
# One of two scripts run one-to-one by `check`'s `test { ./$<in> }` line.
grep -q HELLO out/app.txt
