# Knightty Cell Span Protocol

Knightty supports an experimental namespaced OSC extension that draws one
Unicode extended grapheme cluster inside a rectangular range of logical cells.
The terminal grid remains fixed-width; only the glyph rendering is scaled.

```text
ESC ] 7777 ; knightty ; span=<columns>x<rows> : <UTF-8 grapheme> ST
ESC ] 7777 ; knightty ; span=<columns>x<rows> : <UTF-8 grapheme> BEL
```

OSC 7777 is a Knightty-specific experimental extension. It is not a portable
terminal protocol. See the general OSC envelope in the
[XTerm control-sequence reference](https://invisible-island.net/xterm/ctlseqs/ctlseqs.html).

## Validation

- `columns` and `rows` are decimal integers from 1 through the current grid
  width and height respectively.
- The rectangle must be at least as wide as the grapheme's normal terminal
  width.
- The payload must be valid UTF-8, contain no control characters, and contain
  exactly one Unicode extended grapheme cluster.
- Invalid commands are consumed and ignored without changing the grid or
  cursor.
- Other OSC, CSI, iTerm2 image, and Kitty graphics sequences retain their
  existing behavior.

## Placement and lifetime

The current cursor is the rectangle's top-left anchor. If the rectangle does
not fit before the right edge, Knightty moves to column zero on the next line
before placement. If it does not fit before the bottom edge, Knightty scrolls
the required number of lines.

After placement the cursor remains at the top-left anchor. The OSC itself does
not advance to the cell after the rectangle. Consequently, ordinary text sent
immediately afterward overwrites the anchor and removes the entire span.

Writing, erasing, inserting, or deleting any cell in the reserved rectangle
removes the complete placement atomically. Cursor movement and SGR changes do
not. Placements follow normal scrolling, and primary and alternate screens keep
separate placements. A terminal resize releases all reservations and leaves
the anchor grapheme as normal-size terminal text.

## Rendering

Knightty shapes the span text once at the normal font size. Let `glyph_advance`
be the sum of the shaped glyph advances, not the occupied terminal-cell width.
The rendering scale is:

```text
min(span_pixel_width / glyph_advance,
    span_pixel_height / base_line_height)
```

The scaled glyph is centered horizontally and vertically and clipped to the
rectangle. A non-positive advance falls back to normal-size anchor rendering.
Foreground color, bold, italic, inverse, and underline are inherited from the
current terminal attributes. Background, selection, OSC 8 hyperlink hit area,
and search highlighting cover the complete rectangle. The cursor remains one
normal logical cell at the anchor.

Selecting any occupied cell resolves to the anchor. Copying such a selection
includes the grapheme exactly once.

## Current limitation

Version 1 accepts only one extended grapheme cluster. The command and placement
types store text as a string so a future version can shape and wrap multiple
graphemes across the same span model without adding one OSC per grapheme.

## Smoke test

Start Knightty with a grid of at least 40 columns by 18 rows, then run this
inside the Knightty shell:

```powershell
powershell -ExecutionPolicy Bypass -File .\docs\dev\show-cell-span.ps1
```

The script shows ASCII, full-width CJK, combining-mark, ZWJ emoji, and
non-square examples. It uses both ST and BEL termination.
