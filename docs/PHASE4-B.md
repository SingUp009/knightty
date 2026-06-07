Knightty の次フェーズとして、Phase 4-B: selection + copy を実装してください。

## 現在の状態

Phase 3-A / 3-B / 4-A は実装済みです。

Phase 3-A:
- DEC 1047/1048/1049 系 alternate screen 互換
- resize propagation
- scroll region / origin mode
- IL / DL / ICH / DCH
- bracketed paste API

Phase 3-B:
- DEC 1000/1002/1003/1004/1006 の mode state
- SGR mouse encoding
- 最小 legacy default mouse encoding
- focus in/out encoding
- winit mouse/focus event の PTY routing
- pixel-to-cell conversion
- pressed button tracking
- Ctrl+Shift+V / Shift+Insert の clipboard paste integration

Phase 4-A:
- primary screen 用 scrollback buffer
- scroll_offset state
- wheel scroll routing
- Ctrl+Shift+PageUp / PageDown / Home / End
- GridSnapshot の scrollback 表示対応
- config に terminal.scrollback_lines / scroll_multiplier を追加

検証済み:
- cargo fmt --all
- cargo test --workspace
- cargo build --workspace
- GUI 手動確認済み

未完了:
- cargo clippy --workspace --all-targets -- -D warnings
- git diff --check

## 作業前の必須確認

まず以下を実行してください。

```bash
git status
git branch --show-current
cargo clippy --workspace --all-targets -- -D warnings
git diff --check
cargo test --workspace
cargo build --workspace
````

`docs/PHASE3-B.md` が未追跡の場合、勝手に削除・変更・add しないでください。

## 今回の目的

scrollback を含む terminal text selection と clipboard copy を実装してください。

今回のスコープは以下に限定します。

1. selection state
2. mouse drag による範囲選択
3. selection rect の RenderPlan 反映
4. selected text extraction
5. Ctrl+Shift+C による clipboard copy
6. double click word selection
7. triple click line selection
8. unit tests

以下は今回実装しないでください。

* OSC 8 hyperlink click
* OSC 52 clipboard
* search
* rectangular/block selection
* semantic prompt navigation
* tabs / panes
* kitty graphics / sixel
* reflow scrollback

## 設計方針

`alacritty_terminal` の型を app/render 側へ漏らさないでください。
外側へ出す型は Knightty 独自型だけにしてください。

想定型:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelectionPoint {
    pub col: usize,
    pub row: isize,
}
```

`row` は scrollback を含む論理行として扱ってください。

例:

```text
row < 0:
  scrollback 側の行

row >= 0:
  visible screen 側の行
```

ただし、実装が難しい場合は以下のような明示型でも構いません。

```rust
pub enum BufferLineRef {
    Scrollback { index: usize },
    Screen { row: usize },
}

pub struct SelectionPoint {
    pub line: BufferLineRef,
    pub col: usize,
}
```

renderer/app には最終的に visible row/col の矩形情報だけを渡してください。

## Selection state

以下のような状態を持たせてください。

```rust
pub enum SelectionMode {
    Simple,
    Word,
    Line,
}

pub struct SelectionState {
    pub anchor: SelectionPoint,
    pub focus: SelectionPoint,
    pub mode: SelectionMode,
    pub active: bool,
}
```

API 案:

```rust
impl Terminal {
    pub fn clear_selection(&mut self);
    pub fn begin_selection(&mut self, point: SelectionPoint, mode: SelectionMode);
    pub fn update_selection(&mut self, point: SelectionPoint);
    pub fn end_selection(&mut self);
    pub fn selected_text(&self) -> Option<String>;
    pub fn selection_rects(&self) -> Vec<SelectionRect>;
}
```

`selection_rects()` は現在表示中の viewport に対する visible rect だけ返してください。

## Mouse routing

既存の Phase 3-B / 4-A routing を壊さないでください。

基本ルール:

```text
if mouse_reporting_enabled:
    app へ mouse event を送る
else:
    terminal selection を処理する
```

ただし、Shift を押している場合は、多くの端末と同じように端末側 selection を優先してよいです。

```text
if shift_pressed:
    terminal selection を処理する
else if mouse_reporting_enabled:
    PTY へ mouse event
else:
    terminal selection
```

### primary screen

* mouse drag で selection
* scrollback 表示中でも selection 可能
* wheel scroll と selection drag が競合しないこと

### alternate screen

* mouse reporting enabled なら PTY へ送る
* Shift + drag なら terminal selection
* mouse reporting off なら terminal selection
* alternate screen の selection は screen 内だけでよい
* alternate screen の内容は scrollback に入れない

## Coordinate conversion

Phase 4-A の scroll_offset を考慮してください。

pixel -> visible cell:

```text
visible_row = pixel_to_cell_row(...)
visible_col = pixel_to_cell_col(...)
```

visible cell -> logical selection point:

```text
if scroll_offset == 0:
    logical row = screen row
else:
    logical row = visible viewport が参照している scrollback/screen 合成行
```

この変換 helper を app/core のどちらかに分離し、test してください。

## Text extraction

`selected_text()` は以下を満たしてください。

* 複数行 selection に対応
* wide / wide-spacer を壊さない
* wide-spacer は文字として出力しない
* 行末の不要な空白は基本 trim_end してよい
* 行間は `\n`
* CRLF にはしない
* selection が空なら None
* reversed selection でも同じ結果を返す

例:

```text
hello world
abc def
```

`hello` から `abc` まで選択した場合:

```text
hello world
abc
```

## Selection rendering

`RenderPlan` に selection rect を追加してください。

想定:

```rust
pub struct RenderPlan {
    // existing fields
    pub selection_rects: Vec<RectSpan>,
}
```

描画順:

```text
background
selection background
underline
text
cursor
```

または、現在の実装で自然な順序にしてください。
ただし selection が text より前に描画され、文字が読めること。

selection 色はまず config 固定値か theme default で構いません。
高度な theme system は後回しでよいです。

## Clipboard copy

Ctrl+Shift+C で `Terminal::selected_text()` を取得し、OS clipboard へ書き込んでください。

* clipboard crate は Phase 3-B の paste integration で追加済みのものを使う
* selection が空なら何もしない
* copy 成功後に selection を保持する
* copy 失敗時に panic しない
* Ctrl+C と競合しないこと

  * Ctrl+C は shell へ SIGINT 相当として送る
  * Ctrl+Shift+C は copy

## Double click / triple click

最小実装で構いません。

### double click

* word selection
* word boundary は最初は ASCII 寄りでよい
* `[A-Za-z0-9_./:-]` あたりを word constituent として扱う
* 日本語 word segmentation は後回し

### triple click

* line selection
* 表示行全体を選択
* 行末空白は selected_text 時に trim_end してよい

click count を winit event から取れない場合は、app 側で時間差と位置で簡易判定してください。

## Tests

core unit test を優先してください。

最低限追加する test:

1. single-line selection の selected_text
2. multi-line selection の selected_text
3. reversed selection の selected_text
4. wide / wide-spacer が selected_text に重複出力されない
5. scrollback 行を含む selection
6. scroll_offset > 0 の selection_rects が visible rect だけ返す
7. selection clear
8. double click word selection
9. triple click line selection
10. selection 中に新規出力が来た場合の扱い

    * まずは selection clear でよい
11. alternate screen では primary scrollback を選択しない
12. resize 後 selection が安全に clamp または clear される

app helper test:

1. primary screen + mouse reporting off では drag が selection action
2. mouse reporting on + no shift では PTY mouse action
3. mouse reporting on + shift では selection action
4. Ctrl+Shift+C 判定
5. Ctrl+C は copy shortcut と判定されない
6. double click 判定
7. triple click 判定

render test がある場合:

1. selection_rects が RenderPlan に反映される
2. selection rect が cursor/text と座標ズレしない

## Manual check

実装後に以下を実行してください。

```bash
cargo fmt --all
cargo test --workspace
cargo build --workspace
cargo clippy --workspace --all-targets -- -D warnings
git diff --check
```

GUI で確認:

```text
seq 1 200
# または Windows なら長い dir /s

mouse wheel で scrollback
drag selection
Ctrl+Shift+C
別アプリへ paste

nvim
:set mouse=a
# 通常 click は nvim へ渡る
# Shift+drag では terminal selection になる

less Cargo.toml
# less 中の mouse/wheel 挙動が壊れない
```

期待:

* 通常 shell で drag selection が見える
* scrollback 上の文字も選択できる
* Ctrl+Shift+C でコピーできる
* Ctrl+C は shell interrupt として残る
* nvim mouse reporting を壊さない
* Shift+drag で terminal selection が可能
* wide 文字が二重コピーされない
* cargo test/build/clippy が通る

## コミット方針

小さく分けてください。

例:

```text
core: add selection state and text extraction
core: map scrollback viewport selection rects
render: draw selection rectangles
app: route drag selection and copy shortcut
app: add click selection helpers
test: cover phase4 selection behavior
```

## 完了条件

* cargo fmt --all 成功
* cargo test --workspace 成功
* cargo build --workspace 成功
* cargo clippy --workspace --all-targets -- -D warnings 成功
* git diff --check 成功
* 通常画面で drag selection できる
* scrollback 上の選択ができる
* Ctrl+Shift+C で OS clipboard にコピーできる
* nvim / less の mouse reporting を壊していない
* OSC 8 / OSC 52 / search は未実装のまま TODO


---

# その次

Phase 4-B が通ったら、次は **OSC 0/2 title** を先に入れてから **OSC 8 hyperlink** がよいです。

```text
Phase 4-B: selection + copy
Phase 4-C: OSC 0/2 window title
Phase 4-D: OSC 8 hyperlink
Phase 5-A: font fallback
Phase 5-B: Unicode width / ambiguous width
Phase 5-C: IME
````

Alacritty は「既存アプリと統合し、再実装しすぎず、高性能にする」方針を掲げています。Knightty も今は同じく、mux や shell integration に広げるより、端末の基礎 UX を固める段階です。([github.com][2])

[1]: https://wezterm.org/index.html "WezTerm - Wez's Terminal Emulator"
[2]: https://github.com/alacritty/alacritty "GitHub - alacritty/alacritty: A cross-platform, OpenGL terminal emulator. · GitHub"
