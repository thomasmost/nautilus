// Prints the window number of the first on-screen, ordinary (layer 0)
// window of the process whose id is the argument, or nothing. For
// `screencapture -l <number>` of a browser a scripted run launched: the
// owner's own Nautilus runs beside it, so the window is found by process,
// never by the application's name. Needs no Accessibility or Screen
// Recording permission: process ids, layers and window numbers are public.
//   swiftc -O ci/side-panel/window_id.swift -o <dir>/window-id
//   <dir>/window-id <pid>
import CoreGraphics
import Foundation

guard CommandLine.arguments.count > 1, let pid = Int(CommandLine.arguments[1]) else {
    exit(2)
}
let options: CGWindowListOption = [.optionOnScreenOnly, .excludeDesktopElements]
let list = CGWindowListCopyWindowInfo(options, kCGNullWindowID) as? [[String: Any]] ?? []
for window in list {
    if (window[kCGWindowOwnerPID as String] as? Int) == pid,
       (window[kCGWindowLayer as String] as? Int) == 0,
       let number = window[kCGWindowNumber as String] as? Int {
        print(number)
        break
    }
}
