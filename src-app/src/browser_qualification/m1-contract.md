# M1 integrated capture contract

On Linux, the normal application opens an additional CEF-to-GPUI window when
`PANEFLOW_M1_BROWSER_URL` is set. `PANEFLOW_M1_LOG` and
`PANEFLOW_M1_BROWSER_LOG` must name separate absolute, new JSONL files. The first
contains terminal and Browser measurement events; the second contains the full
prototype lifecycle log. The usual verified CEF runtime and host environment
variables still apply.

The capture runner creates the same four terminals with `workspace.up` and the
same replay used for A. The Browser window leaves the normal terminal layout
unchanged. Its requested client size is 1920 by 1080 physical pixels, converted
using the window scale. `PANEFLOW_M1_BROWSER_X_PX` and
`PANEFLOW_M1_BROWSER_Y_PX` optionally request its position, using the initial
terminal window scale. Wayland compositors control final placement. The runner
must verify both windows remain visible and that the actual terminal and
Browser viewport dimensions satisfy the capture conditions. A position request
is not proof of placement or visibility.

The Browser window has application ID `paneflow-m1-browser` and does not request
keyboard focus. The runner terminates the normal app through its existing
shutdown path after replay. The prototype startup, host failure, and 240-second
run guards remain active; its normal successful hold does not quit the combined
application.

## Event identity

Every Browser frame event below carries these fields:

```json
{
  "document": {
    "owner": {"workspace": "prototype-workspace", "session": "prototype-session"},
    "browser": "prototype",
    "generation": 1
  },
  "pool_generation": 1,
  "buffer": 0,
  "frame_sequence": 42,
  "callback_ns": 1000000000,
  "ready_ns": 1000100000,
  "capture_timestamp_us": 999900,
  "intake_ns": 1000200000
}
```

The strings and numbers above are illustrative. Correlate using the complete
`document` plus `pool_generation`, `buffer`, and `frame_sequence`. Do not join
only by `frame_sequence`: session restarts and pool replacement exist.
`callback_ns`, `ready_ns`, and `intake_ns` use the host/application monotonic
clock contract. `capture_timestamp_us` preserves the Chromium-provided value
without remapping. Its source clock and calibration must be verified from the
host evidence before calculating a capture-to-presentation interval. Zero or
unverified capture timestamps cannot qualify that interval.

| Event | Additional fields | Boundary |
| --- | --- | --- |
| `browser_intake` | `at_ns`, `outstanding`, `pools` | Consumer accepted a frame; not a paint or presentation |
| `browser_paint` | `at_ns`, `feedback_id` | External surface primitive inserted during GPUI paint and presentation feedback requested for that window |
| `browser_presented` | Fields below | Compositor `wp_presentation_feedback.presented` for the associated surface commit |
| `browser_discarded` | `at_ns`, `feedback_id` | Compositor discarded the associated commit |
| `browser_paint_unobserved` | `at_ns`, `reason` | Primitive painted before the observer was ready; no presentation inference is permitted |

`browser_presented` additionally contains:

```json
{
  "feedback_id": 17,
  "present_ns": 1016666667,
  "native_present_ns": 1016666667,
  "presentation_callback_ns": 1016800000,
  "clock_id": 1,
  "refresh_ns": 16666667,
  "sequence": 200,
  "flags": "Vsync | HwCompletion | HwClock",
  "calibration": {
    "source_ns": 1016800050,
    "mapped_ns": 1016800050,
    "max_error_ns": 50
  }
}
```

`sequence` is the compositor sequence. `frame_sequence` is the browser frame
sequence. `presentation_callback_ns` records delivery of compositor feedback;
`callback_ns` continues to identify the CEF callback. `present_ns` maps the
native presentation clock to `CLOCK_MONOTONIC` using the midpoint and error
bound recorded in `calibration`. Reject measurements whose calibration exceeds
the budget. This is a compositor presentation observation, not photon timing.

Repeated paints of the same browser frame create one feedback request. A frame
superseded before paint can have an intake event without a paint event. Counting
all intakes as presentations is invalid. Count `browser_intake`, `browser_paint`,
`browser_presented`, and `browser_discarded` separately, correlate their identities,
and account for unobserved frames and incomplete feedback near capture edges.
The prototype lifecycle summary retains legacy `presented_frames` compatibility,
but explicitly identifies that counter as accepted intake and also exposes it
as `intake_frames`. It is not a measurement of compositor presentation.

`browser_viewport` reports `width_px`, `height_px`, `scale`, and `at_ns` from the
live GPUI Browser window. Terminal `viewport`, `geometry`, `input`, `paint`,
`presented`, `discarded`, and `cpu` events retain their existing schema. Terminal
and Browser observers use separate native surfaces and feedback sequences.
A `fatal` event or log queue overflow invalidates the capture. Window geometry,
visibility, exact replay equivalence, native sandbox state, buffer ownership,
copy counts, release fences, hardware refresh rate, and the CEF minimal B
comparison still require the capture runner and host evidence.
