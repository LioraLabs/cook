§22.5.9 non-array value. The `cards` probe resolves to a record, not an
array-shaped table, so it cannot drive an `ingredients <probe>` source. The
register pre-pass rejects it naming the probe key and the resolved shape.
