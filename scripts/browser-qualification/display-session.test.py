import copy
import importlib.util
import json
from pathlib import Path
import signal
import tempfile
import types
import unittest
from unittest.mock import patch


SPEC = importlib.util.spec_from_file_location("display_session", Path(__file__).with_name("display-session.py"))
SESSION = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(SESSION)
OUTPUTS = ["DP-3", "DP-4"]


def initial_state():
    monitors = []
    logical = []
    for index, connector in enumerate(OUTPUTS):
        spec = [connector, "vendor", "model", str(index)]
        modes = [
            {"id": "original", "width": 2560, "height": 1440, "refresh_hz": 144.0,
             "scales": [1.0], "properties": {"is-current": True}},
            {"id": "60", "width": 1920, "height": 1080, "refresh_hz": 60.0,
             "scales": [1.0], "properties": {}},
            {"id": "120", "width": 1920, "height": 1080, "refresh_hz": SESSION.ACTUAL_HZ[120],
             "scales": [1.0], "properties": {}},
            {"id": "120+vrr", "width": 1920, "height": 1080, "refresh_hz": SESSION.ACTUAL_HZ[120],
             "scales": [1.0], "properties": {"refresh-rate-mode": "variable"}},
        ]
        monitors.append({"spec": spec, "modes": modes, "properties": {"color-mode": 0, "rgb-range": 1}})
        logical.append({"x": index * 2560, "y": 0, "scale": 1.0, "transform": 0,
                        "primary": index == 0, "monitors": [spec], "properties": {}})
    return {"serial": 1, "monitors": monitors, "logical_monitors": logical,
            "properties": {"layout-mode": 1}}


class FakeDisplay:
    def __init__(self):
        self.state = initial_state()
        self.applications = []
        self.after_apply = None
        self.events = []

    def begin_observation(self):
        self.events.clear()

    def observed_changes(self):
        return list(self.events)

    def read(self):
        return copy.deepcopy(self.state)

    def apply(self, plan, connectors):
        self.applications.append(copy.deepcopy(plan))
        self.state["serial"] += 1
        self.state["logical_monitors"] = []
        self.state["properties"]["layout-mode"] = plan["layout_mode"]
        for output in plan["outputs"]:
            monitor = next(item for item in self.state["monitors"] if item["spec"][0] == output["connector"])
            for mode in monitor["modes"]:
                mode["properties"]["is-current"] = mode["id"] == output["mode"]
            self.state["logical_monitors"].append({
                **{key: output[key] for key in ("x", "y", "scale", "transform", "primary")},
                "monitors": [monitor["spec"]], "properties": {},
            })
        if self.after_apply:
            self.after_apply(self)


class FakeChild:
    pid = 54321
    returncode = 0

    def poll(self):
        return self.returncode


class DisplaySessionTests(unittest.TestCase):
    def execute(self, display, child_effect=None):
        child = FakeChild()
        with tempfile.TemporaryDirectory() as directory:
            with patch.object(SESSION.subprocess, "Popen", side_effect=child_effect, return_value=child) as spawn:
                with patch.object(SESSION, "terminate_child"):
                    code = SESSION.supervise(display, Path(directory), OUTPUTS, 120, "DP-4", ["capture"], None)
            receipts = {path.name: json.loads(path.read_text()) for path in Path(directory).glob("*.json")}
        return code, receipts, spawn

    def test_actual_refresh_and_nonoverlapping_primary_selection(self):
        plan = SESSION.measurement_plan(initial_state(), OUTPUTS, 120, "DP-4")
        self.assertEqual([item["mode"] for item in plan["outputs"]], ["120", "120"])
        self.assertEqual([item["x"] for item in plan["outputs"]], [0, 1920])
        self.assertEqual([item["primary"] for item in plan["outputs"]], [False, True])
        self.assertEqual(plan["outputs"][0]["refresh_actual_hz"], 119.8787841796875)

    def test_rejects_mirroring_overlap_and_unknown_output(self):
        state = initial_state()
        state["logical_monitors"][0]["monitors"].append(state["monitors"][1]["spec"])
        with self.assertRaises(ValueError):
            SESSION.current_plan(state, OUTPUTS)
        state = initial_state()
        state["logical_monitors"][1]["x"] = 100
        with self.assertRaises(ValueError):
            SESSION.current_plan(state, OUTPUTS)
        with self.assertRaises(ValueError):
            SESSION.current_plan(initial_state(), ["DP-3", "DP-99"])

    def test_success_restores_original_mode_layout_scale_rotation_and_primary(self):
        display = FakeDisplay()
        logical = display.state["logical_monitors"]
        logical[0].update(scale=2.0, transform=1)
        logical[1].update(x=720, y=200)
        original = SESSION.current_plan(display.read(), OUTPUTS)
        code, receipts, spawn = self.execute(display)
        self.assertEqual(code, 0)
        self.assertTrue(SESSION.same_plan(SESSION.current_plan(display.read(), OUTPUTS), original))
        self.assertEqual(receipts["restored.json"]["status"], "RESTORED")
        self.assertLess(receipts["applied.json"]["monotonic_ns"], receipts["condition-complete.json"]["monotonic_ns"])
        self.assertLess(receipts["condition-complete.json"]["monotonic_ns"], receipts["restored.json"]["monotonic_ns"])
        self.assertEqual(receipts["applied.json"]["verified_plan"], receipts["condition-complete.json"]["verified_plan"])
        spawn.assert_called_once_with(["capture"], start_new_session=True)

    def test_ambiguous_apply_failure_restores_without_launching_child(self):
        display = FakeDisplay()
        def fail_first_apply(instance):
            if len(instance.applications) == 1:
                raise RuntimeError("reply lost after applying")
        display.after_apply = fail_first_apply
        code, receipts, spawn = self.execute(display)
        self.assertEqual(code, 1)
        self.assertEqual(len(display.applications), 2)
        self.assertIn("restored.json", receipts)
        spawn.assert_not_called()

    def test_condition_change_at_child_exit_fails_and_restores(self):
        display = FakeDisplay()
        def change_then_exit(*_args, **_kwargs):
            display.state["logical_monitors"][1]["x"] += 100
            return FakeChild()
        code, receipts, _spawn = self.execute(display, change_then_exit)
        self.assertEqual(code, 1)
        self.assertIn("condition-changed.json", receipts)
        self.assertIn("restored.json", receipts)

    def test_signal_after_apply_prevents_launch_and_restores(self):
        display = FakeDisplay()
        def interrupt_first_apply(instance):
            if len(instance.applications) == 1:
                signal.getsignal(signal.SIGTERM)(signal.SIGTERM, None)
        display.after_apply = interrupt_first_apply
        code, receipts, spawn = self.execute(display)
        self.assertEqual(code, 143)
        self.assertIn("restored.json", receipts)
        spawn.assert_not_called()

    def test_transient_change_returning_to_identical_plan_is_rejected(self):
        for detection in ("signal", "serial"):
            with self.subTest(detection=detection):
                display = FakeDisplay()
                def change_then_return(*_args, **_kwargs):
                    if detection == "signal":
                        display.events.append({"signal": "MonitorsChanged", "received_monotonic_ns": 100})
                    else:
                        display.state["serial"] += 2
                    return FakeChild()
                code, receipts, _spawn = self.execute(display, change_then_return)
                self.assertEqual(code, 1)
                self.assertIn("condition-changed.json", receipts)
                self.assertNotIn("condition-complete.json", receipts)
                self.assertIn("restored.json", receipts)

    def test_restore_failure_is_reported_as_failure(self):
        display = FakeDisplay()
        def fail_restore(instance):
            if len(instance.applications) == 2:
                raise RuntimeError("restore refused")
        display.after_apply = fail_restore
        code, receipts, _spawn = self.execute(display)
        self.assertEqual(code, 1)
        self.assertEqual(receipts["restore-failed.json"]["status"], "RESTORE_FAILED")

    def test_apply_retries_fresh_serial_and_can_restore_a_mirrored_current_layout(self):
        display = SESSION.DisplayConfig.__new__(SESSION.DisplayConfig)
        state = initial_state()
        plan = SESSION.current_plan(state, OUTPUTS)
        state["logical_monitors"] = []
        reads = []
        calls = []
        def read():
            state["serial"] += 1
            reads.append(state["serial"])
            return copy.deepcopy(state)
        def call(_method, parameters, *_args):
            calls.append(parameters)
            if len(calls) == 2:
                raise RuntimeError("The requested configuration is based on stale information")
        display.read = read
        display.GLib = types.SimpleNamespace(Variant=lambda _kind, value: value, Error=RuntimeError)
        display.Gio = types.SimpleNamespace(DBusCallFlags=types.SimpleNamespace(NONE=0))
        display.proxy = types.SimpleNamespace(call_sync=call)
        display.apply(plan, OUTPUTS)
        self.assertEqual([item[1] for item in calls], [0, 1, 0, 1])
        self.assertEqual([item[0] for item in calls], [2, 2, 4, 4])
        self.assertEqual(reads, [2, 3, 4])


if __name__ == "__main__":
    unittest.main()
