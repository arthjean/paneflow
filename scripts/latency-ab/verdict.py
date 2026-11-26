import json
import random
import re
import statistics
import sys
from pathlib import Path

MIN_SAMPLES = 300
BOOTSTRAP_RESAMPLES = 2000


def percentile(values, q):
    ordered = sorted(values)
    return ordered[min(len(ordered) - 1, int(q * len(ordered)))]


def frames(title):
    match = re.search(r"frames=(\d+)", title or "")
    return int(match.group(1)) if match else None


def load_run(path, arm):
    report = json.loads(path.read_text())
    samples = report["samples_ms"]
    problems = []
    if report.get("error"):
        problems.append(report["error"])
    if len(samples) < MIN_SAMPLES:
        problems.append(f"{len(samples)} samples, fewer than {MIN_SAMPLES}")
    if report["arm"] != arm:
        problems.append(f"ran arm {report['arm']}, scheduled {arm}")
    played = frames(report.get("tab_title"))
    if arm in ("control", "cpu") and not played:
        problems.append(f"the video did not play (title {report.get('tab_title')!r})")
    if arm == "cpu" and not report.get("frames_copied"):
        problems.append("the window received no frame")
    return {
        "file": path.name,
        "arm": arm,
        "n": len(samples),
        "median_ms": statistics.median(samples) if samples else None,
        "p99_ms": percentile(samples, 0.99) if samples else None,
        "video_frames": played,
        "frames_copied": report.get("frames_copied"),
        "root_renders_per_s": (
            report["root_renders"] / report["recording_seconds"]
            if report.get("recording_seconds")
            else None
        ),
        "window": [report.get("window_width"), report.get("window_height"), report.get("scale_factor")],
        "samples": samples,
        "problems": problems,
    }


def bootstrap_median_difference(treated, control, seed):
    rng = random.Random(seed)
    differences = []
    for _ in range(BOOTSTRAP_RESAMPLES):
        a = [rng.choice(treated) for _ in treated]
        b = [rng.choice(control) for _ in control]
        differences.append(statistics.median(a) - statistics.median(b))
    differences.sort()
    low = differences[int(0.025 * len(differences))]
    high = differences[int(0.975 * len(differences)) - 1]
    return statistics.median(treated) - statistics.median(control), low, high


def main():
    out = Path(sys.argv[1])
    rule = json.loads((out / "rule.json").read_text())
    runs = [load_run(out / entry["file"], entry["arm"]) for entry in rule["schedule"]]
    outlier_factor = rule.get("outlier_factor")
    for arm in ("control", "cpu") if outlier_factor else ():
        medians = [run["median_ms"] for run in runs if run["arm"] == arm and run["median_ms"] is not None]
        if not medians:
            continue
        center = statistics.median(medians)
        for run in runs:
            if run["arm"] == arm and run["median_ms"] is not None and run["median_ms"] > outlier_factor * center:
                run["problems"].append(
                    f"median {run['median_ms']:.3f} ms exceeds {outlier_factor} times its arm's median of medians ({center:.3f} ms)"
                )
    invalid = [run for run in runs if run["problems"]]
    control = [run for run in runs if run["arm"] == "control"]
    cpu = [run for run in runs if run["arm"] == "cpu"]
    verdict = {"rule": rule["rule"], "registered_at": rule["registered_at"], "runs": []}
    for run in runs:
        verdict["runs"].append({key: value for key, value in run.items() if key != "samples"})
    if invalid:
        verdict["decision"] = "invalid"
        verdict["reason"] = "; ".join(f"{run['file']}: {', '.join(run['problems'])}" for run in invalid)
    else:
        largest_median = max(run["median_ms"] for run in control)
        largest_p99 = max(run["p99_ms"] for run in control)
        over_median = [run["file"] for run in cpu if run["median_ms"] > largest_median]
        over_p99 = [run["file"] for run in cpu if run["p99_ms"] > largest_p99]
        verdict["control_largest_median_ms"] = largest_median
        verdict["control_largest_p99_ms"] = largest_p99
        verdict["cpu_over_median"] = over_median
        verdict["cpu_over_p99"] = over_p99
        verdict["decision"] = "regression" if over_median or over_p99 else "no_regression"
        pooled_cpu = [value for run in cpu for value in run["samples"]]
        pooled_control = [value for run in control for value in run["samples"]]
        estimate, low, high = bootstrap_median_difference(pooled_cpu, pooled_control, rule["seed"])
        verdict["informational_median_difference_ms"] = {
            "estimate": estimate,
            "ci95": [low, high],
            "resamples": BOOTSTRAP_RESAMPLES,
        }
    (out / "verdict.json").write_text(json.dumps(verdict, indent=2) + "\n")
    print(f"decision: {verdict['decision']}")
    for run in verdict["runs"]:
        print(
            f"{run['file']}: n={run['n']} median={run['median_ms']} p99={run['p99_ms']} "
            f"video_frames={run['video_frames']} frames_copied={run['frames_copied']} "
            f"root_renders_per_s={run['root_renders_per_s']}"
            + (f" PROBLEMS={run['problems']}" if run["problems"] else "")
        )
    if "reason" in verdict:
        print(verdict["reason"])
    if "control_largest_median_ms" in verdict:
        diff = verdict["informational_median_difference_ms"]
        print(
            f"control largest median {verdict['control_largest_median_ms']:.3f} ms, "
            f"largest p99 {verdict['control_largest_p99_ms']:.3f} ms; "
            f"cpu over median: {verdict['cpu_over_median']}, over p99: {verdict['cpu_over_p99']}; "
            f"pooled median difference {diff['estimate']:.3f} ms, 95% CI [{diff['ci95'][0]:.3f}, {diff['ci95'][1]:.3f}]"
        )
    return 0 if verdict["decision"] != "invalid" else 3


if __name__ == "__main__":
    sys.exit(main())
