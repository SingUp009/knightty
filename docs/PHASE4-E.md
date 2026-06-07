Knightty の次フェーズとして、Phase 4-E: OSC 8 hover / Ctrl+Click open を実装してください。

## 現在の状態

Phase 4-D で OSC 8 hyperlink metadata は実装済みです。

実装済み:
- Knightty-owned Hyperlink metadata
- OSC 8 parsing
- snapshot hyperlink interning
- per-cell hyperlink indexes
- hyperlink_at_cell() helpers
- RenderPlan hyperlink spans
- contiguous hyperlink spans generation
- URL opening は未実装

検証済み:
- cargo fmt --all
- cargo test --workspace
- cargo build --workspace
- cargo clippy --workspace --all-targets -- -D warnings
- git diff --check

未実施:
- GUI smoke check

未追跡 phase docs は勝手に削除・変更・stage しないでください。

## 今回の目的

OSC 8 hyperlink metadata を使って、hover 表示と Ctrl+Click open を実装してください。

今回のスコープ:

1. hyperlink hover state
2. mouse move による hyperlink hit-test
3. hover 中の cursor icon 変更
4. hover 中の hyperlink underline / visual feedback
5. Ctrl+Click で URL open
6. URL scheme allowlist
7. config に hyperlink open 設定追加
8. unit tests / helper tests

以下は今回実装しないでください。

- 通常 click で URL open
- URL auto-detection
- OSC 52 clipboard
- tooltip preview
- context menu
- visited link color
- terminal bell / notification
- shell integration

## 作業前チェック

まず以下を実行してください。

```bash
git status
git branch --show-current
cargo test --workspace
cargo build --workspace
cargo clippy --workspace --all-targets -- -D warnings
git diff --check
````

可能なら GUI smoke check:

```cmd
powershell -NoProfile -Command "$e=[char]27; [Console]::Write($e+']8;;https://example.com'+$e+'\example link'+$e+']8;;'+$e+'\'+[Environment]::NewLine)"
```

期待:

* `example link` だけ表示される
* OSC sequence が表示されない

## URL open policy

Ctrl+Click のみで URL を開いてください。

許可する scheme:

```text
https
http
```

初期状態では以下は開かないでください。

```text
file
javascript
data
mailto
ssh
ftp
その他 unknown scheme
```

理由:

* terminal output は untrusted とみなす
* CLI tool や remote host が arbitrary OSC 8 を出せる
* 誤爆・危険 scheme open を避ける

必要なら `url` crate を追加し、`Url::parse` で validation してください。

## Config 追加

`config.json` に以下を追加してください。

```json
{
  "hyperlink": {
    "open_on_ctrl_click": true,
    "allowed_schemes": ["https", "http"],
    "underline_on_hover": true
  }
}
```

default:

```text
open_on_ctrl_click = true
allowed_schemes = ["https", "http"]
underline_on_hover = true
```

validation:

* allowed_schemes は lowercase に normalize
* empty allowed_schemes なら open 無効
* unknown config field は既存方針に合わせる

## Core API 案

既存の `hyperlink_at_cell()` を活かしてください。

必要に応じて追加:

```rust
impl Terminal {
    pub fn hovered_hyperlink(&self) -> Option<&Hyperlink>;
    pub fn set_hovered_cell(&mut self, col: usize, row: usize);
    pub fn clear_hovered_hyperlink(&mut self);
}
```

ただし hover state は app 側に置いても構いません。

推奨:

* hyperlink metadata: core
* hover coordinate / cursor icon / open action: app
* hover underline span: render plan または app -> render option

## App routing

mouse move:

```text
if cell has hyperlink:
    set hover state
    window cursor = pointer/hand if supported
else:
    clear hover
    window cursor = default/text
```

mouse click:

```text
if left button pressed && ctrl_pressed && cell has hyperlink:
    validate URL
    open URL
    do not send mouse event to PTY
else:
    existing mouse reporting / selection routing
```

注意:

* mouse reporting enabled の TUI 上では、Ctrl+Click hyperlink を優先するか PTY を優先するか決める
* 今回は `Ctrl+Click hyperlink` を優先してよい
* 通常 click は既存 routing を壊さない
* drag selection 中は open しない
* selection が active なら open しない
* scrollback 表示中の hyperlink も開けること
* alternate screen 内の hyperlink も metadata があれば開けること

## Render / visual feedback

hover 中の hyperlink は最小で underline してください。

既存 underline rect pass を使えるなら流用してください。

期待:

* hover hyperlink の visible span だけ underline
* text color は変えなくてよい
* selection と競合しない
* selection 中は hover underline を出さなくてもよい
* RenderPlan に `hovered_hyperlink_id` または hover rects を渡す設計でよい

## URL opening

候補:

* `open` crate の `open::that(url)`
* 既に別の crate があるならそれを使う

注意:

* open result は握りつぶさず log する
* 失敗しても panic しない
* open operation が重い可能性があるので、UI thread blocking が目立つなら TODO にする
* 最初は同期呼び出しでもよいが、将来非同期化 TODO を残す

## Tests

core / app helper tests を追加してください。

最低限:

### core

1. hyperlink_at_cell が hyperlink を返す
2. hyperlink_at_cell が non-link cell で None
3. scroll_offset > 0 でも hyperlink hit-test が正しい
4. hyperlink metadata は selection copy text に混ざらない

### config

1. default allowed_schemes が https/http
2. allowed_schemes が lowercase normalize される
3. empty allowed_schemes で open disabled 相当になる

### app helper

1. https URL は allowed
2. http URL は allowed
3. file URL は rejected
4. javascript URL は rejected
5. invalid URL は rejected
6. Ctrl+Click + hyperlink cell で open action
7. Click without Ctrl では open しない
8. drag 中は open しない
9. selection active 中は open しない
10. mouse reporting enabled でも Ctrl+Click hyperlink は open action になる
11. hyperlink hover で cursor state が pointer になる
12. non-link hover で cursor state が default/text になる

### render

1. hovered hyperlink span から underline rect が生成される
2. selection rect と hover underline が共存する
3. scrollback 表示中の hovered hyperlink が正しい visible rect になる

## Manual check

実装後:

```bash
cargo fmt --all
cargo test --workspace
cargo build --workspace
cargo clippy --workspace --all-targets -- -D warnings
git diff --check
```

GUI / Windows PowerShell OSC 8 regression:

```cmd
powershell -NoProfile -Command "$e=[char]27; [Console]::Write($e+']8;;https://example.com'+$e+'\example link'+$e+']8;;'+$e+'\'+[Environment]::NewLine)"
```

確認:

```text
- valid HTTPS link 上で hover すると cursor が pointer/hand になる
- valid HTTPS link 上で hover underline が出る
- 通常 click では開かない
- Ctrl+left click press/release が同じ link 上なら browser が開く
- Ctrl+left click を消費した場合、その mouse event は PTY に送られない
- link 文字列上を drag すると browser は開かず、terminal text selection になる
- link 文字列を選択して Ctrl+Shift+C すると表示 text だけが clipboard に入り、URI は混ざらない
- clipboard text を Ctrl+Shift+V で PTY に paste できる
- selection が残っている状態で通常入力または paste すると selection が消える
- scrollback 上の link も Ctrl+left click で開く
- nvim / less の mouse reporting を壊さない
```

危険 scheme の確認:

```cmd
powershell -NoProfile -Command "$e=[char]27; [Console]::Write($e+']8;;file:///C:/Windows/win.ini'+$e+'\file link'+$e+']8;;'+$e+'\'+[Environment]::NewLine)"
```

期待:

* 表示はされる
* Ctrl+Click しても開かない
* `knightty hyperlink: rejected ... scheme` のような拒否ログが出る

## コミット方針

小さく分けてください。

```text
config: add hyperlink open settings
app: add hyperlink hover hit testing
render: underline hovered hyperlinks
app: open allowed hyperlinks on ctrl click
test: cover hyperlink hover and open policy
```

## 完了条件

* cargo fmt --all 成功
* cargo test --workspace 成功
* cargo build --workspace 成功
* cargo clippy --workspace --all-targets -- -D warnings 成功
* git diff --check 成功
* hover で hyperlink が視覚的に分かる
* Ctrl+Click で http/https のみ開く
* file/javascript/data は開かない
* selection / scrollback / mouse reporting を壊していない
* 通常 click では開かない

OSC 52 は clipboard を terminal output から操作できるため便利ですが、**必ず opt-in / size limit / read/write policy** を入れてからにしたほうがよいです。

[1]: https://xtermjs.org/docs/guides/link-handling/?utm_source=chatgpt.com "Link Handling"
[2]: https://iterm2.com/feature-reporting/Hyperlinks_in_Terminal_Emulators.html?utm_source=chatgpt.com "Hyperlinks (aka HTML-like anchors) in terminal emulators"
[3]: https://docs.rs/open?utm_source=chatgpt.com "open - Rust"
[4]: https://docs.rs/url?utm_source=chatgpt.com "url - Rust"
