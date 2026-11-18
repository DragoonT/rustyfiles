import AppKit

/// Development aid (RF_SELFTEST): the file operation engine and its undo, run for real in a
/// scratch folder, each step checked; prints PASS or FAIL lines and quits.
@MainActor
enum SelfTest {
    private static var failures = 0

    private static func check(_ ok: Bool, _ what: String) {
        print((ok ? "PASS " : "FAIL ") + what)
        if !ok { failures += 1 }
    }

    static func run() {
        let fm = FileManager.default
        let root = fm.temporaryDirectory.appendingPathComponent("rf-selftest-\(UUID().uuidString)")
        let a = root.appendingPathComponent("a"), b = root.appendingPathComponent("b")
        try? fm.createDirectory(at: a.appendingPathComponent("folder/inner"), withIntermediateDirectories: true)
        try? fm.createDirectory(at: b, withIntermediateDirectories: true)
        try? Data(repeating: 7, count: 3_000_000).write(to: a.appendingPathComponent("big.bin"))
        try? "hello".write(to: a.appendingPathComponent("folder/inner/note.txt"), atomically: true, encoding: .utf8)
        let um = FileUndo.shared.manager

        // 1. copy a file and a folder: both arrive whole
        Transfer(.copy, [a.appendingPathComponent("big.bin"), a.appendingPathComponent("folder")], into: b).start(window: nil) { made in
            check(made.count == 2, "copy reports two new items")
            check((try? Data(contentsOf: b.appendingPathComponent("big.bin")).count) == 3_000_000, "copied file is whole")
            check(fm.fileExists(atPath: b.appendingPathComponent("folder/inner/note.txt").path), "copied folder keeps its inside")
            // 2. undo the copy: the copies go to the Trash, the originals stay
            um.undo()
            check(!fm.fileExists(atPath: b.appendingPathComponent("big.bin").path), "undo copy removes the copy")
            check(fm.fileExists(atPath: a.appendingPathComponent("big.bin").path), "undo copy keeps the original")
            // 3. redo: they come back
            um.redo()
            check(fm.fileExists(atPath: b.appendingPathComponent("folder/inner/note.txt").path), "redo copy brings the copies back")
            // 4. a copy into its own folder is "name - Copy"
            Transfer(.copy, [a.appendingPathComponent("big.bin")], into: a).start(window: nil) { made in
                check(made.first?.lastPathComponent == "big - Copy.bin", "copy into its own folder is named “- Copy”")
                // 5. move within the volume, then undo it
                let note = b.appendingPathComponent("folder/inner/note.txt")
                Transfer(.move, [note], into: b).start(window: nil) { made in
                    check(fm.fileExists(atPath: b.appendingPathComponent("note.txt").path) && !fm.fileExists(atPath: note.path), "move puts it in its new place")
                    um.undo()
                    check(fm.fileExists(atPath: note.path) && !fm.fileExists(atPath: b.appendingPathComponent("note.txt").path), "undo move puts it back")
                    // 6. Trash and back
                    let pairs = (try? FileOps.trash([b.appendingPathComponent("big.bin")])) ?? []
                    FileUndo.shared.record(.moves(pairs), title: "Move to Trash")
                    check(!fm.fileExists(atPath: b.appendingPathComponent("big.bin").path), "trash takes it away")
                    um.undo()
                    check(fm.fileExists(atPath: b.appendingPathComponent("big.bin").path), "undo trash brings it back")
                    // 7. rename and back
                    let renamed = try? FileOps.rename(b.appendingPathComponent("big.bin"), to: "huge.bin")
                    if let renamed { FileUndo.shared.record(.moves([(from: renamed, to: b.appendingPathComponent("big.bin"))]), title: "Rename") }
                    check(um.undoMenuItemTitle.contains("Rename"), "Edit says what undo does (\(um.undoMenuItemTitle))")
                    um.undo()
                    check(fm.fileExists(atPath: b.appendingPathComponent("big.bin").path), "undo rename gives the old name back")
                    // 8. the Trash remembers where things came from, and puts them back
                    let pb = (try? FileOps.trash([b.appendingPathComponent("big.bin")])) ?? []
                    check(pb.count == 1 && TrashBin.origin(of: pb[0].from) == b.appendingPathComponent("big.bin"), "Trash remembers the origin")
                    if let t = pb.first { _ = try? TrashBin.putBack([(trashed: t.from, origin: t.to)]) }
                    check(fm.fileExists(atPath: b.appendingPathComponent("big.bin").path), "Put Back returns it")
                    // 9. the organizer: pictures into Pictures/, then ⌘Z brings them back
                    let inbox = root.appendingPathComponent("inbox")
                    try? fm.createDirectory(at: inbox, withIntermediateDirectories: true)
                    for n in ["one.jpg", "two.png", "note.txt"] { fm.createFile(atPath: inbox.appendingPathComponent(n).path, contents: Data("x".utf8)) }
                    let rule = Organizer.Rule(id: 1, name: "Pictures", enabled: true, folder: inbox.path, subfolders: false, join: "all",
                                              conditions: [.init(type: "kind", kinds: ["image"])], action: .init(type: "move_to", folder: "Pictures"), auto: false)
                    let plan = Organizer.plan(Organizer.json([rule]), only: nil)
                    check(plan?.steps.count == 2, "organizer plans the two pictures (\(plan?.steps.count ?? -1))")
                    if let plan { Organizer.execute(plan, quiet: true) }
                    DispatchQueue.main.asyncAfter(deadline: .now() + 1) {
                        check(fm.fileExists(atPath: inbox.appendingPathComponent("Pictures/one.jpg").path), "organizer moved the pictures")
                        check(fm.fileExists(atPath: inbox.appendingPathComponent("note.txt").path), "organizer left the text file")
                        um.undo()
                        check(fm.fileExists(atPath: inbox.appendingPathComponent("one.jpg").path), "undo puts the pictures back")
                        try? fm.removeItem(at: root)
                        print(failures == 0 ? "ALL PASSED" : "\(failures) FAILED")
                        exit(failures == 0 ? 0 : 1)
                    }
                }
            }
        }
    }
}
