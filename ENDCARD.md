# End card

Every MP4 exported by `cast2mp4`, `cloud-sessions cast --mp4` and `remote-sessions cast --mp4` (and the
remote-sessions recordings `.mp4` route) ends with a 5 second card: which tool recorded it, the sha256 of the
`.cast` (so the recording stays verifiable), an optional sponsor box and a call to action.

The `.cast` file is never modified. The card is appended at the MP4 step only.

## Change the look without a rebuild

The look is a JSON file: `~/.config/endcard/endcard.json` (override with the env var `ENDCARD_CONFIG`).
No file means the built-in defaults. Edit, preview, look:

```
cloud-sessions endcard init                       # writes the default spec if absent, never overwrites
$EDITOR ~/.config/endcard/endcard.json
cloud-sessions endcard preview --out card.png     # [--cast file.cast] [--size 1280x720]
```

`remote-sessions` has the same `endcard init` and `endcard preview`. The shell exporter has
`cast2mp4 --endcard-init` and `cast2mp4 --endcard-preview card.png [--cast file.cast]` (it uses `bin/endcard`).

Turn the card off for one export with `--no-endcard`, or for good with `"enabled": false`.

## Spec

```json
{
  "enabled": true,
  "duration_s": 5,
  "fade_s": 0.5,
  "background": { "color": "#0b0f17" },
  "font": "/System/Library/Fonts/Menlo.ttc",
  "lines": [
    { "text": "Recorded with {tool}", "size": 0.0556, "color": "#ffffff", "y": 0.18 }
  ],
  "sponsor": { "enabled": true, "text": "Sponsored by example.com  -  one round-trip on the rail",
               "color": "#f2c94c", "box_color": "#f2c94c", "y": 0.66, "size": 0.0333 },
  "cta": "Record yours: remote-sessions, free and MIT"
}
```

- `background`: `{"color": "#rrggbb"}` or `{"image": "/path/card.png"}` (scaled and padded to the recording size).
- `font`: a font file. Default: Menlo on macOS, else DejaVu Sans Mono or Liberation Mono, else ffmpeg picks.
- `lines[]`: `text`, `size` (fraction of the video height), `color` (`#rrggbb`), `y` (top of the line, fraction of the height).
  Lines are centered and shrunk to fit the frame. A `lines` list replaces the default lines.
- `sponsor`: drawn as a box sized to its text with padding. `y` is the vertical CENTER of the box.
- Placeholders, in any text: `{tool}` (cloud-sessions, remote-sessions or cast2mp4), `{cast_name}`, `{sha256}`
  (of the `.cast`), `{sponsor}` (the sponsor text), `{cta}`.
- Unknown fields are ignored. Bad JSON is one warning plus the defaults: styling never fails an export.
- If ffmpeg or its `drawtext` filter is missing, the MP4 is written without the card and one warning is printed.

Text is handed to ffmpeg through text files, never escaped into the filter, and the only recording-derived text
on the card is the cast file name and its sha256.

## Sponsor box and Pro

The local tools are MIT, so the sponsor box is simply `"sponsor": {"enabled": false}` in the spec.
Pro (hosted) removes the sponsor box on hosted shares.
