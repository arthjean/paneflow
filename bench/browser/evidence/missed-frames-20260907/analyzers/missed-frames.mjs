const integer = value => Number.isSafeInteger(value) && value >= 0;
const assert = (value, message) => { if (!value) throw new Error(message); };

export function countMissedFrames({ presentations, start_ns, end_ns, refresh_hz, uncertainty_ns }) {
  const result = { method: "native_refresh_slots_v1", status: "NOT_EVALUATED", start_ns, end_ns,
    interval: "[start,end)", evidence: { presentations, start_ns, end_ns, refresh_hz, uncertainty_ns }, errors: [] };
  try {
    assert(integer(start_ns) && integer(end_ns) && end_ns > start_ns, "invalid missed-frame window");
    assert(Number.isFinite(refresh_hz) && refresh_hz > 0, "missing actual refresh");
    assert(integer(uncertainty_ns) && uncertainty_ns <= 1_000_000, "invalid native timing uncertainty");
    assert(Array.isArray(presentations) && presentations.length >= 2, "native phase evidence is absent");
    const points = [...presentations].sort((a, b) => a.present_ns - b.present_ns);
    const period = 1e9 / refresh_hz;
    const anchor = points.findLast(point => point.present_ns <= start_ns);
    assert(anchor && points.some(point => point.present_ns >= end_ns), "native phase evidence must bracket the whole measurement window");
    const after = points.find(point => point.present_ns >= end_ns);
    const sequence = point => {
      assert(typeof point.output_sequence === "string" && /^(0|[1-9][0-9]*)$/.test(point.output_sequence), "invalid exact output sequence");
      const value = BigInt(point.output_sequence);
      assert(value <= 0xffffffffffffffffn, "output sequence exceeds uint64");
      return value;
    };
    const anchorSequence = sequence(anchor);
    const contents = new Set();
    const credited = new Set();
    let previous;
    let maxPhaseError = 0;
    for (const point of points) {
      assert(integer(point.present_ns) && integer(point.refresh_ns) && point.refresh_ns > 0, "invalid native presentation timing");
      assert(Math.abs(1e9 / point.refresh_ns - refresh_hz) <= 0.1, "native refresh differs from condition");
      const current = sequence(point);
      assert(previous === undefined || current > previous, "duplicate or reversed output sequence");
      previous = current;
      const offset = Number(current - anchorSequence);
      assert(Number.isSafeInteger(offset), "output sequence distance exceeds safe range");
      const expected = anchor.present_ns + offset * period;
      const error = Math.abs(point.present_ns - expected);
      if (point.present_ns >= anchor.present_ns && point.present_ns <= after.present_ns) {
        maxPhaseError = Math.max(maxPhaseError, error);
        assert(error + uncertainty_ns < Math.min(1_000_000, period / 2), "native refresh phase cannot identify bounded slots");
      }
      assert(point.fresh === true || point.fresh === false, "missing fresh draw classification");
      if (!point.fresh) continue;
      assert(typeof point.content_id === "string" && point.content_id.length > 0, "missing fresh draw identity");
      if (contents.has(point.content_id)) continue;
      contents.add(point.content_id);
      if (point.present_ns - uncertainty_ns >= start_ns && point.present_ns + uncertainty_ns < end_ns) credited.add(offset);
    }
    const margin = uncertainty_ns + maxPhaseError;
    const first = Math.ceil((start_ns - margin - anchor.present_ns) / period);
    const last = Math.ceil((end_ns + margin - anchor.present_ns) / period) - 1;
    const expected = last - first + 1;
    assert(Number.isSafeInteger(expected) && expected > 0, "no refresh opportunities");
    assert([...credited].every(slot => slot >= first && slot <= last), "presentation outside bounded refresh slots");
    const missed = expected - credited.size;
    return { ...result, status: missed * 100 < expected ? "SATISFIED" : "EXCEEDED",
      expected_slots: expected, presented_fresh_slots: credited.size, missed_slots: missed,
      missed_percent: 100 * missed / expected, limit_percent_exclusive: 1,
      boundary_policy: "conservative_phase_uncertainty_envelope", phase_margin_ns: margin,
      phase_anchor: { present_ns: anchor.present_ns, output_sequence: anchor.output_sequence },
      first_slot_offset: first, last_slot_offset: last, max_phase_error_ns: maxPhaseError };
  } catch (error) { return { ...result, errors: [error.message] }; }
}
