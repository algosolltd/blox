# Modular platform cutover

The clean-break restructuring is implemented as five independently buildable
repositories:

- `blox`: `blox-core` matching plus the `blox-server` JSON Lines TCP adapter.
- `algosim`: deterministic, multi-instrument synthetic provider and venue.
- `algovisual`: backend-free TypeScript trading terminal.
- `algolib`: canonical model, 22-family order catalog and provider adapters.
- `algorunner`: deterministic sizing boundary, order lifecycle and composite
  OCO/OTO/bracket coordination.

Only Market, Limit and Cancel enter `blox-core`. Conditional, scheduled and
composite semantics are evaluated by `algorunner`; market-data normalization
is performed by provider adapters in `algolib`.

Verification includes unit tests, strict clippy/TypeScript checks, a real TCP
server e2e, and the cross-repository algosim → normalization → algorunner →
matching → fill lifecycle e2e.
