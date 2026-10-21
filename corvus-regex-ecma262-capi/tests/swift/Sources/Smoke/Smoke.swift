import CCorvusRegexEcma262

/// Where the first match of `pattern` (with `flags`) is in `text`, in bytes, through the C library: nil when there is
/// none or the pattern does not compile.
public func firstMatch(pattern: String, flags: String, in text: String) -> Range<Int>? {
    var regex: OpaquePointer?
    var pattern = pattern
    var flags = flags
    let compiled = pattern.withUTF8 { p in
        flags.withUTF8 { f in cre_regex_new(chars(p), p.count, chars(f), f.count, &regex) }
    }
    guard compiled == cre_status(CRE_OK), let regex else { return nil }
    defer { cre_regex_free(regex) }
    var found = false
    var match = cre_match(start: 0, end: 0)
    var text = text
    let status = text.withUTF8 { cre_regex_find(regex, chars($0), $0.count, 0, &match, &found) }
    guard status == cre_status(CRE_OK), found else { return nil }
    return Int(match.start)..<Int(match.end)
}

/// The C library's version, such as "0.1.1".
public var libraryVersion: String {
    let v = cre_version()
    return String(decoding: UnsafeRawBufferPointer(start: v.ptr, count: v.len), as: UTF8.self)
}

private func chars(_ bytes: UnsafeBufferPointer<UInt8>) -> UnsafePointer<CChar>? {
    UnsafeRawPointer(bytes.baseAddress)?.assumingMemoryBound(to: CChar.self)
}
