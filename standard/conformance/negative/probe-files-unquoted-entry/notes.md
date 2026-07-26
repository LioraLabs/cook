Pins §22.5.2 (CS-0148): an unquoted entry in a `files` glob list is rejected.
The quote is load-bearing per §22.5.10's two-category discriminator — a
double-quoted operand is a literal filesystem string, a bare operand is a
namespace identifier — so admitting `src/*.ts` bare would make the `files`
list the one place where that law does not hold.
