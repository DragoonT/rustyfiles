"""Writes what a locale still has to translate, for a translator (a person or the `translator` agent).

    python scripts/i18n_todo.py de-DE        # -> target/i18n/de-DE.todo.json
    python scripts/i18n_todo.py --all        # one line per locale: how many texts are missing

The todo file is one JSON object:
    "locale":  the code
    "terms":   English -> the translation this locale ALREADY uses, for the short texts it has (labels, button names): the words
               to stay consistent with
    "texts":   id -> English, for every text of _source.json the locale file does not have yet
One entry per line, so it can be read in parts. Translations go into a file {"<id>": "<translation>", ...} and are put into the
locale file by scripts/i18n_merge.py, which checks them.
"""
import json
import pathlib
import sys

sys.stdout.reconfigure(encoding="utf-8")

ROOT = pathlib.Path(__file__).resolve().parent.parent
LOC = ROOT / "crates" / "rf-app" / "locales"
OUT = ROOT / "target" / "i18n"
# how many of the locale's own short texts are given as terms
MAX_TERMS = 160


def load(code):
    source = json.loads((LOC / "_source.json").read_text(encoding="utf-8"))
    path = LOC / f"{code}.json"
    data = json.loads(path.read_text(encoding="utf-8")) if path.exists() else {}
    return source, data


def missing(source, data):
    return [e for e in source if not isinstance(data.get(e["id"]), str) or not data[e["id"]].strip()]


def terms(source, data):
    short = [e for e in source if e["id"] in data and len(e["en"]) <= 24 and len(e["en"].split()) <= 3 and "{" not in e["en"]]
    # the shortest first: they are the words everything else is made of
    short.sort(key=lambda e: (len(e["en"].split()), len(e["en"])))
    out = {}
    for e in short:
        if e["en"] not in out and data[e["id"]] != e["en"]:
            out[e["en"]] = data[e["id"]]
        if len(out) >= MAX_TERMS:
            break
    return out


def lines(obj):
    """A JSON object with one entry per line."""
    if not obj:
        return "{}"
    return "{\n" + ",\n".join(f"  {json.dumps(k, ensure_ascii=False)}: {json.dumps(v, ensure_ascii=False)}" for k, v in obj.items()) + "\n }"


def main():
    if len(sys.argv) != 2:
        print(__doc__)
        sys.exit(2)
    if sys.argv[1] == "--all":
        source = json.loads((LOC / "_source.json").read_text(encoding="utf-8"))
        for p in sorted(LOC.glob("*.json")):
            if not p.name.startswith("_"):
                print(f"{p.stem}: {len(missing(source, json.loads(p.read_text(encoding='utf-8'))))} missing")
        return
    code = sys.argv[1]
    if not (LOC / f"{code}.json").exists():
        print(f"{code}: no locale file")
        sys.exit(1)
    source, data = load(code)
    todo = missing(source, data)
    OUT.mkdir(parents=True, exist_ok=True)
    out = OUT / f"{code}.todo.json"
    text = "{\n" + f' "locale": {json.dumps(code)},\n "terms": {lines(terms(source, data))},\n "texts": {lines({e["id"]: e["en"] for e in todo})}\n' + "}\n"
    json.loads(text)  # (it is what it says it is)
    out.write_text(text, encoding="utf-8", newline="\n")
    print(f"{code}: {len(todo)} texts to translate -> {out}")


if __name__ == "__main__":
    main()
