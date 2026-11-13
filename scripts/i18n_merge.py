"""Puts translations into a locale file, after checking each of them.

    python scripts/i18n_merge.py de-DE target/i18n/de-DE.part1.json
    python scripts/i18n_merge.py de-DE part.json --replace     # may also replace texts the locale already has

The file is {"<id>": "<translation>", ...} (ids from _source.json, see scripts/i18n_todo.py). A translation is REFUSED, and the
others are still taken, when
  * its id is not in _source.json, or it is not a text, or it is empty;
  * its placeholders ({n}, {n:03}, {name}, ...) are not exactly the English text's;
  * its line breaks are not as many as the English text's;
  * it begins or ends with a space where the English does not (or the other way round);
  * the locale already has that text and --replace was not given.
It is taken WITH A WARNING when it is the English text itself (longer than a word or two, and not for en-GB): probably not
translated. The locale file is written in the order of _source.json, as the other tools write it. Exit code 1 if anything was
refused.
"""
import json
import pathlib
import re
import sys

sys.stdout.reconfigure(encoding="utf-8")

ROOT = pathlib.Path(__file__).resolve().parent.parent
LOC = ROOT / "crates" / "rf-app" / "locales"
PH = re.compile(r"\{[^{}]*\}")


def problem(en, value):
    """Why `value` cannot be the translation of `en` (None: it can)."""
    if not isinstance(value, str):
        return "not a text"
    if not value.strip():
        return "empty"
    if sorted(PH.findall(value)) != sorted(PH.findall(en)):
        return f"placeholders differ: {sorted(PH.findall(en))} in English, {sorted(PH.findall(value))} here"
    if value.count("\n") != en.count("\n"):
        return f"{en.count(chr(10))} line break(s) in English, {value.count(chr(10))} here"
    if (value[0] == " ") != (en[0] == " ") or (value[-1] == " ") != (en[-1] == " "):
        return "a space at the beginning or the end that the English does not have (or the other way round)"
    return None


def main():
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    replace = "--replace" in sys.argv
    if len(args) != 2:
        print(__doc__)
        sys.exit(2)
    code, part = args[0], pathlib.Path(args[1])
    path = LOC / f"{code}.json"
    if not path.exists():
        print(f"{code}: no locale file")
        sys.exit(1)
    source = json.loads((LOC / "_source.json").read_text(encoding="utf-8"))
    by_id = {e["id"]: e for e in source}
    data = json.loads(path.read_text(encoding="utf-8"))
    try:
        new = json.loads(part.read_text(encoding="utf-8"))
    except (OSError, ValueError) as e:
        print(f"{part}: cannot be read as JSON: {e}")
        sys.exit(1)
    if not isinstance(new, dict):
        print(f"{part}: not an object of id -> translation")
        sys.exit(1)
    taken, refused, warned = 0, [], []
    for key, value in new.items():
        e = by_id.get(key)
        if e is None:
            refused.append(f"{key}: no such id")
            continue
        why = problem(e["en"], value)
        if why:
            refused.append(f"{key} ({e['en'][:60]!r}): {why}")
            continue
        if key in data and isinstance(data[key], str) and data[key].strip() and not replace:
            if data[key] != value:
                refused.append(f"{key}: the locale already has this text (--replace to change it)")
            continue
        if value == e["en"] and code != "en-GB" and len(e["en"].split()) > 2 and "{" not in e["en"]:
            warned.append(f"{key}: the same as the English ({e['en'][:60]!r})")
        data[key] = value
        taken += 1
    ordered = {e["id"]: data[e["id"]] for e in source if e["id"] in data}
    path.write_text(json.dumps(ordered, ensure_ascii=False, indent=1), encoding="utf-8", newline="\n")
    left = sum(1 for e in source if e["id"] not in ordered)
    print(f"{code}: {taken} taken, {len(refused)} refused, {len(warned)} warning(s); {left} still missing")
    for line in refused:
        print("   refused", line)
    for line in warned:
        print("   warning", line)
    sys.exit(1 if refused else 0)


if __name__ == "__main__":
    main()
