Knightty の次フェーズとして、Phase 4-A: scrollback buffer を実装してください。

## 現在の状態

Phase 3-A / 3-B は実装済みです。

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
- core/app unit tests
- clippy 対応

検証済み:
- cargo fmt --all
- cargo test --workspace
- cargo build --workspace
- cargo clippy --workspace --all-targets -- -D warnings
- git diff --check

未確認:
- GUI での `nvim :set mouse=a` 手動確認

## 作業前の必須確認

まず以下を実行してください。

```bash
git status
git branch --show-current
cargo test --workspace
cargo build --workspace
````

未追跡の `docs/PHASE3-B.md` がある場合、勝手に削除・変更・add しないでください。
必要なら内容を確認し、今回の scrollback 実装とは別コミットにしてください。

可能なら作業前に GUI で以下を確認してください。

```text
nvim
:set mouse=a
```

確認:

* click で cursor が動く
* wheel が nvim 内で動く
* Ctrl+Shift+V で paste できる
* nvim 終了後に primary screen が壊れない

## 今回の目的

primary screen に scrollback buffer を追加し、通常 shell 出力を mouse wheel / keyboard shortcut で遡れるようにしてください。

今回のスコープは以下に限定します。

1. primary screen 用 scrollback buffer
2. scroll offset state
3. wheel scroll routing
4. keyboard scroll shortcut
5. RenderPlan / GridSnapshot へ scrollback 表示を接続
6. config に scrollback_lines を追加
7. unit tests

以下は今回実装しないでください。

* text selection
* copy
* search
* OSC 8 hyperlink
* OSC 52 clipboard
* scrollback pager
* reflow scrollback
* tabs / panes
* kitty graphics / sixel

selection/copy は Phase 4-B に回します。

## 設計方針

現在の `Terminal` / `GridSnapshot` / `RenderPlan` の境界を保ってください。

描画層に `alacritty_terminal` の型を漏らさないでください。
外側へ出すのは Knightty 独自型だけにしてください。

想定構造:

```rust
pub struct Terminal {
    // existing fields

    scrollback: ScrollbackBuffer,
    scroll_offset: usize,
    scrollback_limit: usize,
}
```

または `alacritty_terminal` 側に既に history/grid scrollback がある場合は、それを調査したうえで、Knightty の public API として薄く包んでください。

ただし、renderer/app へは `GridSnapshot` として渡してください。

## Scrollback の基本仕様

### primary screen

* 通常 screen で上に流れた行を scrollback buffer に積む
* scrollback limit を超えた古い行は破棄する
* `scrollback_lines = 0` なら scrollback 無効
* 新しい PTY output が来たら、原則 `scroll_offset = 0` に戻す

  * ただしユーザーが scrollback 閲覧中の場合の挙動は TODO でもよい
* prompt へ入力したときも原則 bottom へ戻す

### alternate screen

* alternate screen の内容は scrollback に積まない
* alternate screen 中の wheel は、mouse reporting が enabled ならアプリへ送る
* alternate screen 中で mouse reporting が off の場合の wheel は今回は no-op でよい
* nvim / less / fzf 終了後に primary screen + scrollback が残ること

### scroll region

* scroll region 内のスクロールを全て scrollback に積むと壊れる可能性がある
* まずは full-screen primary scroll のみ scrollback に入れる
* scroll region が full screen でない場合は scrollback 追加対象外でよい
* この仕様を test と TODO に明記する

## 表示モデル

`GridSnapshot` 生成時に `scroll_offset` を反映してください。

期待:

* `scroll_offset = 0`: 現在の active screen を表示
* `scroll_offset > 0`: scrollback の過去行 + screen の一部を合成して表示
* cursor は scrollback 閲覧中には非表示、または現在位置のまま表示しない
* selection は未実装なので不要
* scrollback 表示中も wide / wide-spacer 情報を壊さない

例:

```text
scrollback:
  [old1]
  [old2]
  [old3]

screen rows = 3:
  [cur1]
  [cur2]
  [cur3]

scroll_offset = 0:
  [cur1]
  [cur2]
  [cur3]

scroll_offset = 2:
  [old2]
  [old3]
  [cur1]
```

実際の合成は rows 数に合わせて調整してください。

## API 追加案

必要に応じて以下を追加してください。

```rust
impl Terminal {
    pub fn scrollback_len(&self) -> usize;
    pub fn scroll_offset(&self) -> usize;
    pub fn scroll_up_lines(&mut self, lines: usize) -> Damage;
    pub fn scroll_down_lines(&mut self, lines: usize) -> Damage;
    pub fn scroll_to_top(&mut self) -> Damage;
    pub fn scroll_to_bottom(&mut self) -> Damage;
    pub fn is_scrolled_back(&self) -> bool;
}
```

`Damage` は基本的に full damage で構いません。
最適化は後回しにしてください。

## Config 追加

`knightty.config` に以下を追加してください。

```toml
[terminal]
scrollback_lines = 10000
scroll_multiplier = 3
```

既存 config がない場合の default:

```text
scrollback_lines = 10000
scroll_multiplier = 3
```

validation:

* `scrollback_lines`: 0..=100000
* `scroll_multiplier`: 1..=100

既存 config との後方互換性を壊さないでください。

## App 側 wheel routing

winit の MouseWheel routing を次のようにしてください。

```text
if alternate_screen && mouse_reporting_enabled:
    send mouse wheel event to PTY
else if primary_screen:
    terminal scrollback を動かす
else:
    no-op
```

注意:

* Phase 3-B で入れた mouse reporting の挙動を壊さない
* primary screen で mouse reporting enabled の場合は、アプリへ送るべきか scrollback すべきか迷うが、今回は xterm 互換寄りに mouse reporting を優先してよい
* ただし通常 shell では mouse reporting off なので、wheel は scrollback へ行く

## Keyboard shortcut

以下を最小実装してください。

```text
Ctrl+Shift+PageUp   scroll up one page
Ctrl+Shift+PageDown scroll down one page
Ctrl+Shift+Home     scroll to top
Ctrl+Shift+End      scroll to bottom
```

既存 paste shortcut と競合しないようにしてください。

## Tests

core unit test を優先してください。

最低限追加する test:

1. scrollback_lines = 0 のとき履歴が残らない
2. screen height を超える出力で scrollback_len が増える
3. scrollback_limit を超えると古い行が破棄される
4. scroll_up_lines で scroll_offset が増える
5. scroll_down_lines で scroll_offset が減る
6. scroll_to_bottom で scroll_offset = 0 になる
7. scroll_offset > 0 の GridSnapshot が過去行を含む
8. scrollback 表示中は cursor を非表示、または描画対象外にする
9. alternate screen の出力は scrollback に入らない
10. alternate screen から戻っても primary scrollback が残る
11. resize 後に scroll_offset が範囲内に clamp される
12. wide / wide-spacer cell を含む行が scrollback に入っても壊れない

app helper test:

1. primary screen + mouse reporting off の wheel は scrollback action になる
2. alternate screen + mouse reporting on の wheel は PTY mouse event になる
3. Ctrl+Shift+PageUp 判定
4. Ctrl+Shift+PageDown 判定
5. Ctrl+Shift+Home / End 判定

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
cargo test --workspace
# 長い出力を wheel で遡る

seq 1 200
# Linux/WSL 等で確認できる場合

dir /s
# Windows cmd/pwsh で確認できる場合

nvim
:set mouse=a
# alternate screen 中は nvim に wheel/click が渡ること

less Cargo.toml
# less 終了後 primary screen と scrollback が壊れないこと
```

期待:

* 通常 shell 出力を wheel で遡れる
* Ctrl+Shift+PageUp / PageDown で遡れる
* Ctrl+Shift+End で bottom に戻る
* nvim / less / fzf など alternate screen 中の wheel routing が壊れない
* primary screen の内容が alternate screen 終了後も残る
* cargo test/build/clippy が通る

## コミット方針

小さく分けてください。

例:

```text
core: add scrollback buffer state
core: render snapshots from scrollback offset
app: route primary wheel events to scrollback
app: add scrollback keybindings
config: add terminal scrollback options
test: cover phase4 scrollback behavior
```

## 完了条件

* cargo fmt --all 実行済み
* cargo test --workspace 成功
* cargo build --workspace 成功
* cargo clippy --workspace --all-targets -- -D warnings 成功
* git diff --check 成功
* primary screen の scrollback が動く
* alternate screen の mouse/wheel routing が壊れていない
* text selection / copy は未実装のまま TODO として残す

[1]: https://xtermjs.org/docs/api/vtfeatures/?utm_source=chatgpt.com "Supported Terminal Sequences"
[2]: https://alacritty.org/config-alacritty.html?utm_source=chatgpt.com "TOML configuration file format."
[3]: https://sw.kovidgoyal.net/kitty/conf/?utm_source=chatgpt.com "kitty.conf - Kovid's software projects"
[4]: https://xtermjs.org/docs/guides/link-handling/?utm_source=chatgpt.com "Link Handling"
