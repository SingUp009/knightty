# PHASE1.md

このドキュメントは、`AGENT.md` を上位方針として、フェーズ1の実装を進めるための具体的な作業指示である。

`AGENT.md` に書かれた技術選定・禁止事項・依存方向は確定事項として扱うこと。
本ドキュメントでは、それらを再検討せず、最初に動く縦切り実装を完成させることを目的とする。

---

## 1. フェーズ1の目的

フェーズ1の目的は、以下を満たす最小実装を作ることである。

> PTY 上で起動したシェルの出力を読み取り、VT core に流し込み、ウィンドウ上に文字として表示できる。

この段階では「高性能なターミナル」を完成させる必要はない。
必要なのは、将来の描画・互換性・最適化を載せられる正しい骨格である。

---

## 2. フェーズ1の完了条件

以下をすべて満たしたらフェーズ1完了とする。

- `cargo build --workspace` が通る
- `cargo test --workspace` が通る
- `cargo run -p app` でウィンドウが開く
- デフォルトシェルが PTY 経由で起動する
- シェルの出力が画面に表示される
- キーボード入力が PTY に送られる
- `echo hello` の結果が画面に表示される
- `ls` / `dir` 相当の大量すぎない出力が表示される
- ウィンドウリサイズ時に PTY と core のサイズが更新される
- `core` のテストは GUI・wgpu・winit に依存しない

---

## 3. このフェーズでやらないこと

フェーズ1では以下を実装しない。

- 高度なグリフアトラス最適化
- リガチャ対応
- Kitty graphics protocol
- Kitty keyboard protocol
- OSC 8 hyperlink
- OSC 133 semantic prompt
- OSC 7 working directory report
- undercurl
- 独自 terminfo
- ネイティブタブ・分割
- 設定 UI
- テーマ UI
- tmux 前提の機能
- スクロールバック UI
- ベンチ最適化
- SIMD パーサ実装
- 独自 VT パーサのフルスクラッチ

これらはフェーズ2以降で扱う。
フェーズ1で重要なのは、将来拡張できる境界を壊さないことである。

---

## 4. ワークスペース構成

Cargo workspace は以下の構成にする。

```txt
crates/
  core/
  pty/
  proto/
  render/
  app/
Cargo.toml
````

依存方向は必ず以下を守る。

```txt
app → {render, pty, proto, core}
render → core
proto → core
pty → core には原則依存しない
core → GUI 非依存
```

`core` に以下を持ち込んではならない。

* `winit`
* `wgpu`
* `glyphon`
* `cosmic-text`
* OS 固有ウィンドウ API
* PTY 実装
* 入力イベント実装

`core` は「バイト列を受け取り、ターミナル状態を更新し、観測可能なグリッド状態を返す」層に限定する。

---

## 5. 実装順序

フェーズ1は以下の順序で進める。

---

### Step 1: Cargo workspace を作る

最初に空の workspace を作る。

作成するクレート:

* `core`
* `pty`
* `proto`
* `render`
* `app`

最初のコミット例:

```txt
chore: create cargo workspace
```

この時点では各クレートは空実装でよい。
ただし `cargo build --workspace` が通ること。

---

### Step 2: `core` の最小 API を作る

`core` に `Terminal` を作る。

想定 API:

```rust
pub struct Terminal {
    // 内部に alacritty_terminal 由来の状態を保持する
}

impl Terminal {
    pub fn new(cols: usize, rows: usize) -> Self;

    pub fn resize(&mut self, cols: usize, rows: usize);

    pub fn feed(&mut self, bytes: &[u8]);

    pub fn snapshot(&self) -> GridSnapshot;

    pub fn take_damage(&mut self) -> Damage;
}
```

`Terminal` は、外部から見て以下の責務を持つ。

* バイト列を受け取る
* VT 状態を更新する
* 現在の可視グリッドを返す
* 変更範囲を返す
* GUI や描画 API には依存しない

---

### Step 3: `GridSnapshot` を作る

`render` が `alacritty_terminal` の内部型に直接依存しないよう、`core` 側で描画用 snapshot を返す。

最小構造:

```rust
pub struct GridSnapshot {
    pub cols: usize,
    pub rows: usize,
    pub cells: Vec<Cell>,
}

impl GridSnapshot {
    pub fn cell(&self, x: usize, y: usize) -> &Cell {
        &self.cells[y * self.cols + x]
    }
}

pub struct Cell {
    pub ch: char,
    pub fg: Color,
    pub bg: Color,
    pub flags: CellFlags,
}
```

最初の `CellFlags` は空でもよい。

```rust
pub struct CellFlags {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub inverse: bool,
}
```

色は最初から enum にしておく。

```rust
pub enum Color {
    DefaultFg,
    DefaultBg,
    Indexed(u8),
    Rgb(u8, u8, u8),
}
```

---

### Step 4: `Damage` を最初から入れる

フェーズ1では damage tracking の精度は粗くてよい。
ただし API と概念は最初から入れる。

最小構造:

```rust
pub enum Damage {
    None,
    Full,
    Lines(Vec<usize>),
    Rects(Vec<Rect>),
}

pub struct Rect {
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
}
```

初期実装では `feed()` 後に常に `Damage::Full` を返してもよい。

禁止事項:

* damage tracking を後回しにして API から消すこと
* `render` 側だけで変更検出する設計にすること
* core の状態変化と damage を分離しすぎること

---

### Step 5: `core` のテストを先に書く

`core` のテストは、実際の利用と同じく `Terminal::feed()` に実バイト列を流す。

最初に書くテスト:

```rust
#[test]
fn plain_ascii_text_is_written_to_grid() {
    let mut term = Terminal::new(80, 24);

    term.feed(b"hello");

    let grid = term.snapshot();
    assert_eq!(grid.cell(0, 0).ch, 'h');
    assert_eq!(grid.cell(1, 0).ch, 'e');
    assert_eq!(grid.cell(2, 0).ch, 'l');
    assert_eq!(grid.cell(3, 0).ch, 'l');
    assert_eq!(grid.cell(4, 0).ch, 'o');
}
```

追加するテスト:

```rust
#[test]
fn newline_moves_cursor_to_next_row() {}

#[test]
fn carriage_return_moves_cursor_to_column_zero() {}

#[test]
fn csi_clear_screen_erases_visible_grid() {}

#[test]
fn csi_truecolor_fg_sets_cell_color() {}

#[test]
fn csi_sequence_split_across_two_feeds_still_parses() {}

#[test]
fn utf8_split_across_two_feeds_still_decodes() {}

#[test]
fn resize_changes_visible_grid_size() {}
```

テスト名は仕様を語る名前にする。
`test_csi` や `test_feed` のような曖昧な名前は禁止する。

---

### Step 6: `pty` を作る

`pty` は `portable-pty` の薄いラッパーにする。

想定 API:

```rust
pub struct PtySession {
    // portable-pty の child/master を保持する
}

impl PtySession {
    pub fn spawn_default_shell(size: PtySize) -> Result<Self, PtyError>;

    pub fn take_reader(&mut self) -> Result<Box<dyn std::io::Read + Send>, PtyError>;

    pub fn take_writer(&mut self) -> Result<Box<dyn std::io::Write + Send>, PtyError>;

    pub fn resize(&mut self, size: PtySize) -> Result<(), PtyError>;
}

pub struct PtySize {
    pub rows: u16,
    pub cols: u16,
    pub pixel_width: u16,
    pub pixel_height: u16,
}
```

`pty` の責務:

* OS ごとの差分を隠す
* デフォルトシェルを起動する
* PTY の reader / writer を提供する
* リサイズを PTY に伝える

`pty` の責務ではないもの:

* VT パース
* グリッド管理
* 描画
* ウィンドウイベント処理
* キーマップ解釈

---

### Step 7: `app` でイベントループを作る

`app` は `winit` を使って全層を束ねる。

`app` の責務:

* ウィンドウを開く
* PTY を起動する
* PTY read thread を spawn する
* 読み取ったバイト列を `core.feed()` に渡す
* 入力イベントを PTY writer に送る
* redraw 時に `render` を呼ぶ
* resize 時に `core` と `pty` と `render` を更新する

イベント定義例:

```rust
enum UserEvent {
    PtyBytes(Vec<u8>),
    RedrawRequested,
}
```

PTY read thread では、PTY から読んだバイト列を即座に redraw しない。
約 4ms 程度でバッチングし、複数の読み取り結果をまとめて `EventLoopProxy` で main thread に送る。

擬似コード:

```rust
spawn(move || {
    let mut pending = Vec::new();
    let mut last_flush = Instant::now();

    loop {
        let n = reader.read(&mut buf)?;
        pending.extend_from_slice(&buf[..n]);

        if last_flush.elapsed() >= Duration::from_millis(4) {
            let bytes = std::mem::take(&mut pending);
            proxy.send_event(UserEvent::PtyBytes(bytes))?;
            last_flush = Instant::now();
        }
    }
});
```

注意:

* PTY イベントごとに redraw しない
* main thread を block しない
* `winit` の event loop は main thread に置く
* `core.feed()` は当面 main thread 側で呼ぶ
* render thread 分離はフェーズ2以降でよい

---

### Step 8: `render` を最小実装する

フェーズ1の `render` は、正しさよりも縦切りを優先する。
ただし API は将来差し替えやすくしておく。

想定 API:

```rust
pub struct Renderer {
    // wgpu device, queue, surface, text renderer など
}

impl Renderer {
    pub fn new(/* surface/window context */) -> Result<Self, RenderError>;

    pub fn resize(&mut self, width: u32, height: u32);

    pub fn render(
        &mut self,
        snapshot: &GridSnapshot,
        damage: &Damage,
    ) -> Result<(), RenderError>;
}
```

フェーズ1では以下でよい。

* ASCII が表示できる
* 等幅セルとして表示する
* 背景色は固定でもよい
* カーソル描画は最低限でよい
* 全画面再描画でもよい

ただし、以下は禁止する。

* `render` から PTY を読む
* `render` が `Terminal` を直接所有する
* `render` が `alacritty_terminal` の内部状態に依存する
* `render` に winit のイベント処理を混ぜる

---

## 6. 入力処理の最小方針

フェーズ1では高度な keyboard protocol は実装しない。

最低限必要な入力:

* 通常文字
* Enter
* Backspace
* Tab
* Ctrl+C
* 矢印キー

最初の変換例:

```txt
Enter      -> \r
Backspace  -> \x7f
Tab        -> \t
Ctrl+C     -> \x03
Up         -> \x1b[A
Down       -> \x1b[B
Right      -> \x1b[C
Left       -> \x1b[D
```

Kitty keyboard protocol はフェーズ3で扱う。
フェーズ1では neovim 完全対応を目指さない。

---

## 7. リサイズ処理

ウィンドウサイズが変わったら、以下を更新する。

1. セルサイズから cols / rows を計算する
2. `core.resize(cols, rows)` を呼ぶ
3. `pty.resize(PtySize)` を呼ぶ
4. `render.resize(pixel_width, pixel_height)` を呼ぶ
5. redraw を要求する

セルサイズは最初は固定値でよい。

例:

```rust
const CELL_WIDTH: u32 = 9;
const CELL_HEIGHT: u32 = 18;
```

将来的にはフォントメトリクスから算出する。

---

## 8. エラー処理方針

プロトタイプ中でも、ライブラリ層で安易に `unwrap()` しない。

許可:

* `main()` 直下の一時的な `expect()`
* テスト内の `unwrap()`
* 明らかに初期化失敗で継続不能な箇所の `expect()`

避ける:

* `core` 内の `unwrap()`
* `pty` 内の IO エラー握りつぶし
* `render` 内の surface error 無視
* thread 内 panic による無言終了

エラー型はクレートごとに定義する。

```rust
pub enum PtyError {
    SpawnFailed(String),
    Io(std::io::Error),
    ResizeFailed(String),
}
```

必要なら `thiserror` を使ってよい。
依存追加時は純 Rust であることを確認する。

---

## 9. コミット単位

以下の粒度でコミットする。

```txt
chore: create cargo workspace
feat(core): add terminal wrapper
test(core): document basic vt behavior
feat(core): expose grid snapshot
feat(core): add coarse damage tracking
feat(pty): spawn default shell
feat(app): connect pty read loop to core
feat(app): forward keyboard input to pty
feat(render): draw terminal snapshot
feat(app): handle window resize
docs: add phase1 smoke test checklist
```

1コミットで複数レイヤを大きく変更しない。
特に `core` と `render` の変更は分ける。

---

## 10. スモークテスト

フェーズ1の手動確認は以下で行う。

Linux:

```bash
cargo run -p app
echo hello
printf '\e[38;2;255;0;0mred\e[0m\n'
ls
pwd
```

Windows:

```powershell
cargo run -p app
echo hello
dir
cd
```

期待結果:

* 文字が表示される
* Enter が効く
* Backspace が効く
* Ctrl+C で実行中コマンドを止められる
* リサイズしてもクラッシュしない
* 大量出力で即座に固まらない

---

## 11. `core` テスト方針

`core` のテストは、ターミナル仕様のドキュメントとして読むことができる形にする。

ルール:

* 実バイト列を `Terminal::feed()` に流す
* 内部状態を直接いじらない
* 1テスト = 1挙動
* テスト名で仕様を説明する
* GUI・wgpu・winit を持ち込まない
* 公開 API には doctest を付ける

良いテスト名:

```rust
plain_ascii_text_is_written_to_grid
newline_moves_cursor_to_next_row
carriage_return_moves_cursor_to_column_zero
csi_truecolor_fg_sets_cell_color
csi_truecolor_split_across_two_feeds_still_parses
utf8_split_across_two_feeds_still_decodes
```

悪いテスト名:

```rust
test_terminal
test_csi
test_feed
parse_works
grid_test
```

---

## 12. フェーズ1での判断基準

迷った場合は以下の優先順位で判断する。

1. 依存方向を壊さない
2. `core` を GUI 非依存に保つ
3. まず動く縦切りを作る
4. テストしやすい API にする
5. 後から最適化できる境界を残す
6. フェーズ2以降の機能を先取りしすぎない

フェーズ1では、性能最適化よりも構造の正しさを優先する。
ただし、大量出力で明らかに固まる設計は禁止する。

---

## 13. 禁止事項

フェーズ1中は以下を禁止する。

* 独自 VT パーサをフルスクラッチする
* C/C++ 依存を安易に追加する
* `core` に `winit` / `wgpu` を入れる
* `render` が PTY を直接読む
* PTY read ごとに redraw する
* debug build の性能値を根拠にする
* テストなしで VT 挙動を追加する
* リガチャを先に実装する
* tmux 前提の設計にする
* `TERM=xterm-256color` 前提で将来設計を固定する

---

## 14. フェーズ1終了時に残してよい負債

以下はフェーズ1では許容する。

* damage が常に `Full`
* render が毎フレーム全画面再描画
* フォント設定が固定
* カーソル描画が簡易
* スクロールバック UI がない
* truecolor の表示が完全ではない
* IME 未対応
* neovim の描画が崩れる
* lazygit の一部キーが効かない
* Windows / Linux の片方で先に動く

ただし、以下は負債として残してはならない。

* `core` が GUI に依存している
* レイヤ間の依存方向が逆転している
* PTY と render が密結合している
* 実バイト列ベースの core テストがない
* `cargo test --workspace` が通らない
* 大量出力で即時 redraw して固まる構造になっている

---

## 15. 次フェーズへの引き継ぎ条件

フェーズ2へ進む前に、以下を確認する。

* `core` の snapshot API が render から使いやすい
* damage API が存在する
* PTY read batching が入っている
* resize 経路が一通り通っている
* render は core の内部型に依存していない
* app に最低限のイベントループがある
* フェーズ1のスモークテストが通る

フェーズ2では以下に進む。

* グリフアトラス
* キャッシュ
* cosmic-text shaping
* damage tracking の精密化
* 同期更新
* 入力遅延の初期計測
