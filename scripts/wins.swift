import CoreGraphics
import Foundation
let pid = Int(CommandLine.arguments[1])!
let list = CGWindowListCopyWindowInfo([.optionAll], kCGNullWindowID) as? [[String: Any]] ?? []
for w in list where (w[kCGWindowOwnerPID as String] as? Int) == pid {
  print(w[kCGWindowNumber as String] ?? "-", w[kCGWindowLayer as String] ?? "-", w[kCGWindowIsOnscreen as String] ?? "-", w[kCGWindowBounds as String] ?? "-")
}
