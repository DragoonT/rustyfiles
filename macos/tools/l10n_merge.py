#!/usr/bin/env python3
"""Takes translations into macos/Localization/<lang>.json after checking each, as the Windows
app's scripts/i18n_merge.py does:

    l10n_merge.py <lang> <file.json> [<file.json> ...]

A file is {"<English key>": "<translation>"}. A translation is REFUSED (the others are still
taken) when its key is not a text of the app, it is empty, its format specifiers are not the
key's (%@ and %lld; with several, %1$@, %2$@ ... in any order are fine too), its line breaks
are not as many, or it starts or ends with a space where the English does not.
"""
import json, os, re, sys

HERE = os.path.dirname(os.path.abspath(__file__))
MAC = os.path.dirname(HERE)
sys.path.insert(0, HERE)
from l10n import keys  # noqa: E402

SPEC = re.compile(r'%(\d+\$)?(@|lld|d)')


def kinds(s, positional):
    out = []
    for m in SPEC.finditer(s.replace('%%', '')):
        if positional and m.group(1):
            out.append(m.group(1) + m.group(2))
        else:
            out.append(m.group(2))
    return sorted(out)


def problem(key, value):
    if not isinstance(value, str) or not value.strip():
        return 'empty'
    want = kinds(key, False)
    got = kinds(value, False)
    if got != want:
        # several specifiers may be numbered so the language can reorder them
        numbered = sorted(f'{i}${k}' for i, k in enumerate((m.group(2) for m in SPEC.finditer(key.replace('%%', ''))), 1))
        if len(want) < 2 or kinds(value, True) != numbered:
            return f'specifiers {want} in English, {kinds(value, True)} here'
    if value.count('\n') != key.count('\n'):
        return 'line breaks differ'
    if (value[0] == ' ') != (key[0] == ' ') or (value[-1] == ' ') != (key[-1] == ' '):
        return 'spaces at the edges differ'
    return None


def main(lang, files):
    wanted = keys(os.path.join(MAC, 'Sources', 'RustyFiles'))
    path = os.path.join(MAC, 'Localization', lang + '.json')
    table = json.load(open(path, encoding='utf-8')) if os.path.exists(path) else {}
    taken = refused = 0
    for f in files:
        for k, v in json.load(open(f, encoding='utf-8')).items():
            why = 'not a text of the app' if k not in wanted else problem(k, v)
            if why:
                refused += 1
                print(f'{lang}: refused {k!r}: {why}', file=sys.stderr)
            else:
                table[k] = v
                taken += 1
    with open(path, 'w', encoding='utf-8') as out:
        out.write(json.dumps(table, ensure_ascii=False, indent=0))
    missing = sum(1 for k in wanted if k not in table)
    print(f'{lang}: {taken} taken, {refused} refused, {missing} still missing')
    return 1 if refused else 0


if __name__ == '__main__':
    sys.exit(main(sys.argv[1], sys.argv[2:]))
