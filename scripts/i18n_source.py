"""Brings crates/rf-app/locales/_source.json up to date with the program's texts.

    python scripts/i18n_source.py            # rewrites _source.json and says what changed
    python scripts/i18n_source.py --check    # changes nothing; exit code 1 when _source.json is behind

Run it after texts were added or changed and before the translators (scripts/i18n_todo.py reads _source.json: a text that is
not in it is missing in no locale, and is shown in English everywhere).

Where the texts come from:
  * the table of crates/rf-app/src/i18n_en.rs: (Turkish, English); the key is the Turkish text;
  * the inline pairs `i18n::tx("Türkçe", "English")` in crates/rf-app/src: the key is "en:<English>";
  * the counted and templated texts of i18n.rs, whose keys begin with "@": they are kept as _source.json has them.
An entry is {"id", "key", "tr", "en"} (and a "note" when a person wrote one); the id is FNV-1a 32 of the key, as `i18n::key_id`.

What it does to the list: an entry the program still has stays where it is, with its English renewed when that changed; an
entry the program no longer has goes; new entries are added at the end, in the order of the table. So a locale file, which the
merge writes in this order, only grows at its end. An id that a locale file has and this list no longer has is dropped from that
file by the next scripts/i18n_merge.py; --check and the run both tell how many locales still lack texts.
"""
import json
import pathlib
import re
import sys

sys.stdout.reconfigure(encoding="utf-8")

ROOT = pathlib.Path(__file__).resolve().parent.parent
SRC = ROOT / "crates" / "rf-app" / "src"
LOC = ROOT / "crates" / "rf-app" / "locales"
TABLE_START = "pub const EN: &[(&str, &str)] = &["
ESC = {"n": "\n", "r": "\r", "t": "\t", "0": "\0", "\\": "\\", '"': '"', "'": "'"}


def fnv(key):
    h = 0x811C9DC5
    for b in key.encode("utf-8"):
        h = ((h ^ b) * 0x01000193) & 0xFFFFFFFF
    return f"{h:08x}"


def string_at(text, i):
    """The Rust string literal whose opening quote is at `i`: (its value, the index after its closing quote)."""
    out = []
    i += 1
    while True:
        c = text[i]
        if c == '"':
            return "".join(out), i + 1
        if c != "\\":
            out.append(c)
            i += 1
            continue
        n = text[i + 1]
        if n in ESC:
            out.append(ESC[n])
            i += 2
        elif n == "x":
            out.append(chr(int(text[i + 2 : i + 4], 16)))
            i += 4
        elif n == "u":
            end = text.index("}", i)
            out.append(chr(int(text[i + 3 : end].replace("_", ""), 16)))
            i = end + 1
        elif n in "\r\n":
            # a backslash at the end of a line: the line break and the next line's indent are not part of the text
            i += 2
            while text[i] in " \t\r\n":
                i += 1
        else:
            raise ValueError(f"unknown escape \\{n} at {i}")


def table():
    """The (Turkish, English) pairs of i18n_en.rs, in its order."""
    text = (SRC / "i18n_en.rs").read_text(encoding="utf-8")
    i = text.index(TABLE_START) + len(TABLE_START)
    found = []
    while True:
        c = text[i]
        if c == '"':
            value, i = string_at(text, i)
            found.append(value)
        elif text.startswith("//", i):
            i = text.index("\n", i)
        elif c == "]":
            break
        elif c in " \t\r\n(),":
            i += 1
        else:
            raise ValueError(f"i18n_en.rs: unexpected {c!r} at {i}: {text[i:i + 40]!r}")
    if len(found) % 2:
        raise ValueError("i18n_en.rs: an odd number of texts in the table")
    pairs = list(zip(found[0::2], found[1::2]))
    if len({tr for tr, _ in pairs}) != len(pairs):
        raise ValueError("i18n_en.rs: a key stands twice")
    return pairs


TX = re.compile(r'\btx\(\s*(?=")')


def inline_pairs():
    """The (Turkish, English) of every `i18n::tx("…", "…")` in the code, each English once."""
    out = {}
    for path in sorted(SRC.rglob("*.rs")):
        if path.name == "i18n.rs":
            continue
        text = path.read_text(encoding="utf-8")
        for m in TX.finditer(text):
            try:
                tr, i = string_at(text, m.end())
                rest = re.match(r'\s*,\s*(?=")', text[i:])
                if not rest:
                    continue  # one argument: another function of that name
                en, _ = string_at(text, i + rest.end())
            except (ValueError, IndexError):
                continue
            out.setdefault(en, tr)
    return [(tr, en) for en, tr in out.items()]


def build(old):
    """The new list from the old one, and what changed: (list, added, gone, reworded, rekeyed)."""
    want = {tr: (tr, en) for tr, en in table()}
    want.update({f"en:{en}": (tr, en) for tr, en in inline_pairs()})
    out, gone, reworded, rekeyed, seen = [], [], [], [], set()
    for e in old:
        key = e["key"]
        if key.startswith("@"):
            out.append(e)
        elif key in want:
            tr, en = want[key]
            if e["en"] != en:
                reworded.append((e["id"], e["en"], en))
            if e["id"] != fnv(key):
                # the key was rewritten and the id stayed: the program asks for the key's own id and found no translation
                rekeyed.append((e["id"], fnv(key)))
            out.append({**e, "id": fnv(key), "tr": tr, "en": en})
        else:
            gone.append(e)
            continue
        seen.add(key)
    added = [{"id": fnv(key), "key": key, "tr": tr, "en": en} for key, (tr, en) in want.items() if key not in seen]
    out += added
    ids = [e["id"] for e in out]
    if len(set(ids)) != len(ids):
        twice = sorted({i for i in ids if ids.count(i) > 1})
        raise ValueError(f"two texts share an id: {twice}")
    return out, added, gone, reworded, rekeyed


def main():
    check = "--check" in sys.argv
    path = LOC / "_source.json"
    old = json.loads(path.read_text(encoding="utf-8"))
    new, added, gone, reworded, rekeyed = build(old)
    print(f"_source.json: {len(old)} texts -> {len(new)}; {len(added)} new, {len(gone)} gone, {len(reworded)} with other English, {len(rekeyed)} with another id")
    for e in gone:
        print(f"   gone      {e['id']}  {e['en'][:90]!r}")
    for i, was, now in reworded:
        print(f"   reworded  {i}  {was[:60]!r} -> {now[:60]!r}")
    for was, now in rekeyed:
        print(f"   id        {was} -> {now}")
    behind = new != old
    if not check and behind:
        path.write_text(json.dumps(new, ensure_ascii=False, indent=1), encoding="utf-8", newline="\n")
    ids = [e["id"] for e in new]
    lacking = 0
    for p in sorted(LOC.glob("*.json")):
        if p.name.startswith("_"):
            continue
        data = json.loads(p.read_text(encoding="utf-8"))
        # a translation follows its text to the text's new id; what the list no longer has goes
        for was, now in rekeyed:
            if was in data and now not in data:
                data[now] = data[was]
        kept = {i: data[i] for i in ids if i in data}
        if not check and list(kept.items()) != list(data.items()):
            p.write_text(json.dumps(kept, ensure_ascii=False, indent=1), encoding="utf-8", newline="\n")
        lacking += len(kept) < len(ids)
    print(f"locales that lack texts: {lacking}")
    sys.exit(1 if check and behind else 0)


if __name__ == "__main__":
    main()
