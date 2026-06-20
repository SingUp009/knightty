# 今回の実装範囲

## Phase 4-F1: Inline Image Foundation

今回実装するのは、**静止 PNG をセル位置にインライン表示する最小垂直スライス**です。

対応対象:

```text
OSC 1337 ; File=inline=1:<base64-data> ST
```

今回は次を対象外にします。

* GIF アニメーション
* JPEG
* ファイル転送
* ダウンロード専用シーケンス
* `preserveAspectRatio=0`
* Kitty Graphics Protocol
* Sixel
* 画像 ID による更新
* z-index
* 画像上への複雑なテキスト合成
* スクロールバックへの画像ピクセル保存
* ファイルパスを直接読み込む転送方式

iTerm2 Inline Images Protocol は OSC 1337 と Base64 を利用するため、既存の OSC 処理に統合できます。([iTerm2][2])

---

# 設計方針

## 1. Core は圧縮画像や GPU リソースを保持しない

`crates/core` が保持するものは、画像の論理情報だけにします。

```rust
pub struct ImageId(u64);

pub struct ImagePlacement {
    pub image_id: ImageId,
    pub anchor: GridPoint,
    pub columns: u16,
    pub rows: u16,
    pub source_width: u32,
    pub source_height: u32,
}
```

画像本体は別のストアへ置きます。

```rust
pub struct DecodedImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Arc<[u8]>,
}
```

推奨配置:

```text
crates/
  core/
    src/image.rs
  render/
    src/image.rs
  app/
    src/inline_image.rs
```

責務は以下です。

| 層                            | 責務                              |
| ---------------------------- | ------------------------------- |
| `core`                       | ID、セル基準の配置、削除、スクロール追従           |
| `app`                        | OSC の解釈、Base64 復号、PNG デコード、制限検査 |
| `render`                     | RGBA の GPU アップロード、quad 描画、キャッシュ |
| `alacritty_terminal adapter` | OSC イベントを Knightty 側へ通知         |

---

## 2. 圧縮データはメイン描画ループでデコードしない

PTY スレッドまたは専用デコード処理で以下を済ませます。

```text
OSC受信
  ↓
メタデータ解析
  ↓
Base64復号
  ↓
サイズ制限確認
  ↓
PNGデコード
  ↓
RGBA8へ変換
  ↓
ImageCommandとしてGUI側へ送信
```

GUI 側には次のような完成済みデータを送ります。

```rust
pub enum ImageCommand {
    Add {
        id: ImageId,
        image: DecodedImage,
        placement: ImagePlacement,
    },
    Delete {
        id: ImageId,
    },
    Clear,
}
```

ただし、最初のフェーズでは PNG デコードが多少重くても実装を単純化して構いません。重要なのは、**レンダリング中に毎フレーム再デコードしないこと**です。

---

## 3. 画像は文字セルとは独立して保持する

画像を各セルに分割して埋め込む設計は避けます。

```text
Terminal grid
Image placements
GPU image cache
```

をそれぞれ独立させます。

セルに画像断片を所有させると、次の問題が発生します。

* テキスト更新で画像に穴が開く
* サイズ変更時の再配置が複雑
* Kitty の z-index や画像更新へ拡張しにくい
* 大きな画像でセルごとのデータ量が増える

Kitty の仕様も、画像データと配置を別の概念として扱います。将来の Kitty 対応を考えても、`Image` と `Placement` を分離すべきです。([Kovid Goyal's Software Projects][3])

---

## 4. 最初の描画順

最初は次の順序にします。

```text
1. セル背景
2. インライン画像
3. 選択背景
4. グリフ
5. 下線・カーソル
```

これにより画像の上に文字を表示できます。

ただし、通常セルの背景を画像より後に描くと、背景色が画像を覆います。そこで初期実装では、画像が配置された領域でも通常どおり背景を先に描き、その上に画像を描きます。

透過 PNG は、画像の透明部分からセル背景が見える構造になります。

---

## 5. スクロール動作

画像のアンカーは、表示行番号ではなくターミナルの論理位置に関連付けます。

初期実装では次の挙動に限定します。

* 表示時のカーソル位置を画像左上とする
* 画像の高さに相当する行数だけカーソルを下へ進める
* 通常のスクロールで配置も一緒に移動する
* スクロールバック領域から完全に外れた画像は削除可能
* ウィンドウリサイズ時はセル数を維持し、ピクセル寸法を再計算する

画像を完全な scrollback エンティティとして扱うのは後続フェーズに回します。F1 では、表示中の論理行に追従できれば十分です。

---

# セキュリティ制限

画像シーケンスは任意サイズの Base64 データを受け取れるため、必ず制限が必要です。

初期値として以下を推奨します。

```rust
pub struct ImageLimits {
    pub max_encoded_bytes: usize,   // 16 MiB
    pub max_decoded_bytes: usize,   // 64 MiB
    pub max_width: u32,             // 8192
    pub max_height: u32,            // 8192
    pub max_pixels: u64,            // 32_000_000
    pub max_images: usize,          // 128
    pub max_total_gpu_bytes: usize, // 256 MiB
}
```

必須チェック:

```text
Base64デコード前の文字列長
Base64デコード後のバイト長
PNGヘッダーの幅・高さ
width × height のオーバーフロー
width × height × 4 のオーバーフロー
画像数
GPUキャッシュ総量
```

壊れた画像や制限超過は、端末全体をエラー終了させず無視してログへ記録します。

---

# 依存候補

最小構成なら以下です。

```toml
base64 = "..."
image = { version = "...", default-features = false, features = ["png"] }
```

`image` crate は PNG のみに限定し、JPEG、GIF、WebP などを最初から有効化しない方がよいです。

依存バージョンは、実装時にワークスペースの MSRV と既存依存を確認して決めます。

---

# Codex への実装指示

以下をそのまま渡せます。

````markdown
# Phase 4-F1: iTerm2 Inline PNG Image Foundation

Knightty に、プロトコル非依存の画像表示基盤と、
iTerm2 Inline Images Protocol の最小 PNG 対応を実装してください。

## 背景

Knightty は Rust 製ターミナルエミュレータです。

現在の主要構成:

- VT core: alacritty_terminal
- window/input: winit
- renderer: wgpu + glyphon
- terminal snapshot / damage / render plan を使用
- セル背景、文字、下線、カーソル、選択範囲を描画済み
- OSC 8 hyperlink 対応済み
- scrollback、selection、mouse、keyboard、IME commit 対応済み

将来 Kitty Graphics Protocol と Sixel を追加する予定です。
今回の設計を iTerm2 固有のデータモデルにしないでください。

## 目的

次の形式で送信された静止 PNG を、現在のカーソル位置から
インライン画像として描画できるようにしてください。

    OSC 1337 ; File=inline=1:<base64 PNG> ST

BEL 終端と ST 終端のうち、既存の OSC parser が渡せる形式を確認し、
可能なら両方をサポートしてください。

## 今回の対応範囲

対応:

- inline=1
- Base64 デコード
- PNG デコード
- RGBA8 への変換
- 現在のカーソル位置への配置
- アスペクト比維持
- セル単位の配置サイズ計算
- wgpu texture と quad による描画
- 透明 PNG の alpha blend
- 通常スクロールへの追従
- リサイズ時の再配置
- resource limits
- headless unit tests
- render plan tests

対象外:

- JPEG
- GIF / animation
- file transfer
- download-only sequences
- multipart image transfer
- preserveAspectRatio=0
- Kitty Graphics Protocol
- Sixel
- z-index
- image replacement
- image composition
- shared memory
- temporary file transfer
- file path transfer
- network access

未知または未対応の属性は、シーケンス全体を panic させず、
安全に無視または reject してください。

## アーキテクチャ

画像データ、配置、GPU リソースを分離してください。

推奨概念:

```rust
pub struct ImageId(u64);

pub struct ImagePlacement {
    pub image_id: ImageId,
    pub anchor: GridPoint,
    pub columns: u16,
    pub rows: u16,
    pub source_width: u32,
    pub source_height: u32,
}

pub struct DecodedImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Arc<[u8]>,
}

pub enum ImageCommand {
    Add {
        id: ImageId,
        image: DecodedImage,
        placement: ImagePlacement,
    },
    Delete {
        id: ImageId,
    },
    Clear,
}
````

型名や配置場所は既存コードへ自然に合わせて変更して構いません。

ただし、以下の制約を守ってください。

* 各セルに画像ピクセルや画像断片を所有させない
* compressed PNG bytes を renderer に渡さない
* 毎フレーム PNG を decode しない
* GPU texture を terminal core に持たせない
* renderer 固有型を core に入れない
* iTerm2 固有属性を共通 ImagePlacement に混在させない

## OSC integration

既存の alacritty_terminal adapter と OSC 8 対応を確認し、
OSC 1337 を Knightty 側へ通知する最小の統合点を作ってください。

alacritty_terminal を大規模に fork しないでください。

OSC payload のコピー回数を不必要に増やさず、
不正な UTF-8 や巨大 payload で panic しないようにしてください。

## iTerm2 metadata parser

少なくとも次を解析してください。

```
File=inline=1:<payload>
```

可能なら次も解析してください。

```
File=name=<base64-name>;width=<value>;height=<value>;preserveAspectRatio=1;inline=1:<payload>
```

width / height は初期実装では以下を許可してください。

* 未指定
* セル数
* `auto`

ピクセル、割合などの複雑な単位は今回は reject して構いません。

名前は表示に使用せず、デコード可能な場合のみ診断情報として保持してください。

metadata parser は純粋関数として分離し、
table-driven unit tests を追加してください。

## サイズ決定

width と height が未指定の場合:

1. source image の aspect ratio を維持する
2. source pixel size と現在の cell pixel size から必要セル数を求める
3. columns / rows は ceil する
4. columns は terminal columns 内へ制限する
5. 必要なら縮小し、画面幅を超えないようにする
6. 0 cell にはしない

片方のみ指定された場合は、aspect ratio からもう片方を求めてください。

cell pixel size が 0 の場合は画像を reject し、
ゼロ除算しないでください。

## カーソルとスクロール

画像左上を受信時のカーソル位置へ配置してください。

画像表示後は、画像が占有する rows に基づいてカーソルを進めてください。
既存 terminal semantics と衝突する場合は、
iTerm2 imgcat の期待動作に近い最小挙動を文書化してください。

画像配置は viewport の絶対ピクセル位置ではなく、
terminal の論理行・列に関連付けてください。

通常の terminal scroll が発生した際に、
画像配置も同じだけ移動してください。

scrollback から完全に脱落した placement と、
参照されなくなった image resource は回収してください。

完全な永続 scrollback image support は今回の範囲外です。
その制限をドキュメントに明記してください。

## Renderer

wgpu で RGBA8 texture を作成し、text glyph atlas とは分離してください。

最低限必要なもの:

* image texture cache
* image quad generation
* texture upload
* alpha blending
* viewport clipping
* texture/resource eviction
* resize 後の quad 再計算

描画順:

1. cell backgrounds
2. inline images
3. selection background
4. glyphs
5. underline / cursor / remaining overlays

既存の sRGB surface と色補正を壊さないでください。

PNG は標準的な sRGB RGBA データとして扱い、
二重 gamma correction を行わないでください。

画像ごとに render pipeline や bind group layout を作り直さないでください。
pipeline と layout は renderer 初期化時に共有してください。

初期実装では画像ごとの bind group は許容しますが、
毎フレーム再作成しないでください。

## Damage

次の場合に画像領域を damage としてください。

* 画像追加
* placement 移動
* scroll
* resize
* placement 削除
* texture upload 完了

既存 Damage::Lines だけでは正しく表現できない場合は、
最小限の ImageDamage または Full damage fallback を追加してください。

最初の実装では画像変更時の Full damage を許容しますが、
通常の文字入力まで常時 Full damage に退行させないでください。

## Resource limits

設定可能または定数化された制限を追加してください。

推奨初期値:

* encoded payload: 16 MiB
* decoded compressed bytes: 16 MiB
* max width: 8192
* max height: 8192
* max pixels: 32,000,000
* max decoded RGBA bytes: 128 MiB
* max image count: 128
* max total GPU image bytes: 256 MiB

必須:

* checked_mul / checked_add を使う
* width * height * 4 の overflow を防止する
* Base64 decode 前に payload length を検査する
* PNG decode 後にも実寸を検査する
* 制限超過で panic しない
* 制限超過を terminal text として PTY へ送り返さない
* concise な debug/warn log を出す

## Configuration

最低限、画像表示全体を無効化できる設定を追加してください。

例:

```toml
[graphics]
enabled = true
max_encoded_bytes = 16777216
max_decoded_bytes = 134217728
max_images = 128
max_gpu_bytes = 268435456
```

設定構造は既存 config conventions に合わせてください。

`enabled = false` の場合:

* OSC 1337 image payload を描画しない
* Base64 decode しない
* terminal parser を壊さない
* payload を通常文字として表示しない

## Tests

少なくとも以下を追加してください。

### Metadata parser

* minimal inline image
* name
* width
* height
* width + height
* auto
* unknown key
* duplicate key
* malformed key/value
* missing colon
* missing payload
* inline=0
* invalid Base64 metadata name
* oversized metadata

### Payload and decoder

* valid 1x1 PNG
* transparent PNG
* invalid Base64
* invalid PNG
* truncated PNG
* oversized encoded payload
* oversized dimensions
* decoded byte overflow
* zero dimension rejection

### Placement

* no explicit dimensions
* width only
* height only
* terminal width clamp
* non-square cell size
* zero cell size
* cursor near bottom
* scroll caused by image
* viewport resize

### Renderer plan

* image quad is between background and glyph pass
* correct destination rectangle
* correct UV rectangle
* viewport clipping
* alpha image does not disable later text rendering
* image cache reuses an existing texture
* removed placement eventually releases unused texture

実 GPU が必要なテストだけに依存せず、
quad generation と placement calculation は headless test 可能にしてください。

## Manual smoke test

Linux / WSL / PowerShell のいずれかで再現可能な、
小さな PNG を表示する smoke script を追加してください。

外部ネットワークから画像を取得するテストにはしないでください。
repository 内の fixture または生成した 1x1 / small PNG を使用してください。

可能であれば iTerm2 の imgcat 互換形式でも確認してください。

確認項目:

* PNG が表示される
* 透明部分から背景が見える
* 画像後のテキストが表示される
* スクロールで画像が移動する
* resize で panic しない
* clear/reset で resource が回収される
* graphics.enabled=false で表示されない

## Documentation

以下を文書化してください。

* 対応する iTerm2 sequence
* PNG only
* size unit restrictions
* current scrollback limitation
* resource limits
* Kitty Graphics Protocol は未対応
* Sixel は未対応
* animation は未対応

既存の compatibility document があれば更新し、
なければ `docs/image-compatibility.md` を追加してください。

## Dependencies

必要なら以下を追加してください。

* base64
* image with PNG feature only

`image` crate の default features は無効化し、
今回不要な codec を有効化しないでください。

既存 MSRV と dependency policy を確認してください。

## Quality requirements

次を通してください。

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets
cargo test --workspace
git diff --check
```

既存の keyboard、IME、selection、hyperlink、scrollback、
color rendering tests を壊さないでください。

最後に以下を報告してください。

* 変更ファイル
* OSC integration point
* image ownership model
* GPU resource lifetime
* resource limit behavior
* supported metadata
* known limitations
* smoke test command
* 実行した検証結果

---

Kitty プロトコルは PNG だけでなく、24-bit RGB、32-bit RGBA、画像と配置の分離、複数転送方式などを規定しています。そのため、先に今回の共通画像モデルを作っておくことで、Kitty 対応時の変更をプロトコル処理中心に限定できます。([Kovid Goyal's Software Projects][3])

[1]: https://github.com/alacritty/vte/issues?utm_source=chatgpt.com "Issues · alacritty/vte"
[2]: https://iterm2.com/documentation-images.html?utm_source=chatgpt.com "Inline Images Protocol"
[3]: https://sw.kovidgoyal.net/kitty/graphics-protocol/?utm_source=chatgpt.com "Terminal graphics protocol - kitty - Kovid's software projects"
