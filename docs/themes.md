# Theme folders

A theme folder changes rustyfiles' colours and the icons of folders and files. Pick one in
Settings › Appearance › Theme folder; "Open the themes folder" there shows where they live
(`%APPDATA%\rustyfiles\themes\`, or the portable copy's own folder).

```
themes\
  Nord\
    theme.toml
    icons\
      folder.svg
      file.svg
      pdf.png
      zip.svg
```

## Colours: `theme.toml`

Two tables, `[dark]` and `[light]`, for rustyfiles' dark and light theme (one of them is enough).
Each key is one of the palette's colours, each value `#RRGGBB` or `#RRGGBBAA`. A colour you leave
out keeps rustyfiles' own.

```toml
[dark]
accent = "#88C0D0"
accent_text = "#2E3440"
window_bg = "#2E3440"
content_bg = "#3B4252CC"
sidebar_bg = "#2E3440"
text = "#ECEFF4"
text_secondary = "#D8DEE9"
selection = "#88C0D040"
selection_border = "#88C0D0"

[light]
accent = "#5E81AC"
```

The keys: `window_bg`, `titlebar_bg`, `sidebar_bg`, `content_bg`, `card_bg`, `bar_bg`, `tile_bg`,
`tile_hover`, `border`, `divider`, `text`, `text_secondary`, `text_tertiary`, `accent`, `accent_text`,
`hover`, `pressed`, `selection`, `selection_border`, `tab_active`, `danger`.

With Mica or HyperGlass as the background material, the backgrounds stay partly see-through;
choose Settings › Appearance › Background material › Solid to see a theme's backgrounds as they are.

## Icons: `icons\`

Pictures in `.svg`, `.png` or `.ico`, named for what they stand for:

- `folder`: every folder;
- an extension (`pdf`, `zip`, `mp4` ...): files of that type;
- `file`: every other file.

A program's own icon (an `.exe`'s, a shortcut's) stays, unless the theme has a picture for exactly
its extension (`exe.svg`, `lnk.svg`). Pictures and videos keep their previews. Icon packs such as
Papirus come as SVG files that can be copied in and renamed this way.

Freedesktop packs are a separate setting, `Settings › Appearance › Icon theme`. When both are set,
this theme folder's pictures win and the pack fills what this folder does not define.
