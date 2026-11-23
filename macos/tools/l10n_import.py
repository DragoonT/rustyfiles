#!/usr/bin/env python3
"""Fills macos/Localization/<lang>.json from the Windows app's translations.

The Windows app (crates/rf-app/locales) keeps 47 languages by text id, with the English text
of each id in _source.json. A Mac text whose English matches a Windows English text (the
interpolations aside) takes that text's translation, its {placeholders} turned into the
Mac's format specifiers: %@ / %lld when there is one, %1$@, %2$@ ... when there are several,
so a language may put them in its own order. Texts the Mac table already has are kept.

    l10n_import.py                 # every language
    l10n_import.py de fr           # these only (Apple's codes)
    l10n_import.py --todo DIR      # also writes DIR/<lang>.todo.json: the Mac texts still missing

LANGS maps Windows' locale names to Apple's .lproj codes.
"""
import glob, json, os, re, sys

HERE = os.path.dirname(os.path.abspath(__file__))
MAC = os.path.dirname(HERE)
ROOT = os.path.dirname(MAC)
WIN = os.path.join(ROOT, 'crates', 'rf-app', 'locales')
sys.path.insert(0, HERE)
from l10n import keys  # noqa: E402

LANGS = {
    'af': 'af', 'ar': 'ar', 'be-BY': 'be', 'bg': 'bg', 'ca': 'ca', 'cs-CZ': 'cs', 'da': 'da',
    'de-DE': 'de', 'el': 'el', 'en-GB': 'en-GB', 'es-419': 'es-419', 'es-ES': 'es', 'fa-IR': 'fa',
    'fi-FI': 'fi', 'fil-PH': 'fil', 'fr-FR': 'fr', 'he-IL': 'he', 'hi-IN': 'hi', 'hr-HR': 'hr',
    'hu-HU': 'hu', 'hy-AM': 'hy', 'id-ID': 'id', 'it-IT': 'it', 'ja-JP': 'ja', 'ka': 'ka',
    'km-KH': 'km', 'ko-KR': 'ko', 'lt-LT': 'lt', 'lv-LV': 'lv', 'ms-MY': 'ms', 'nb-NO': 'nb',
    'nl-NL': 'nl', 'pl-PL': 'pl', 'pt-BR': 'pt-BR', 'pt-PT': 'pt-PT', 'ro-RO': 'ro', 'ru-RU': 'ru',
    'sk-SK': 'sk', 'sq-AL': 'sq', 'sr-Cyrl': 'sr', 'sv-SE': 'sv', 'ta': 'ta', 'th-TH': 'th',
    'uk-UA': 'uk', 'vi': 'vi', 'zh-Hans': 'zh-Hans', 'zh-Hant': 'zh-Hant',
}

SPEC = re.compile(r'%(?:\d+\$)?(?:@|lld|d)')
PH = re.compile(r'\{[^{}]*\}')


def shape(text, pattern):
    """The text with its placeholders as {} (to compare Mac and Windows English)."""
    return pattern.sub('{}', text.replace('%%', '%'))


def mac_specs(key):
    return [m.group(0) for m in SPEC.finditer(key.replace('%%', ''))]


def convert(win_en, translation, key):
    """Windows' translation with the Mac key's specifiers in place of its placeholders."""
    names = PH.findall(win_en)
    specs = mac_specs(key)
    if len(names) != len(specs) or sorted(PH.findall(translation)) != sorted(names):
        return None
    out = translation.replace('%', '%%')
    if len(specs) == 1:
        return out.replace(names[0], specs[0].replace('%', '\0'), 1).replace('\0', '%')
    # several: positional, in the English order, so the translation's own order holds
    for i, (n, s) in enumerate(zip(names, specs), 1):
        kind = 'lld' if s.endswith('lld') or s.endswith('d') else '@'
        out = out.replace(n, f'\0{i}${kind}', 1)
    return out.replace('\0', '%')


def main(argv):
    todo_dir = None
    if '--todo' in argv:
        i = argv.index('--todo')
        todo_dir = argv[i + 1]
        argv = argv[:i] + argv[i + 2:]
    wanted = keys(os.path.join(MAC, 'Sources', 'RustyFiles'))
    source = json.load(open(os.path.join(WIN, '_source.json'), encoding='utf-8'))
    by_shape = {}
    for e in source:
        by_shape.setdefault(shape(e['en'], PH), e)
    for win, mac in LANGS.items():
        if argv and mac not in argv:
            continue
        win_table = json.load(open(os.path.join(WIN, win + '.json'), encoding='utf-8'))
        path = os.path.join(MAC, 'Localization', mac + '.json')
        table = json.load(open(path, encoding='utf-8')) if os.path.exists(path) else {}
        added = 0
        for k in wanted:
            if k in table:
                continue
            e = by_shape.get(shape(k, SPEC))
            if not e or e['id'] not in win_table:
                continue
            t = convert(e['en'], win_table[e['id']], k)
            if t is not None and t.count('\n') == k.count('\n'):
                table[k] = t
                added += 1
        with open(path, 'w', encoding='utf-8') as f:
            f.write(json.dumps(table, ensure_ascii=False, indent=0))
        missing = {k: k for k in wanted if k not in table}
        if todo_dir:
            os.makedirs(todo_dir, exist_ok=True)
            with open(os.path.join(todo_dir, mac + '.todo.json'), 'w', encoding='utf-8') as f:
                f.write(json.dumps(missing, ensure_ascii=False, indent=1))
        print(f'{mac}: {added} from Windows, {len(table)} in all, {len(missing)} to translate')


if __name__ == '__main__':
    main(sys.argv[1:])
