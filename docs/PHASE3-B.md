Knightty の次フェーズとして、Phase 3-B: mouse / focus / paste integration を実装してください。

## 現在の状態

ブランチ:

- codex/phase3-terminal-compat

作成済みコミット:

- 431afa9 chore: baseline project state
- 6156273 core: support phase3 terminal modes
- ac868cc app: use live terminal size for pty

検証済み:

- cargo test -p core passed
- cargo test -p app passed
- cargo test --workspace passed
- cargo build --workspace passed
- GUI 起動成功
- LazyVim 起動確認済み

Phase 3-A では以下が入っています。

- DEC 1047/1048/1049 系の alternate screen 互換
- bracketed paste API
- PTY 起動サイズ / resize 伝播修正
- DECSTBM / DECOM / IL / DL / ICH / DCH 系のテスト
- render 層は大きく触っていない

## 今回の目的

Knightty 上で nvim / LazyVim / less / fzf / lazygit などの TUI を、キーボードだけでなく mouse / focus / paste でも破綻せず使えるようにしてください。

今回のスコープは以下に限定します。

1. OS clipboard paste を bracketed paste API に接続する
2. xterm SGR mouse reporting を実装する
3. focus in/out reporting を実装する
4. app 側の winit mouse/focus event を terminal cell 座標へ変換する
5. mouse event routing の基礎を作る

scrollback / selection / copy / OSC 8 / OSC 52 / kitty graphics / tabs / panes は今回実装しないでください。

## 参照仕様の方針

xterm 系の control sequences を基準にしてください。

特に以下を扱います。

- CSI ? 1000 h/l: button press/release mouse reporting
- CSI ? 1002 h/l: button event mouse reporting
- CSI ? 1003 h/l: any event mouse reporting
- CSI ? 1004 h/l: focus in/out reporting
- CSI ? 1006 h/l: SGR mouse mode
- wheel up/down reporting
- focus in: ESC [ I
- focus out: ESC [ O

1005 UTF-8 mouse mode と 1015 urxvt mouse mode は今回は実装しなくてよいです。
SGR mouse mode、つまり 1006 を優先してください。

## 作業前チェック

まず以下を確認してください。

```bash
git status
git branch --show-current
cargo test --workspace
cargo build --workspace
````

既存のユーザー変更・未追跡ファイルを勝手に消さないでください。

作業ブランチは現在の `codex/phase3-terminal-compat` から継続してもよいですが、必要なら以下を切ってください。

```bash
git switch -c codex/phase3b-input-mouse
```

## 実装対象

### 1. Terminal core に mouse/focus mode state を追加

`crates/core` に、DEC private mode の状態として以下を持たせてください。

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseProtocol {
    Off,
    X10,
    Normal,
    ButtonMotion,
    AnyMotion,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseEncoding {
    Default,
    Sgr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MouseModes {
    pub protocol: MouseProtocol,
    pub encoding: MouseEncoding,
    pub focus_events: bool,
}
```

既存の alacritty_terminal が mode state を保持している場合は、二重管理にならないように注意してください。
ただし app 側が「現在 mouse reporting が有効か」を判断できる public API は必要です。

追加 API 例:

```rust
impl Terminal {
    pub fn mouse_modes(&self) -> MouseModes;
    pub fn focus_events_enabled(&self) -> bool;
    pub fn encode_mouse_event(&self, event: TerminalMouseEvent) -> Option<Vec<u8>>;
    pub fn encode_focus_event(&self, focused: bool) -> Option<Vec<u8>>;
}
```

### 2. DECSET/DECRST の mode handling

以下を feed 経路で検出・反映してください。

* `CSI ? 1000 h/l`
* `CSI ? 1002 h/l`
* `CSI ? 1003 h/l`
* `CSI ? 1004 h/l`
* `CSI ? 1006 h/l`

期待挙動:

* 1000 enabled: button press/release を報告
* 1002 enabled: button press/release + drag/motion during button を報告
* 1003 enabled: any mouse motion を報告
* 1004 enabled: focus in/out を報告
* 1006 enabled: SGR mouse encoding を使う
* 1006 disabled: default encoding に戻す
* 1000/1002/1003 は互いに優先順位を持たせる

  * 1003 > 1002 > 1000 > off
* reset された mode が現在 protocol と一致する場合は適切に fallback する
* 1005 / 1015 / 1016 は TODO か unsupported として安全に無視する

### 3. SGR mouse encoding

SGR mouse mode が enabled の場合、以下形式で PTY へ送る byte sequence を生成してください。

```text
ESC [ < Cb ; Cx ; Cy M
ESC [ < Cb ; Cx ; Cy m
```

仕様:

* `M`: press / motion / wheel
* `m`: release
* `Cx`, `Cy` は 1-based cell coordinate
* button:

  * left press: 0
  * middle press: 1
  * right press: 2
  * release: same button code with final `m`
  * wheel up: 64
  * wheel down: 65
  * motion: base button code + 32
* modifiers:

  * Shift: +4
  * Alt: +8
  * Ctrl: +16

必要であれば以下のような型を導入してください。

```rust
pub enum MouseButton {
    Left,
    Middle,
    Right,
    WheelUp,
    WheelDown,
    Other(u8),
}

pub enum MouseEventKind {
    Press,
    Release,
    Move,
    Drag,
    Wheel,
}

pub struct TerminalMouseEvent {
    pub kind: MouseEventKind,
    pub button: Option<MouseButton>,
    pub col: usize, // 0-based internal
    pub row: usize, // 0-based internal
    pub shift: bool,
    pub alt: bool,
    pub ctrl: bool,
}
```

### 4. Default mouse encoding は最小対応

1006 disabled の legacy encoding は、今回は最小対応で構いません。

ただし、方針を明確にしてください。

* SGR mode が有効なら完全対応
* SGR mode が無効なら X10/default encoding を最小対応
* 255 を超える座標問題は TODO として明記
* 現代 TUI では 1006 を優先する前提でよい

### 5. Focus event reporting

`CSI ? 1004 h` が有効なときだけ、winit の focused/unfocused event から以下を PTY へ送ってください。

```text
focus in:  ESC [ I
focus out: ESC [ O
```

追加 API 例:

```rust
pub fn encode_focus_event(&self, focused: bool) -> Option<Vec<u8>>;
```

### 6. App 側 event routing

`crates/app/src/main.rs` 側で、winit の mouse/focus events を PTY input へ流す経路を追加してください。

必要な変換:

* window pixel position -> terminal cell position
* padding / viewport origin がある場合は差し引く
* cell width / cell height で割る
* 範囲外 mouse event は無視
* col/row は内部 0-based、encode 時に 1-based に変換

実装対象イベント:

* MouseInput pressed/released
* CursorMoved
* MouseWheel
* WindowEvent::Focused

注意:

* mouse reporting が off のとき、mouse input は PTY へ送らない
* wheel は、alternate screen かつ mouse reporting enabled の場合は app へ送る
* primary screen で mouse reporting off の wheel は、今回は何もしない
* scrollback は次フェーズで実装するため、今回は TODO コメントにする

### 7. OS clipboard paste integration

前回追加した `Terminal::paste_bytes(&self, bytes: &[u8]) -> Vec<u8>` を app 側の paste 操作へ接続してください。

想定:

* Ctrl+Shift+V
* Shift+Insert
* 可能なら platform default paste shortcut

  * Windows/Linux: Ctrl+Shift+V
  * macOS は現時点で対象外なら TODO

実装方針:

* clipboard crate は既存依存を確認する
* 未導入なら `arboard` など軽量な crate を検討する
* clipboard 読み取り失敗時は panic しない
* bracketed paste mode が on の場合は `ESC [ 200 ~` / `ESC [ 201 ~` で包まれること
* off の場合は生 bytes を PTY へ送ること
* 改行正規化は過剰に行わない
* NUL など危険な制御文字をどう扱うかは TODO または最小 sanitize とする

### 8. Tests

可能な限り core unit test を優先してください。

最低限追加する test:

#### core

1. `CSI ? 1006 h` で SGR mouse mode が有効になる
2. `CSI ? 1006 l` で SGR mouse mode が無効になる
3. `CSI ? 1000 h` で normal mouse reporting が有効になる
4. `CSI ? 1002 h` が 1000 より優先される
5. `CSI ? 1003 h` が 1002 より優先される
6. `CSI ? 1004 h/l` で focus event state が変わる
7. SGR left press が `ESC[<0;x;yM` になる
8. SGR left release が `ESC[<0;x;ym` になる
9. SGR wheel up/down が `64` / `65` になる
10. Shift/Alt/Ctrl modifier が button code に反映される
11. focus in/out が enabled のときだけ `ESC[I` / `ESC[O` を返す
12. mouse reporting off のとき encode_mouse_event が None を返す

#### app

可能なら helper を分離して test してください。

1. pixel position から cell coordinate への変換
2. padding がある場合の coordinate 変換
3. 範囲外座標を None にする
4. Ctrl+Shift+V 判定 helper
5. Shift+Insert 判定 helper

### 9. Manual check

実装後、以下を実行してください。

```bash
cargo fmt --all
cargo test --workspace
cargo build --workspace
```

可能なら追加で:

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

GUI で確認:

```text
nvim
:set mouse=a
```

確認項目:

* nvim / LazyVim で mouse click に反応する
* split/window 移動や cursor 移動が mouse で大きく破綻しない
* wheel が alternate screen 内でアプリに送られる
* focus in/out で画面が壊れない
* Ctrl+Shift+V で paste できる
* bracketed paste mode 中の paste で nvim が壊れない
* less / fzf / lazygit で起動終了後に primary screen が復元される
* cargo test / build が通る

## コミット方針

小さく分けてください。

例:

```text
core: add mouse and focus mode state
core: encode sgr mouse and focus events
app: route winit mouse events to pty
app: wire clipboard paste through terminal paste api
test: cover phase3b mouse and paste behavior
```

## 完了条件

* cargo fmt --all 実行済み
* cargo test --workspace 成功
* cargo build --workspace 成功
* 可能なら cargo clippy --workspace --all-targets -- -D warnings 成功
* core に mouse/focus/paste の unit test がある
* app に coordinate conversion / paste shortcut の test がある
* nvim / LazyVim で mouse 操作が最低限動く
* scrollback / selection は未実装のまま TODO として残す

````

---

## この次のフェーズ

Phase 3-B が通ったら、次は **Phase 4: scrollback / selection / copy** です。

WezTerm も現代ターミナルの代表機能として native mouse / scrollback、font fallback、hyperlinks などを掲げています。:contentReference[oaicite:2]{index=2}  
なので、順番は次が最も自然です。

```text
Phase 3-A: alternate screen / resize / scroll region / bracketed paste  ← 完了
Phase 3-B: mouse / focus / paste integration                            ← 次
Phase 4-A: scrollback buffer
Phase 4-B: selection + copy
Phase 4-C: OSC 0/2 title + OSC 8 hyperlink
````

Windows 側は引き続き ConPTY の deadlock に注意が必要です。Microsoft は ConPTY の入出力チャネルを個別 thread で処理すること、resize は文字セル単位で `ResizePseudoConsole` に伝えることを示しています。([learn.microsoft.com][2])

[1]: https://xtermjs.org/docs/api/vtfeatures/ "Supported Terminal Sequences"
[2]: https://learn.microsoft.com/en-us/windows/console/creating-a-pseudoconsole-session "Creating a Pseudoconsole session - Windows Console | Microsoft Learn"
