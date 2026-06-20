# Inline Image Compatibility

Knightty supports the Phase 4-F1 subset of the iTerm2 Inline Images Protocol and
the Phase 4-F3 direct-transmission subset of the Kitty Graphics Protocol.

## Supported

```text
OSC 1337 ; File=inline=1:<base64 PNG> BEL
OSC 1337 ; File=inline=1:<base64 PNG> ST
```

- Static PNG images only
- Base64 payloads
- Optional Base64 `name` metadata for diagnostics
- Optional `width` and `height` as positive cell counts or `auto`
- `preserveAspectRatio=1`
- Transparent PNG alpha blending
- Cell-relative placement at the current cursor
- Normal output scrolling and viewport scrollback visibility
- Cell-based resize behavior

Kitty graphics commands use the following APC envelope:

```text
ESC _ G <control-data> ; <base64 PNG> ESC \
```

Supported Kitty operations and keys:

- `a=T`: transmit and display a PNG
- `a=t`: transmit a PNG without displaying it
- `a=p`: place a previously transmitted image
- `a=d,d=i`: soft-delete placements by image ID, optionally narrowed by `p`
- `f=100` and direct transmission `t=d`
- Single-chunk `m=0` and multipart `m=1` continuation followed by final `m=0`
- Client image IDs `i`, placement IDs `p`, cell dimensions `c` and `r`
- Cursor policy `C=0/1` and response policy `q=0/1/2`
- `OK`, `EINVAL`, `ENOENT`, `ENODATA`, `EBADPNG`, `E2BIG`, and `ENOSPC`

Kitty image IDs are mapped to Knightty-internal image IDs. A non-zero
`(i, p)` pair identifies one named placement and placing it again replaces that
placement. An omitted or zero `p` creates another anonymous placement. Reusing
an image ID for a successful transmission replaces the old image and removes
all of its old placements.

Multipart transfers hold one bounded partial upload at a time. Only the first
chunk carries the image and placement controls; continuation chunks carry `m`
and optionally `q`. Each chunk is at most 4096 Base64 bytes, and non-final
chunks have a length divisible by four. Decoding and image replacement happen
only after the final chunk validates successfully.

Lowercase `d=i` is a soft delete: placements disappear, but the transmitted
image remains available to a later `a=p` command.

For iTerm2 images, the cursor advances by the calculated row count using
block-style carriage-return/newline steps. Kitty `C=0` moves the cursor after
the placement rectangle, while `C=1` leaves it unchanged.

## Limits

Defaults are configurable under `[graphics]`:

```toml
[graphics]
enabled = true
max_encoded_bytes = 16777216
max_decoded_bytes = 134217728
max_width = 8192
max_height = 8192
max_pixels = 32000000
max_images = 128
max_gpu_bytes = 268435456
```

Invalid, malformed, unsupported, or oversized image payloads are never printed
as terminal text. iTerm2 failures are logged; Kitty failures produce a bounded
APC error response unless suppressed by `q`.

## Current Limitations

- PNG only; JPEG, GIF, WebP, and animation are not supported.
- Windows ConPTY can remove Kitty APC sequences before Knightty's PTY reader
  receives them. This was reproduced on Windows build `10.0.26200.7171`: the
  reader received the ASCII text immediately before and after the APC, but
  received zero `ESC _ G` starts and zero `ESC \` terminators. PowerShell
  `[Console]::Write`, `[Console]::Out.Write`, and
  `[Console]::OpenStandardOutput().Write(byte[])` all produced the same result.
  This is below the graphics router boundary; changing the Kitty parser or
  renderer cannot recover bytes that ConPTY removed.
- Pixel and percentage width/height units are rejected.
- `preserveAspectRatio=0` is rejected.
- Kitty RGB/RGBA pixels, files, temporary files, shared memory, source
  rectangles, pixel offsets, z-index, Unicode placeholders, relative placement,
  and animation are not supported.
- iTerm2 file paths, downloads, multipart transfers, shared memory, and
  networking are not supported.
- Anonymous and iTerm2 images are retained while a placement remains in live
  terminal history. Named Kitty images also remain available after transfer-only
  commands and soft deletion. Pixel data is not serialized into persistent
  scrollback.
- iTerm2 image replacement and client-supplied IDs remain unsupported.
- Sixel is not supported.

## Smoke Test

Run Knightty, then execute:

```powershell
powershell -ExecutionPolicy Bypass -File .\docs\dev\show-inline-png.ps1
```

The script emits a repository-local embedded PNG, followed by ordinary text.

For the Kitty direct-transmission path, run:

```powershell
powershell -ExecutionPolicy Bypass -File .\docs\dev\show-kitty-png.ps1
```

With no arguments the script displays an embedded sample. To display an
arbitrary PNG, start Knightty from the repository and run the script inside its
shell:

```powershell
cargo run -p app --release

# Run this command inside the Knightty window.
powershell -ExecutionPolicy Bypass -File .\docs\dev\show-kitty-png.ps1 `
  -Path "C:\path\image.png" -Columns 40
```

`-Rows` can set an explicit cell height; leaving it at zero preserves the PNG's
aspect ratio using the requested column width. `-ImageId` selects the non-zero
Kitty image ID used for replacement. The configured `max_encoded_bytes` applies
to the complete Base64 upload, not to each individual chunk.

### Transport diagnostics

Set `KNIGHTTY_GRAPHICS_DIAGNOSTICS=1` before starting Knightty to inspect the
PTY boundary without logging the complete image payload:

```powershell
$env:KNIGHTTY_GRAPHICS_DIAGNOSTICS = "1"
cargo run -p app --release 2> knightty-graphics.log
```

The diagnostic records each raw PTY read's byte count and only its first and
last 16 bytes. It also records cumulative counts for `ESC _ G`, `ESC \`,
completed Kitty commands, successful PNG decodes, and created Kitty placements.

The three PowerShell output APIs can be compared through Windows ConPTY with:

```powershell
cargo test -p pty --test windows_conpty_apc -- --ignored --nocapture
```

On a Unix host, the raw PTY preservation E2E test is:

```sh
cargo test -p pty --test unix_apc -- --nocapture
```
