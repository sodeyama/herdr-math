import AppKit

// Native input helper for the tmath terminal renderer.
//
// It reports trackpad and cursor state to the supervising process over stdout
// using the `s`/`z`/`m`/`w`/`scale` line protocol. Ported from the
// terminal-browser pixel-core helper so scroll and pinch behavior matches the
// reference implementation.

let app = NSApplication.shared
app.setActivationPolicy(.prohibited)

let scale = NSScreen.main?.backingScaleFactor ?? 2.0
print("scale \(scale)")
fflush(stdout)

private func cursorPoint() -> CGPoint {
    CGEvent(source: nil)?.location ?? .zero
}

private let outputLock = NSLock()

private func emit(_ line: String) {
    outputLock.lock()
    print(line)
    fflush(stdout)
    outputLock.unlock()
}

private func windowUnderCursor(_ point: CGPoint) -> CGRect? {
    let options: CGWindowListOption = [.optionOnScreenOnly, .excludeDesktopElements]
    guard let list = CGWindowListCopyWindowInfo(options, kCGNullWindowID) as? [[String: Any]] else {
        return nil
    }
    for info in list {
        guard let layer = info[kCGWindowLayer as String] as? Int, layer == 0 else { continue }
        if let alpha = info[kCGWindowAlpha as String] as? Double, alpha < 0.05 { continue }
        guard let raw = info[kCGWindowBounds as String],
              let bounds = CGRect(dictionaryRepresentation: raw as! CFDictionary),
              bounds.width > 1, bounds.height > 1 else { continue }
        if bounds.contains(point) { return bounds }
    }
    return nil
}

private final class WindowProbe {
    private let lock = NSLock()
    private var lastPoint = CGPoint(x: CGFloat.infinity, y: CGFloat.infinity)
    private var lastRect: CGRect?
    private var lastProbe = 0.0

    func refresh(_ point: CGPoint, force: Bool) {
        lock.lock()
        let now = ProcessInfo.processInfo.systemUptime
        let moved = hypot(point.x - lastPoint.x, point.y - lastPoint.y) > 2
        guard force || (moved && now - lastProbe > 0.08) else {
            lock.unlock()
            return
        }
        lastPoint = point
        lastProbe = now
        let previous = lastRect
        let rect = windowUnderCursor(point)
        lastRect = rect
        let changed = previous.map { p in rect.map { !$0.equalTo(p) } ?? true } ?? (rect != nil)
        lock.unlock()
        guard changed else { return }
        if let rect {
            emit("w \(rect.origin.x) \(rect.origin.y) \(rect.width) \(rect.height)")
        } else {
            emit("w none")
        }
    }

    func invalidate() {
        lock.lock()
        lastPoint = CGPoint(x: CGFloat.infinity, y: CGFloat.infinity)
        lastRect = nil
        lock.unlock()
    }
}

private let windowProbe = WindowProbe()

private final class PositionStream {
    private let lock = NSLock()
    private var armedUntil = 0.0
    private var lastEmit = 0.0

    private static let keepalive = 8.0

    func setArmed(_ value: Bool) {
        lock.lock()
        armedUntil = value ? ProcessInfo.processInfo.systemUptime + Self.keepalive : 0
        lock.unlock()
        if value { windowProbe.invalidate() }
    }

    func tick() {
        lock.lock()
        let now = ProcessInfo.processInfo.systemUptime
        guard now < armedUntil, now - lastEmit > 1.0 / 90.0 else {
            lock.unlock()
            return
        }
        lastEmit = now
        lock.unlock()
        let point = cursorPoint()
        emit("m \(point.x) \(point.y)")
        windowProbe.refresh(point, force: false)
    }
}

private let positions = PositionStream()

NSEvent.addGlobalMonitorForEvents(matching: .scrollWheel) { event in
    let precise = event.hasPreciseScrollingDeltas ? 1 : 0
    let point = cursorPoint()
    windowProbe.refresh(point, force: event.phase.contains(.began))
    emit("s \(event.scrollingDeltaY) \(event.phase.rawValue) \(event.momentumPhase.rawValue) \(precise) \(event.scrollingDeltaX) \(point.x) \(point.y)")
}

NSEvent.addGlobalMonitorForEvents(matching: [.mouseMoved, .leftMouseDragged, .rightMouseDragged]) { _ in
    positions.tick()
}

NotificationCenter.default.addObserver(
    forName: NSApplication.didChangeScreenParametersNotification, object: nil, queue: .main
) { _ in
    windowProbe.invalidate()
    emit("scale \(NSScreen.main?.backingScaleFactor ?? 2.0)")
}

DispatchQueue.global().async {
    while let line = readLine() {
        let fields = line.split(separator: " ")
        if fields.count == 2, fields[0] == "positions" {
            positions.setArmed(fields[1] == "1")
        }
    }
    exit(0)
}

app.run()
