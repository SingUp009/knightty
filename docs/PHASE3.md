Knightty の次フェーズとして、Phase 3-A の端末互換性を実装してください。

## 現在の状態

Phase 1 は完了済みです。

- PTY 起動
- VT core への入力反映
- winit + wgpu + glyphon による GUI 表示
- キーボード入力の PTY 転送

Phase 2 の描画基盤も完了済みです。

- GridSnapshot にカーソル表示状態を追加済み
- wide / wide-spacer セル情報を公開済み
- Damage::Lines を追加済み
- RenderPlan を導入済み
- 前景色、背景色、bold、italic、underline、inverse を反映済み
- 背景、下線、カーソルを wgpu の矩形 pass で描画済み
- 文字描画は glyphon
- セル単位配置に切り替え済み
- wide spacer は文字として描画しない
- Shaping::Advanced に切り替え済み
- liga / clig / dlig / calt は無効化済み
- knightty.config 基盤あり
- Windows 起動時の cmd.exe バナー抑制済み

## 今回の目的

「Knightty 上で nvim / less / fzf / LazyVim を開いて終了しても画面が壊れない」状態に近づけてください。

今回のスコープは以下に限定してください。

1. alternate screen
2. resize propagation
3. scroll region / origin mode
4. insert/delete line
5. insert/delete character
6. bracketed paste

mouse reporting や scrollback は今回の主目的から外し、必要なら TODO として残してください。

## 実装方針

既存の描画基盤、RenderPlan、GridSnapshot、Damage::Lines、glyphon/wgpu パスは大きく壊さないでください。

まず現在の repository 構造を確認し、VT parser / VT core / PTY / app 側の責務分離を把握してください。そのうえで、最小の差分で以下を実装してください。

## 実装対象

### 1. Alternate Screen

以下の DEC private mode を扱えるようにしてください。

- CSI ? 1047 h / l
- CSI ? 1048 h / l
- CSI ? 1049 h / l

期待挙動:

- 通常 screen と alternate screen を分離する
- alternate screen 中は通常 screen の内容を破壊しない
- 1048 は cursor save/restore として扱う
- 1049 は save cursor + alternate screen switch + clear alternate screen 相当として扱う
- nvim / less 終了後に元画面へ戻ること

必要であれば TerminalState に以下のような構造を導入してください。

```text
TerminalState
  - primary_screen
  - alternate_screen
  - active_screen_kind
  - saved_cursor_for_alternate
````

既存の GridSnapshot は active screen から生成してください。

### 2. Resize propagation

ウィンドウサイズ変更時に以下が成立するようにしてください。

* VT core の grid rows/cols が更新される
* PTY 側へ resize が伝播される
* primary / alternate screen の両方が resize に耐える
* cursor が範囲外に出ない
* resize 後に full damage または適切な damage が返る

Windows ConPTY では resize / pipe I/O の扱いに注意してください。Microsoft のドキュメントでは ConPTY の入出力チャネルを適切に分離して扱う必要があるため、既存実装に合わせて deadlock を起こさない構造を維持してください。

### 3. Scroll Region / DECSTBM

以下を実装してください。

* CSI top ; bottom r

仕様:

* 1-based の top/bottom を内部 0-based に変換する
* 引数なしの場合は full screen に戻す
* invalid range は安全に無視するか clamp する
* scroll operation は scroll region 内だけに作用する
* cursor 位置は xterm 互換寄りに扱う

内部状態例:

```text
scroll_region_top: usize
scroll_region_bottom: usize // inclusive
```

### 4. Origin Mode / DECOM

以下を実装してください。

* CSI ? 6 h
* CSI ? 6 l

期待挙動:

* origin mode enabled のとき、cursor addressing は scroll region 相対
* disabled のとき、通常の画面絶対座標
* mode 切替時は cursor home 相当に移動する

### 5. Insert/Delete Line

以下を実装してください。

* CSI Ps L  // IL
* CSI Ps M  // DL

期待挙動:

* scroll region 内でのみ作用する
* cursor が scroll region 外にいる場合は無視
* Ps 省略時は 1
* 挿入/削除で生じる空行は現在の属性、または既定属性で埋める
* Damage::Lines を適切に返す

### 6. Insert/Delete Character

以下を実装してください。

* CSI Ps @  // ICH
* CSI Ps P  // DCH

期待挙動:

* 現在行の cursor 位置以降に作用
* wide / wide-spacer セルを壊さないように扱う
* Ps 省略時は 1
* 行末から溢れるセルは捨てる
* 空きセルは既定属性または現在属性で埋める
* Damage::Lines で対象行を返す

### 7. Bracketed Paste

以下を実装してください。

* CSI ? 2004 h
* CSI ? 2004 l

期待挙動:

* bracketed paste mode が off のときは通常 paste
* on のときは paste 内容を以下で包んで PTY へ送る

```text
ESC [ 200 ~
<pasted text>
ESC [ 201 ~
```

既存の paste 入力経路がない場合は、内部 API とテストだけ先に作ってください。

## テスト方針

可能な限り unit test を追加してください。

最低限ほしいテスト:

1. alternate screen に切り替えて文字を書いても primary screen が保持される
2. alternate screen から戻ると primary screen が復元される
3. 1049 h/l で cursor save/restore される
4. DECSTBM で scroll region が設定される
5. scroll region 内だけ scroll される
6. DECOM enabled 時の cursor addressing が region 相対になる
7. IL / DL が scroll region 内だけに作用する
8. ICH / DCH が現在行だけに作用する
9. bracketed paste mode on/off で PTY へ送る byte sequence が変わる
10. resize 後に cursor が範囲内に clamp される

wide cell / wide-spacer を扱う既存テストがある場合は、ICH/DCH で壊れないテストも追加してください。

## 手動確認

実装後、可能なら以下を確認してください。

```bash
cargo test --workspace
cargo build --workspace
```

GUI 起動後:

```text
echo hello
cargo test
git log --oneline --graph
less <適当な長いファイル>
nvim
```

期待:

* less / nvim 終了後に元画面へ戻る
* 画面上端/下端のスクロールが壊れない
* cursor 位置がズレない
* 通常入力で不必要な full redraw にならない
* cargo test / build が通る

## 注意

* 今回は tabs / panes / scrollback / selection / OSC 8 / OSC 52 / kitty graphics は実装しない
* 既存の RenderPlan と wgpu/glyphon レイヤーを大きく書き換えない
* VT core 側の純粋ロジックに寄せ、可能な限り GPU 非依存でテスト可能にする
* 実装が大きくなりすぎる場合は、alternate screen + DECSTBM + bracketed paste を優先し、残りは TODO として分離する

## 完了条件

* cargo test --workspace が成功する
* cargo build --workspace が成功する
* alternate screen の unit test がある
* scroll region の unit test がある
* bracketed paste の unit test がある
* nvim / less の起動終了で primary screen が破壊されない

[1]: https://invisible-island.net/xterm/ctlseqs/ctlseqs.html "ctlseqs(ms)"
