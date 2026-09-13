import AppKit
import CoreGraphics
import Foundation

struct AppSpec {
    let name: String
    let path: String
    let version: String
    var environment: [String: String] = [:]

    var executableURL: URL {
        let url = URL(fileURLWithPath: path)
        if url.pathExtension == "app", let bundle = Bundle(url: url), let executable = bundle.executableURL {
            return executable
        }
        return url
    }
}

struct Options {
    var apps: [AppSpec] = []
    var runs = 10
    var warmups = 1
    var timeoutSeconds = 30.0
    var paintTimeoutSeconds = 5.0
    var cooldownSeconds = 1.0
    var paint = true
    var output = ""
    var meta: [String: String] = [:]
}

struct Sample {
    let windowOnscreenMs: Double
    let firstPaintMs: Double?
}

func fail(_ message: String) -> Never {
    fputs("bench-startup-compare: \(message)\n", stderr)
    exit(1)
}

func warn(_ message: String) {
    fputs("PANEFLOW_BENCH_WARNING \(message)\n", stderr)
}

func require<T>(_ value: T?, _ message: String) -> T {
    guard let value else { fail(message) }
    return value
}

func parseOptions() -> Options {
    var options = Options()
    var arguments = Array(CommandLine.arguments.dropFirst())
    func take(_ flag: String) -> String {
        guard !arguments.isEmpty else { fail("\(flag) needs a value") }
        return arguments.removeFirst()
    }
    while !arguments.isEmpty {
        let flag = arguments.removeFirst()
        switch flag {
        case "--app":
            let name = take(flag)
            let path = take(flag)
            let version = take(flag)
            options.apps.append(AppSpec(name: name, path: path, version: version))
        case "--env":
            let name = take(flag)
            let assignment = take(flag)
            guard let separator = assignment.firstIndex(of: "=") else { fail("--env expects KEY=VALUE") }
            guard let index = options.apps.firstIndex(where: { $0.name == name }) else {
                fail("--env names an unknown app \(name); declare --app first")
            }
            options.apps[index].environment[String(assignment[..<separator])] =
                String(assignment[assignment.index(after: separator)...])
        case "--runs":
            options.runs = require(Int(take(flag)), "--runs expects an integer")
        case "--warmups":
            options.warmups = require(Int(take(flag)), "--warmups expects an integer")
        case "--timeout":
            options.timeoutSeconds = require(Double(take(flag)), "--timeout expects seconds")
        case "--paint-timeout":
            options.paintTimeoutSeconds = require(Double(take(flag)), "--paint-timeout expects seconds")
        case "--cooldown":
            options.cooldownSeconds = require(Double(take(flag)), "--cooldown expects seconds")
        case "--no-paint":
            options.paint = false
        case "--out":
            options.output = take(flag)
        case "--meta":
            let assignment = take(flag)
            guard let separator = assignment.firstIndex(of: "=") else { fail("--meta expects KEY=VALUE") }
            options.meta[String(assignment[..<separator])] = String(assignment[assignment.index(after: separator)...])
        default:
            fail("unknown argument \(flag)")
        }
    }
    guard options.apps.count >= 2 else { fail("declare at least two --app NAME PATH VERSION entries") }
    guard !options.output.isEmpty else { fail("--out is required") }
    guard options.runs > 0 else { fail("--runs must be positive") }
    for app in options.apps where !FileManager.default.isExecutableFile(atPath: app.executableURL.path) {
        fail("\(app.name): no executable at \(app.executableURL.path)")
    }
    return options
}

func monotonicMs() -> Double {
    Double(DispatchTime.now().uptimeNanoseconds) / 1_000_000
}

func onscreenWindows(of pid: pid_t) -> [[String: Any]] {
    guard let windows = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID)
        as? [[String: Any]]
    else {
        return []
    }
    return windows.filter { info in
        guard (info[kCGWindowOwnerPID as String] as? NSNumber)?.int32Value == pid else { return false }
        guard ((info[kCGWindowLayer as String] as? NSNumber)?.intValue ?? 0) == 0 else { return false }
        guard ((info[kCGWindowAlpha as String] as? NSNumber)?.doubleValue ?? 1) > 0 else { return false }
        guard let boundsDictionary = info[kCGWindowBounds as String] as? NSDictionary else { return true }
        var bounds = CGRect.zero
        guard CGRectMakeWithDictionaryRepresentation(boundsDictionary as CFDictionary, &bounds) else { return true }
        return bounds.width > 1 && bounds.height > 1
    }
}

func windowHasPaintedContent(_ windowID: CGWindowID) -> Bool {
    guard let image = CGWindowListCreateImage(
        .null, .optionIncludingWindow, windowID, [.boundsIgnoreFraming, .nominalResolution]
    ), image.width > 8, image.height > 8,
        let data = image.dataProvider?.data, let bytes = CFDataGetBytePtr(data)
    else {
        return false
    }
    let bytesPerPixel = image.bitsPerPixel / 8
    guard bytesPerPixel >= 3 else { return false }
    let stepX = max(1, image.width / 48)
    let stepY = max(1, image.height / 48)
    var reference: (UInt8, UInt8, UInt8)?
    var y = 0
    while y < image.height {
        var x = 0
        while x < image.width {
            let offset = y * image.bytesPerRow + x * bytesPerPixel
            let pixel = (bytes[offset], bytes[offset + 1], bytes[offset + 2])
            if let reference {
                if pixel != reference { return true }
            } else {
                reference = pixel
            }
            x += stepX
        }
        y += stepY
    }
    return false
}

func launch(_ app: AppSpec, options: Options, measurePaint: Bool) -> Sample {
    let process = Process()
    process.executableURL = app.executableURL
    var environment = ProcessInfo.processInfo.environment
    environment.removeValue(forKey: "RUST_LOG")
    environment.merge(app.environment) { _, override in override }
    process.environment = environment
    process.standardInput = FileHandle.nullDevice
    process.standardOutput = FileHandle.nullDevice
    process.standardError = FileHandle.nullDevice

    let started = monotonicMs()
    do {
        try process.run()
    } catch {
        fail("\(app.name): launch failed: \(error)")
    }
    let pid = process.processIdentifier
    func stop() {
        kill(pid, SIGKILL)
        process.waitUntilExit()
    }
    func abandon(_ message: String) -> Never {
        stop()
        fail("\(app.name): \(message)")
    }

    var windowID: CGWindowID?
    var windowOnscreenMs: Double?
    while monotonicMs() - started < options.timeoutSeconds * 1000 {
        if !process.isRunning {
            abandon("exited with status \(process.terminationStatus) before showing a window")
        }
        if let window = onscreenWindows(of: pid).first {
            windowOnscreenMs = monotonicMs() - started
            windowID = (window[kCGWindowNumber as String] as? NSNumber).map { CGWindowID($0.uint32Value) }
            break
        }
        usleep(1000)
    }
    guard let windowOnscreenMs, let windowID else {
        abandon("no on-screen window within \(options.timeoutSeconds)s")
    }

    var firstPaintMs: Double?
    if measurePaint {
        let paintDeadline = monotonicMs() + options.paintTimeoutSeconds * 1000
        while monotonicMs() < paintDeadline {
            if windowHasPaintedContent(windowID) {
                firstPaintMs = monotonicMs() - started
                break
            }
            usleep(1000)
        }
    }
    stop()
    return Sample(windowOnscreenMs: windowOnscreenMs, firstPaintMs: firstPaintMs)
}

func sysctlString(_ name: String) -> String {
    var size = 0
    guard sysctlbyname(name, nil, &size, nil, 0) == 0, size > 0 else { return "unknown" }
    var buffer = [CChar](repeating: 0, count: size)
    guard sysctlbyname(name, &buffer, &size, nil, 0) == 0 else { return "unknown" }
    return String(cString: buffer)
}

func percentile(_ sorted: [Double], _ fraction: Double) -> Double {
    let index = min(sorted.count - 1, max(0, Int((Double(sorted.count - 1) * fraction).rounded())))
    return sorted[index]
}

func summary(_ samples: [Double]) -> [String: Any] {
    let sorted = samples.sorted()
    return [
        "samples": samples,
        "median": percentile(sorted, 0.5),
        "p95": percentile(sorted, 0.95),
        "min": sorted[0],
        "mean": samples.reduce(0, +) / Double(samples.count),
    ]
}

func format(_ value: Double?) -> String {
    guard let value else { return "n/a" }
    return String(format: "%.0f ms", value)
}

let options = parseOptions()
var measurePaint = options.paint
var samples: [String: [Sample]] = [:]

for app in options.apps {
    for _ in 0..<options.warmups {
        _ = launch(app, options: options, measurePaint: false)
        Thread.sleep(forTimeInterval: options.cooldownSeconds)
    }
}

for run in 0..<options.runs {
    for app in options.apps {
        let sample = launch(app, options: options, measurePaint: measurePaint)
        if measurePaint, sample.firstPaintMs == nil {
            warn("\(app.name): window content never left a uniform color within \(options.paintTimeoutSeconds)s; first paint needs Screen Recording permission for the probe, disabling it for the remaining launches")
            measurePaint = false
        }
        samples[app.name, default: []].append(sample)
        print("run \(run + 1)/\(options.runs) \(app.name): window \(format(sample.windowOnscreenMs)), paint \(format(sample.firstPaintMs))")
        Thread.sleep(forTimeInterval: options.cooldownSeconds)
    }
}

var appDocuments: [[String: Any]] = []
var medians: [String: Double] = [:]
for app in options.apps {
    let appSamples = samples[app.name] ?? []
    let window = summary(appSamples.map(\.windowOnscreenMs))
    medians[app.name] = window["median"] as? Double
    var document: [String: Any] = [
        "name": app.name,
        "path": app.path,
        "executable": app.executableURL.path,
        "version": app.version,
        "window_onscreen_ms": window,
    ]
    let paints = appSamples.compactMap(\.firstPaintMs)
    if paints.count == appSamples.count {
        document["first_paint_ms"] = summary(paints)
    } else {
        document["first_paint_ms"] = NSNull()
    }
    appDocuments.append(document)
}

let document: [String: Any] = [
    "schema": 1,
    "suite": "paneflow-startup-compare",
    "generated_unix": Int(Date().timeIntervalSince1970),
    "os": "macos",
    "os_version": ProcessInfo.processInfo.operatingSystemVersionString,
    "arch": sysctlString("hw.machine"),
    "cpu": sysctlString("machdep.cpu.brand_string"),
    "runs": options.runs,
    "warmups": options.warmups,
    "cooldown_seconds": options.cooldownSeconds,
    "metric_origin": "posix_spawn of the executable",
    "meta": options.meta,
    "apps": appDocuments,
]
do {
    let json = try JSONSerialization.data(withJSONObject: document, options: [.prettyPrinted, .sortedKeys])
    try json.write(to: URL(fileURLWithPath: options.output))
} catch {
    fail("could not write \(options.output): \(error)")
}

let reference = options.apps[0]
print("PANEFLOW_BENCH_TABLE_BEGIN")
print("| App | Version | Window on screen (median) | p95 | First paint (median) | p95 | vs \(reference.name) |")
print("|---|---|---|---|---|---|---|")
for (app, appDocument) in zip(options.apps, appDocuments) {
    let window = appDocument["window_onscreen_ms"] as? [String: Any] ?? [:]
    let paint = appDocument["first_paint_ms"] as? [String: Any]
    let ratio: String
    if let own = window["median"] as? Double, let base = medians[reference.name], base > 0 {
        ratio = app.name == reference.name ? "1.00x" : String(format: "%.2fx", own / base)
    } else {
        ratio = "n/a"
    }
    print(
        "| \(app.name) | \(app.version) | \(format(window["median"] as? Double)) | \(format(window["p95"] as? Double)) | \(format(paint?["median"] as? Double)) | \(format(paint?["p95"] as? Double)) | \(ratio) |"
    )
}
print("PANEFLOW_BENCH_TABLE_END")
