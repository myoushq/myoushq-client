# The mark

The myous mark is a Greek mu drawn as one path, with a ring on each of its
four free ends (both tops, the foot, the tail), like the pads of a printed
circuit board: a monogram for myous, and a trace with its pads for the
network. White on the brand navy `#1f3a5f`, on a rounded square; where it
stands alone, one colour (the text colour). It is the site's favicon and
the apps' icon.

| File | What |
|---|---|
| `mark.svg` | The mark: the navy rounded square with the white mu, `viewBox="0 0 32 32"`. The source of every icon. |
| `mark-glyph.svg` | The mu alone, black on transparent: for one-colour uses (a menu bar template image, text, print). Not used by the apps yet. |
| `make-icons.sh` | Regenerates the icon files in the repository from `mark.svg`. |

The geometry, in the 32 by 32 box: the stroke
`M7.5 9.6V22.4M7.5 13.5a5.25 5.25 0 0 0 10.5 0M18 9.6V15C18 19.89 20.27 22.52 23.2 23.52`
(width 2.75, round joins, default caps), rings of radius 3.1 and stroke
width 1.6 (outer radius 3.9, hole 2.3) centred at (7.5, 6.5), (7.5, 25.5),
(18, 6.5) and (26.25, 24), drawn after the path with no fill, so the hole
shows what is behind (the navy, the tray's state colour, nothing in the
glyph); the square's corners radius 7. The stroke ends on each ring's
centreline, 3.1 before the pad's centre measured along the path, so nothing
crosses a hole: the verticals are cut at y 9.6 and 22.4, and the tail is
the cubic (18, 15) (18, 21.5) (22, 24) (26.25, 24) split by de Casteljau
where 3.1 of its arc length remains (t = 0.753), the control points rounded
to two decimals.

## Where it is used

- **myous for Mac, Linux and Windows** (`worker/desktop`): the files in
  `src-tauri/icons/` (PNGs, `icon.ico`, `icon.icns` for the Mac app, the
  Windows Store logos; `toast.rs` shows `128x128.png` on Windows toasts)
  come from `make-icons.sh`. The tray icon, the menu bar icon on a Mac,
  is drawn at run time by `trayIcon` in `ui/src/app.ts` from the
  `MARK_*` constants: the same mark on a square in the state's colour
  (it shows through the rings), with a white dot in the corner while a
  request runs (on a Mac 36
  pixels for its 18 points).
- **The Objective-C myous for Mac** (`worker/mac`, superseded and no
  longer released): drawn in code, no image files. `drawMark` in
  `src/Icon.m` draws the same geometry; `myous --render-icon DIR` writes
  the iconset and its `make-app.sh` packs it into `myous.icns`.

A change to the mark goes in `mark.svg`, `mark-glyph.svg` and the
`MARK_*` constants in `worker/desktop/ui/src/app.ts` (and `drawMark` in
`worker/mac/src/Icon.m` while that app is kept); then run
`make-icons.sh`.

## Regenerating

```sh
worker/icons/make-icons.sh
```

It rasterizes `mark.svg` at 1024 by 1024 (with `rsvg-convert`; without it,
with Playwright's Chromium from the Python in `MYOUS_PW_PYTHON`, default
`/tmp/pw-venv/bin/python`), runs `tauri icon` on that (it runs `npm ci` in
`worker/desktop` first if the Tauri CLI is not installed), drops the
Android and iOS sets and replaces `worker/desktop/src-tauri/icons/`. It can
be run again at any time (the PNGs and `icon.ico` come out the same byte
for byte; `icon.icns` does not, so commit it only when the mark changed).
