import AppKit

/// Tags (settings_view.rs `tags`): Finder's own tags, which every Mac app shares. rustyfiles
/// keeps the list it offers (names and colours), finds tagged files with Spotlight, and renames
/// or deletes a tag on every file that has it.
@MainActor
enum TagStore {
    struct Tag: Codable, Equatable {
        var name: String
        /// Finder's colour index: 0 none, 1 gray, 2 green, 3 purple, 4 blue, 5 yellow, 6 red, 7 orange.
        var color: Int
    }

    private static let key = "tagList"

    /// The tags offered in menus, the sidebar and Home: the user's own list, or Finder's
    /// coloured tags as this Mac names them.
    static var all: [Tag] {
        get {
            if let data = UserDefaults.standard.data(forKey: key), let list = try? JSONDecoder().decode([Tag].self, from: data) { return list }
            let names = NSWorkspace.shared.fileLabels
            return names.indices.dropFirst().map { Tag(name: names[$0], color: $0) }
        }
        set {
            if let data = try? JSONEncoder().encode(newValue) { UserDefaults.standard.set(data, forKey: key) }
            NotificationCenter.default.post(name: .rfSettingsChanged, object: nil, userInfo: ["key": "tagList"])
        }
    }

    static func color(_ index: Int) -> NSColor? {
        let colors = NSWorkspace.shared.fileLabelColors
        return index > 0 && index < colors.count ? colors[index] : nil
    }

    static func color(of name: String) -> NSColor? { all.first { $0.name == name }.flatMap { color($0.color) } }

    static let colorNames: [String] = [
        String(localized: "No colour"), String(localized: "Gray"), String(localized: "Green"), String(localized: "Purple"),
        String(localized: "Blue"), String(localized: "Yellow"), String(localized: "Red"), String(localized: "Orange"),
    ]

    /// The files that carry `name`, from Spotlight (only what it indexes).
    nonisolated static func files(tagged name: String) -> [String] {
        let p = Process()
        p.executableURL = URL(fileURLWithPath: "/usr/bin/mdfind")
        let safe = name.replacingOccurrences(of: "\"", with: "").replacingOccurrences(of: "\\", with: "")
        p.arguments = ["kMDItemUserTags == \"\(safe)\""]
        let out = Pipe()
        p.standardOutput = out
        p.standardError = FileHandle.nullDevice
        guard (try? p.run()) != nil else { return [] }
        let data = out.fileHandleForReading.readDataToEndOfFile()
        p.waitUntilExit()
        return String(decoding: data, as: UTF8.self).split(separator: "\n").map(String.init)
    }

    /// Gives `new` in place of `old` (nil: takes it away) on every file that has `old`.
    nonisolated static func retag(_ old: String, to new: String?) {
        for path in files(tagged: old) {
            var u = URL(fileURLWithPath: path)
            var tags = (try? u.resourceValues(forKeys: [.tagNamesKey]).tagNames) ?? []
            tags.removeAll { $0 == old }
            if let new, !tags.contains(new) { tags.append(new) }
            try? (u as NSURL).setResourceValue(tags, forKey: .tagNamesKey)
            u.removeAllCachedResourceValues()
        }
    }

    static func rename(_ i: Int) {
        var list = all
        guard list.indices.contains(i) else { return }
        let old = list[i].name
        guard let name = TagPrompt.ask(String(localized: "Rename the tag “\(old)”"), initial: old), name != old else { return }
        list[i].name = name
        all = list
        DispatchQueue.global(qos: .userInitiated).async { retag(old, to: name) }
    }

    static func recolor(_ i: Int) {
        var list = all
        guard list.indices.contains(i) else { return }
        let menu = NSMenu()
        for (c, title) in colorNames.enumerated() {
            let item = ClosureMenuItem(title: title) {
                list[i].color = c
                all = list
            }
            item.state = list[i].color == c ? .on : .off
            if let col = color(c) { item.image = TagStore.dot(col) }
            menu.addItem(item)
        }
        menu.popUp(positioning: nil, at: NSEvent.mouseLocation, in: nil)
    }

    static func delete(_ i: Int) {
        var list = all
        guard list.indices.contains(i) else { return }
        let name = list[i].name
        let alert = NSAlert()
        alert.messageText = String(localized: "Delete the tag “\(name)”?")
        alert.informativeText = String(localized: "The tag is taken off every item. The files are not deleted.")
        alert.addButton(withTitle: String(localized: "Delete"))
        alert.addButton(withTitle: String(localized: "Cancel"))
        guard alert.runModal() == .alertFirstButtonReturn else { return }
        list.remove(at: i)
        all = list
        DispatchQueue.global(qos: .userInitiated).async { retag(name, to: nil) }
    }

    static func add() {
        guard let name = TagPrompt.ask(String(localized: "New tag"), initial: ""), !all.contains(where: { $0.name == name }) else { return }
        all = all + [Tag(name: name, color: 0)]
    }

    static func dot(_ color: NSColor?) -> NSImage {
        NSImage(size: NSSize(width: 12, height: 12), flipped: false) { r in
            if let color {
                color.setFill()
                NSBezierPath(ovalIn: r.insetBy(dx: 1, dy: 1)).fill()
            } else {
                NSColor.secondaryLabelColor.setStroke()
                NSBezierPath(ovalIn: r.insetBy(dx: 1.5, dy: 1.5)).stroke()
            }
            return true
        }
    }

    /// The address of a tag's results page.
    static func url(_ name: String) -> URL {
        URL(string: "rustyfiles:tag/" + (name.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? name))!
    }

    /// The tag a results page shows, if `url` is one.
    nonisolated static func name(of url: URL) -> String? {
        guard url.scheme == "rustyfiles" else { return nil }
        let spec = url.absoluteString.dropFirst("rustyfiles:".count)
        guard spec.hasPrefix("tag/") else { return nil }
        return String(spec.dropFirst(4)).removingPercentEncoding
    }
}

@MainActor
enum TagPrompt {
    static func ask(_ title: String, initial: String) -> String? {
        let alert = NSAlert()
        alert.messageText = title
        let field = NSTextField(string: initial)
        field.frame = NSRect(x: 0, y: 0, width: 240, height: 24)
        alert.accessoryView = field
        alert.addButton(withTitle: String(localized: "OK"))
        alert.addButton(withTitle: String(localized: "Cancel"))
        alert.window.initialFirstResponder = field
        guard alert.runModal() == .alertFirstButtonReturn else { return nil }
        let name = field.stringValue.trimmingCharacters(in: .whitespaces)
        return name.isEmpty ? nil : name
    }
}

extension SettingsContent {
    static func tagRows() -> [SettingsRow] {
        let tags = TagStore.all
        return [
            .table(
                title: String(localized: "Tags"),
                detail: String(localized: "Finder’s tags, shared with every app on this Mac. Renaming or deleting a tag changes it on every file that has it."),
                icon: "tag",
                entries: tags.map { ($0.name, nil, TagStore.color($0.color)) },
                empty: String(localized: "No tags"),
                rowActions: [
                    (String(localized: "Rename…"), { TagStore.rename($0) }),
                    (String(localized: "Colour"), { TagStore.recolor($0) }),
                    (String(localized: "Delete"), { TagStore.delete($0) }),
                ],
                buttons: [(String(localized: "New tag…"), { TagStore.add() })]),
        ]
    }
}
