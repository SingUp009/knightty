# PHASE2.md

このドキュメントは、`AGENT.md` と `docs/PHASE1.md` を前提に、フェーズ2の描画基盤を実装・検証するための作業指示である。

フェーズ2では、フェーズ1の「文字が出る最小縦切り」を、ターミナルセル単位の描画基盤へ引き上げる。`glyphon` / `cosmic-text` / `wgpu` の構成は維持し、独自 VT パーサやリガチャ対応は導入しない。

---

## 1. フェーズ2の目的

フェーズ2の目的は、以下を満たす描画基盤を作ることである。

> VT core の可視グリッドを、セル単位の文字・色・スタイル・背景・カーソルとして GPU 描画できる。

この段階では互換プロトコルを増やすのではなく、フェーズ3で truecolor / undercurl / OSC / Kitty 系を載せるための描画境界を固める。

---

## 2. 完了条件

以下をすべて満たしたらフェーズ2完了とする。

- `cargo build --workspace` が通る
- `cargo test --workspace` が通る
- `core` の `GridSnapshot` がカーソル表示状態と wide / wide-spacer を公開する
- `core` の `Damage` が `Full` だけでなく行単位の変更範囲を返せる
- `render` が `GridSnapshot` から GPU 非依存の `RenderPlan` を作れる
- 前景色・背景色・bold・italic・underline・inverse が `RenderPlan` に反映される
- 背景・下線・カーソルは wgpu の矩形描画で出る
- 文字は glyphon の glyph cache / atlas / text renderer を使って描画される
- `app` の terminal / PTY resize は renderer の cell metrics を基準にする
- `config.json` からフォント family / size / line height / 初期セル数 / wgpu backend を読める
- `cargo run -p app` で実ウィンドウが開き、入力・色・スタイル・リサイズが確認できる

---

## 3. このフェーズでやらないこと

フェーズ2では以下を実装しない。

- OSC 8 hyperlink
- OSC 133 semantic prompt
- OSC 7 working directory report
- Kitty keyboard protocol
- Kitty graphics protocol
- 独自 terminfo
- ネイティブタブ・分割
- スクロールバック UI
- リガチャ対応
- ベンチ最適化
- 独自 VT パーサのフルスクラッチ

---

## 4. 描画方針

`render` は `GridSnapshot` を直接 GPU に流さず、まず GPU 非依存の `RenderPlan` に変換する。

`RenderPlan` は以下を持つ。

- glyphon に渡す styled text segment
- 背景矩形
- 下線矩形
- カーソル矩形

文字描画は `glyphon` を使う。背景・下線・カーソルは wgpu の簡易 rect pass で描く。これにより、glyph cache / atlas は既存ライブラリへ任せつつ、ターミナル固有のセル背景とカーソルはセル座標で制御できる。

シェイピングは glyph fallback を優先して `Shaping::Advanced` を使う。ただし、ターミナルのセル幅を崩しやすい OpenType feature は無効化する。

- `liga`
- `clig`
- `dlig`
- `calt`

リガチャを見た目として有効化することはフェーズ2では扱わない。

---

## 5. config.json

`config.json` は初回起動時に自動生成しない。存在しない場合は既存のデフォルト設定で起動する。

探索順は以下の通り。

1. `KNIGHTTY_CONFIG` で指定されたパス
2. OS 標準のユーザー設定パス

Windows:

```text
%APPDATA%\knightty\config.json
```

Linux:

```text
$XDG_CONFIG_HOME/knightty/config.json
~/.config/knightty/config.json
```

最小例:

```json
{
  "font": {
    "family": "CaskaydiaCove Nerd Font",
    "size": 16,
    "line_height": 18
  },
  "window": {
    "initial_cols": 100,
    "initial_rows": 30
  },
  "render": {
    "wgpu_backend": "auto"
  }
}
```

`KNIGHTTY_WGPU_BACKEND` は一時的なデバッグ用 override として残し、`render.wgpu_backend` より優先する。

---

## 6. GUI smoke 手順

実機確認では以下を確認する。

```bash
cargo run -p app
```

ウィンドウが開いたら以下を入力する。

```cmd
echo hello
dir
echo [31mred[0m [48;2;1;2;3mbackground[0m [1;3;4mbold italic underline[0m
```

確認すること:

- 入力した文字が表示される
- `dir` の出力が表示される
- 赤文字が表示される
- 背景色が矩形として表示される
- bold / italic / underline が反映される
- カーソルが表示される
- ウィンドウリサイズで列数・行数が更新され、PTY 側にも反映される
- Nerd Font 系 family を `config.json` に指定すると LazyVim などの private-use glyph の欠けが改善する

---

## 7. 注意

通常の `target/debug/.cargo-lock` がアクセス拒否になる環境では、一時 target dir を使って検証する。

```bash
CARGO_TARGET_DIR=<tmp>/knightty-target cargo test --workspace
CARGO_TARGET_DIR=<tmp>/knightty-target cargo build --workspace
```
