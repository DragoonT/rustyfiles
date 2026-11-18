#!/usr/bin/env python3
"""Builds the app's .lproj string tables from macos/Localization/<lang>.json.

Reads every String(localized: "...") in the Swift sources, turns its interpolations into the
format keys Foundation looks up ("\\(n) items" -> "%lld items" for an Int, "%@" otherwise),
and writes <out>/<lang>.lproj/Localizable.strings. Keys with no translation are listed and
fall back to English at run time.

usage: l10n.py <swift sources dir> <localization dir> <Resources dir>
"""
import glob, json, os, re, sys

# interpolations that are Int (everything else is formatted as %@)
INTS = {'n', 'count', 'rows.count', 'urls.count', 'urls.count - dirs', 'dirs', 't.files', 't.dirs',
        'names.count', 'o.names.count - 1', 'Int(o.fileCount)'}


def keys(src):
    out = {}
    for f in sorted(glob.glob(os.path.join(src, '*.swift'))):
        s = open(f, encoding='utf-8').read()
        for m in re.finditer(r'String\(localized:\s*"', s):
            i, text, kinds = m.end(), '', []
            while i < len(s):
                c = s[i]
                if c == '\\' and s[i + 1] == '(':
                    depth, j = 1, i + 2
                    while depth:
                        depth += {'(': 1, ')': -1}.get(s[j], 0)
                        j += 1
                    kinds.append('%lld' if s[i + 2:j - 1].strip() in INTS else '%@')
                    text += '\0'
                    i = j
                    continue
                if c == '\\':
                    text += {'n': '\n', '"': '"', '\\': '\\'}.get(s[i + 1], s[i:i + 2])
                    i += 2
                    continue
                if c == '"':
                    break
                text += c
                i += 1
            if kinds:
                parts = text.replace('%', '%%').split('\0')
                text = parts[0] + ''.join(k + p for k, p in zip(kinds, parts[1:]))
            out[text] = os.path.basename(f)
    return out


def quote(s):
    return '"' + s.replace('\\', '\\\\').replace('"', '\\"').replace('\n', '\\n') + '"'


def main(src, loc, res):
    wanted = keys(src)
    for path in sorted(glob.glob(os.path.join(loc, '*.json'))):
        lang = os.path.splitext(os.path.basename(path))[0]
        table = json.load(open(path, encoding='utf-8'))
        missing = [k for k in wanted if k not in table]
        for k in missing:
            print(f'l10n: {lang}: no translation for {k!r} ({wanted[k]})', file=sys.stderr)
        d = os.path.join(res, f'{lang}.lproj')
        os.makedirs(d, exist_ok=True)
        with open(os.path.join(d, 'Localizable.strings'), 'w', encoding='utf-8') as f:
            for k in wanted:
                if k in table:
                    f.write(f'{quote(k)} = {quote(table[k])};\n')
    os.makedirs(os.path.join(res, 'en.lproj'), exist_ok=True)


if __name__ == '__main__':
    main(*sys.argv[1:4])
