Knightty の次フェーズとして、Phase 4-D: OSC 8 hyperlink metadata を実装してください。

## 現在の状態

Phase 3-A / 3-B / 4-A / 4-B / 4-B.5 / 4-C は実装済みです。

実装済み:
- alternate screen
- resize propagation
- scroll region / origin mode
- IL / DL / ICH / DCH
- bracketed paste
- mouse reporting
- focus reporting
- clipboard paste
- scrollback
- selection + copy
- color fidelity 修正
- OSC 0 / OSC 2 window title

検証済み:
- cargo fmt --all
- cargo test --workspace
- cargo build --workspace
- cargo clippy --workspace --all-targets -- -D warnings
- git diff --check

未追跡 docs:
- docs/PHASE3-B.md
- docs/PHASE4-A.md
- docs/PHASE4-B.md

これらは勝手に削除・変更・stage しないでください。

## 今回の目的

OSC 8 hyperlink を parsing し、表示 cell に hyperlink metadata を付与できるようにしてください。

今回のスコープは metadata までです。
URL open は実装しないでください。

## 対応 sequence

以下を扱ってください。

```text
OSC 8 ; params ; uri ST
OSC 8 ; params ; uri BEL
OSC 8 ; ; uri ST
OSC 8 ; ; uri BEL
OSC 8 ; ; ST
OSC 8 ; ; BEL
````

ここで:

```text
OSC = ESC ]
ST  = ESC \
BEL = 0x07
```

期待挙動:

* `OSC 8 ; params ; uri` で current hyperlink を開始/更新する
* 以降に出力される visible text cell に hyperlink metadata を付ける
* `OSC 8 ; ;` で current hyperlink を解除する
* hyperlink sequence 自体は grid に表示しない
* split feed に耐える
* unsupported OSC は安全に無視する
* 既存 OSC 0/2 title 実装を壊さない
* selection / copy / scrollback / alternate screen を壊さない

## 仕様方針

OSC 8 の params は最小対応で構いません。

対応:

* `id=<value>` だけ parse
* その他 key=value は保持しなくてよい
* params 全体を raw string として保持してもよい

URI:

* 空 URI は hyperlink close
* 非空 URI は current hyperlink として保持
* URI は sanitize する
* 最大長を設ける

  * 例: 2048 bytes
* params も最大長を設ける

  * 例: 1024 bytes

## Core data model 案

必要に応じて以下のような型を追加してください。

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hyperlink {
    pub id: Option<String>,
    pub uri: String,
}
```

CellSnapshot に hyperlink index または hyperlink metadata を持たせてください。

```rust
pub struct CellSnapshot {
    // existing fields
    pub hyperlink: Option<HyperlinkId>,
}
```

または軽量化のため、snapshot 側では `Option<usize>` を持ち、`GridSnapshot` 側に hyperlink table を持たせてもよいです。

```rust
pub struct GridSnapshot {
    // existing fields
    pub hyperlinks: Vec<Hyperlink>,
}
```

重要:

* renderer/app に `alacritty_terminal` の型を漏らさない
* hyperlink metadata は Knightty 独自型として公開する
* clone コストが大きくなりすぎない構造にする

## Scrollback との関係

scrollback に流れる行にも hyperlink metadata を保持してください。

期待:

* primary screen で hyperlink 付き text が scrollback に入っても metadata が残る
* scrollback 表示中の snapshot にも hyperlink metadata が出る
* alternate screen の hyperlink は alternate screen 内だけでよい
* alternate screen 内容は primary scrollback に入れない

## Selection / copy との関係

今回、copy 時に URI を含める必要はありません。

期待:

* selected_text() は表示文字だけ返す
* hyperlink metadata は copy text に混ぜない
* hyperlink 付き文字を selection しても壊れない
* selection rect と hyperlink metadata が競合しない

## RenderPlan

RenderPlan に hyperlink span か hyperlink rect を追加してください。

例:

```rust
pub struct HyperlinkSpan {
    pub hyperlink_id: usize,
    pub row: usize,
    pub start_col: usize,
    pub end_col: usize,
}
```

または visible rect として:

```rust
pub struct HyperlinkRect {
    pub hyperlink_id: usize,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}
```

今回、描画上の underline / hover effect は必須ではありません。
まずは hit-test 可能な metadata が RenderPlan まで届くことを優先してください。

## App hit-test helper

URL open は実装しません。

ただし、将来の Ctrl+Click 用に helper は追加してください。

```rust
pub fn hyperlink_at_cell(&self, col: usize, row: usize) -> Option<Hyperlink>
```

または RenderPlan / GridSnapshot 側 helper でも構いません。

期待:

* visible viewport 上の cell coordinate から hyperlink を取得できる
* scroll_offset > 0 でも正しい hyperlink を返す
* hyperlink がない cell では None

## Parser 方針

既存の alacritty_terminal が OSC 8 を処理できるか調査してください。

方針:

1. alacritty_terminal から hyperlink metadata を取得できるなら、それを薄く wrap する
2. 取得できない場合は、Knightty core 側で OSC 8 だけ小さく buffer して処理する
3. 既存の OSC 0/2 title 処理と競合しないようにする

注意:

* OSC buffer には上限を設ける
* BEL / ST の両方で終端する
* UTF-8 invalid bytes は安全に lossy decode してよい
* buffer overflow 時は sequence を破棄して復帰する

## Tests

core unit test を優先してください。

最低限追加する test:

1. OSC 8 start の後に出力した text cell に hyperlink が付く
2. OSC 8 close の後に出力した text cell には hyperlink が付かない
3. BEL 終端で動く
4. ST 終端で動く
5. split feed で動く
6. `id=foo` params を parse できる
7. unsupported params でも uri は保持される
8. empty URI は close 扱い
9. hyperlink sequence 自体は grid に表示されない
10. scrollback に流れた hyperlink cell の metadata が残る
11. selection/copy では URI が text に混ざらない
12. OSC 0/2 title と OSC 8 が互いに壊れない
13. oversized OSC 8 payload は安全に破棄または truncate される
14. invalid UTF-8 で panic しない

render/app helper test:

1. hyperlink spans が RenderPlan に反映される
2. hyperlink_at_cell が該当 hyperlink を返す
3. hyperlink_at_cell は hyperlink なし cell で None
4. scroll_offset > 0 でも hit-test が正しい

## Manual check

実装後に以下を実行してください。

```bash
cargo fmt --all
cargo test --workspace
cargo build --workspace
cargo clippy --workspace --all-targets -- -D warnings
git diff --check
```

GUI で以下を試してください。

```bash
printf '\033]8;;https://example.com\033\\example link\033]8;;\033\\\n'
```

期待:

* `example link` だけが表示される
* OSC sequence が表示されない
* grid / selection / copy が壊れない
* 現時点ではクリックで URL を開かなくてよい

追加確認:

```bash
printf '\033]0;title test\007'
printf '\033]8;id=abc;https://example.com\007hello\033]8;;\007\n'
```

期待:

* title update と hyperlink metadata が共存する

## コミット方針

小さく分けてください。

例:

```text
core: track osc8 hyperlink state
core: attach hyperlinks to snapshot cells
render: expose hyperlink spans in render plan
app: add hyperlink hit test helpers
test: cover osc8 hyperlink metadata
```

## 完了条件

* cargo fmt --all 成功
* cargo test --workspace 成功
* cargo build --workspace 成功
* cargo clippy --workspace --all-targets -- -D warnings 成功
* git diff --check 成功
* OSC 8 sequence が grid に表示されない
* hyperlink metadata が cell/snapshot/render plan まで届く
* scrollback 上でも metadata が保持される
* selection/copy が壊れない
* URL open は未実装のまま TODO

OSC 52 は clipboard を外部から操作できるため、便利ですがセキュリティ設定が必要です。先に OSC 8 を metadata として閉じた実装にしてから、クリック動作や OSC 52 の許可設定に進むのが安全です。

[1]: https://wezterm.org/index.html?utm_source=chatgpt.com "WezTerm - Wez's Terminal Emulator"
[2]: https://iterm2.com/3.2/documentation-escape-codes.html?utm_source=chatgpt.com "Proprietary Escape Codes - Documentation"
[3]: https://gist.github.com/egmontkob/eb114294efbcd5adb1944c9f3cb5feda?utm_source=chatgpt.com "Hyperlinks in Terminal Emulators"
