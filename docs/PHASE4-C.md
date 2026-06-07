Knightty の次フェーズとして、Phase 4-C: OSC 0/2 window title を実装してください。

## 現在の状態

Phase 3-A / 3-B / 4-A / 4-B は実装済みです。

Phase 3-A:
- DEC 1047/1048/1049 系 alternate screen 互換
- resize propagation
- scroll region / origin mode
- IL / DL / ICH / DCH
- bracketed paste API

Phase 3-B:
- DEC 1000/1002/1003/1004/1006 の mode state
- SGR mouse encoding
- focus in/out encoding
- winit mouse/focus event の PTY routing
- clipboard paste integration

Phase 4-A:
- primary screen 用 scrollback buffer
- scroll_offset state
- wheel/key scroll routing
- config terminal.scrollback_lines / scroll_multiplier

Phase 4-B:
- selection state
- selected_text extraction
- selection visible rect generation
- mouse drag selection routing
- Ctrl+Shift+C copy
- double/triple click
- RenderPlan selection background

検証済み:
- cargo fmt --all
- cargo test --workspace
- cargo build --workspace
- cargo clippy --workspace --all-targets -- -D warnings
- git diff --check

未確認:
- Phase 4-B の GUI 手動確認

## 作業前の必須確認

まず以下を実行してください。

```bash
git status
git branch --show-current
cargo test --workspace
cargo build --workspace
cargo clippy --workspace --all-targets -- -D warnings
git diff --check
````

未追跡の docs/PHASE3-B.md / docs/PHASE4-A.md / docs/PHASE4-B.md がある場合、勝手に削除・変更・stage しないでください。

可能なら作業前に GUI で Phase 4-B smoke test をしてください。

```text
seq 1 200
drag selection
Ctrl+Shift+C
nvim
:set mouse=a
Shift+drag
```

## 今回の目的

OSC 0 / OSC 2 による window title update を実装してください。

今回のスコープは以下に限定します。

1. OSC 0 / OSC 2 parsing
2. terminal window title state
3. app 側 winit window title 反映
4. title changed damage/event 通知
5. unit tests

以下は今回実装しないでください。

* OSC 1 icon title
* OSC 4 palette
* OSC 8 hyperlink
* OSC 52 clipboard
* shell integration OSC 133
* clickable URL detection
* tab title
* title stack push/pop

## 対応する escape sequence

以下を扱ってください。

```text
OSC 0 ; <title> BEL
OSC 0 ; <title> ST
OSC 2 ; <title> BEL
OSC 2 ; <title> ST
```

ここで:

```text
OSC = ESC ]
BEL = 0x07
ST  = ESC \
```

期待挙動:

* OSC 0 は window title として扱う
* OSC 2 も window title として扱う
* title は Terminal state に保持する
* app 側で winit window title に反映する
* 空 title は許可する
* 不正または未対応 OSC は安全に無視する
* split feed されても壊れない
* title 更新で grid 内容を破壊しない
* RenderPlan には title を混ぜない

## Core API 案

必要に応じて以下を追加してください。

```rust
impl Terminal {
    pub fn window_title(&self) -> &str;
    pub fn take_window_title_changed(&mut self) -> Option<String>;
}
```

または既存の Damage/Event に統合してください。

例:

```rust
pub enum TerminalEvent {
    Damage(Damage),
    WindowTitleChanged(String),
}
```

ただし、今回の変更で既存 API を大きく壊さないでください。
最小差分を優先してください。

## Parser 方針

既に `alacritty_terminal` が OSC title を処理している場合は、それを調査してください。

方針:

1. 既存 engine から title を取得できるなら、それを薄く wrap する
2. 取得できない、または public API と噛み合わない場合のみ、Knightty core 側で OSC 0/2 を小さく buffer して処理する
3. CSI private mode の既存 split-feed 対応と同じ思想で実装する

注意:

* OSC payload は巨大化しうるので、buffer 上限を設けてください
* 例: 4096 bytes または 8192 bytes
* 上限超過時は OSC sequence を破棄して安全に復帰してください
* UTF-8 不正 bytes は lossless でなくてもよいので、`String::from_utf8_lossy` 相当で安全に処理してください
* BEL / ST の両方で終端してください

## Sanitization

window title に制御文字をそのまま入れないでください。

最小 sanitize:

* `\x00` は削除
* C0 control は `\t` 以外削除、または全削除でよい
* 長すぎる title は truncate
* 最大 1024 chars 程度でよい

## App 側

PTY output feed 後に title changed を確認し、winit window title を更新してください。

期待 title:

```text
<title> - knightty
```

または config で prefix/suffix を変えられるようにする必要はありません。
まずは固定で構いません。

例:

```rust
if let Some(title) = terminal.take_window_title_changed() {
    window.set_title(&format!("{title} - knightty"));
}
```

空 title の場合:

```text
knightty
```

に戻してよいです。

## Tests

core unit test を優先してください。

最低限追加する test:

1. `OSC 0 ; hello BEL` で window_title が `hello` になる
2. `OSC 2 ; hello BEL` で window_title が `hello` になる
3. `OSC 0 ; hello ST` で window_title が `hello` になる
4. split feed でも title が更新される
5. unsupported OSC は title を変えない
6. empty title を扱える
7. title update が grid contents を壊さない
8. control chars が sanitize される
9. oversized OSC payload は安全に破棄または truncate される
10. `take_window_title_changed()` は一度取得したら None になる

app helper test:

1. empty title -> `knightty`
2. non-empty title -> `<title> - knightty`
3. title sanitization 後の文字列が window title formatter に渡る

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

Linux / WSL / Git Bash / PowerShell など、使える環境で以下を試してください。

```bash
printf '\033]0;hello knightty\007'
printf '\033]2;build logs\007'
printf '\033]0;st title\033\\'
```

Windows PowerShell なら必要に応じて以下を試してください。

```powershell
[Console]::Write("`e]0;hello knightty`a")
```

期待:

* window title が変わる
* grid 上に OSC sequence が表示されない
* selection / scrollback / nvim mouse が壊れない
* cargo test/build/clippy が通る

## コミット方針

小さく分けてください。

例:

```text
core: add window title state for osc 0 and 2
app: apply terminal title to window
test: cover osc title parsing
```

## 完了条件

* cargo fmt --all 成功
* cargo test --workspace 成功
* cargo build --workspace 成功
* cargo clippy --workspace --all-targets -- -D warnings 成功
* git diff --check 成功
* OSC 0 / 2 で window title が更新される
* OSC sequence が terminal grid に表示されない
* selection / scrollback / mouse reporting を壊していない
* OSC 8 / OSC 52 は未実装のまま TODO

OSC 8 は、表示テキストと URL を分離して clickable link にする仕様として WezTerm でもサポートされています。([WezTerm][3])
ただし `selection rect`、`mouse hover/click`、`copy 時の扱い` と絡むので、次の次に回すのが安全です。

[1]: https://wezterm.org/escape-sequences.html?utm_source=chatgpt.com "Escape Sequences - Wez's Terminal Emulator"
[2]: https://xtermjs.org/docs/guides/link-handling/?utm_source=chatgpt.com "Link Handling"
[3]: https://wezterm.org/hyperlinks.html?utm_source=chatgpt.com "Hyperlinks - Wez's Terminal Emulator"
