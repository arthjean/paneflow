# Browser missed-frame evidence

The B and C analyzers use `scripts/browser-qualification/missed-frames.mjs`.
This is an NFR-03 frame cadence measurement, separate from latency percentiles,
content pixel verification, sandbox qualification and the complete M1 verdict.

The measurement window is fixed and half open: `[origin + 10 s, origin + 70 s)`.
Its denominator is the number of native refresh slots in that window, not the
number of captures, deliveries, intakes or latency samples. Silence and losses
at both ends therefore cannot shorten the denominator. Every repetition of B
and C must satisfy `missed_slots * 100 < expected_slots`. Exactly 1% fails.

The native output sequence and presentation timestamp immediately before the
window anchor the refresh phase. A native presentation at or after its end is
also required. Within those brackets, the measured phase residual plus clock
uncertainty must remain below both 1 ms and half a refresh period. Initial
window setup outside these brackets is not a steady-state phase observation.
Sequence reversal and duplication still invalidate the evidence throughout.

The denominator includes slots in the conservative envelope formed by the
observed phase residual and clock uncertainty at the boundaries. A presentation
receives credit only when its clock uncertainty interval lies wholly inside
the measurement window. This can conservatively count an uncertain edge as
missed. The report retains the anchor, residual, margin and slot offsets.
Absent brackets or ambiguous phase produce `NOT_EVALUATED`, never a pass.

B credits uniquely correlated DrawAndSwap/native presentations. C credits fresh
CopyRequested draws joined through delivery, intake, paint and native feedback.
A draw can receive credit only once. Resurrected content receives no fresh-frame
credit. A draw beginning before measurement and presented during measurement
can occupy a slot while being excluded from the latency samples. A draw
presented after the end cannot occupy a measured slot.

Raw pipeline losses are retained separately and are not added to slot losses,
which would double count some failures. C reports intake cohorts and capture
origin cohorts: before observation, warmup, measurement and drain. B retains
native discard identities and the enclosing native presentation timestamps;
an ambiguous boundary remains explicitly ambiguous. Missing or contradictory
feedback and unproven native connections remain parser failures.

Fullscreen output binding covers the whole observation, including warmup. An
explicitly discarded Browser paint can terminate its feedback identity without
counting as presented. Missing feedback, contradictory terminal events,
identity changes and fatal rendering errors still reject the binding. The
binding result does not claim a missed-frame budget.

Collectors retain normalized native slot evidence and archive their analyzer
sources. B finalization reconstructs evidence from the native raw archives.
Inspection and comparison recompute the slot verdict instead of trusting a
saved status. Synthetic tests and short diagnostic captures cannot certify M1.
