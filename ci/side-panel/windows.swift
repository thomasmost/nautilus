// Prints "<epoch ms> <count>" each time the number of on-screen, ordinary
// (layer 0) windows of the named application changes, for the given number of
// seconds. With a process id as the third argument, only the windows of that
// process are counted (the owner's own Nautilus runs beside a test browser, and
// its windows must not count). Needs no Accessibility or Screen Recording
// permission: owner names, process ids and layers are public.
//   swiftc -O ci/side-panel/windows.swift -o <dir>/p5-windows
//   <dir>/p5-windows Nautilus 120 [pid]
import CoreGraphics
import Foundation

let owner = CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : "Nautilus"
let seconds = CommandLine.arguments.count > 2 ? (Double(CommandLine.arguments[2]) ?? 60) : 60
let pid: Int? = CommandLine.arguments.count > 3 ? Int(CommandLine.arguments[3]) : nil
let end = Date().addingTimeInterval(seconds)
var last = -1
setvbuf(stdout, nil, _IOLBF, 0)
while Date() < end {
    let options: CGWindowListOption = [.optionOnScreenOnly, .excludeDesktopElements]
    let list = CGWindowListCopyWindowInfo(options, kCGNullWindowID) as? [[String: Any]] ?? []
    let count = list.filter { window in
        let mine: Bool
        if let pid = pid {
            mine = (window[kCGWindowOwnerPID as String] as? Int) == pid
        } else {
            mine = (window[kCGWindowOwnerName as String] as? String) == owner
        }
        return mine && (window[kCGWindowLayer as String] as? Int) == 0
    }.count
    if count != last {
        print("\(Int(Date().timeIntervalSince1970 * 1000)) \(count)")
        last = count
    }
    usleep(20_000)
}
