# Phase 4-F4: Kitty Source Rectangles, Pixel Offsets, and Z-Index

Knightty の Kitty Graphics Protocol 実装へ、placement単位のsource rectangle、pixel offset、z-indexを追加してください。

## 現在の状態

実装済み:

* iTerm2 OSC 1337 static PNG
* Kitty APC transport
* Kitty direct PNG transmission
* multipart `m=1` / `m=0`
* `a=T`, `a=t`, `a=p`, `a=d,d=i`
* image ID / placement ID
* cell dimensions `c`, `r`
* cursor policy `C`
* response policy `q`
* GPU texture cache
* logical placement and scroll tracking
* Windows ConPTY APC limitation diagnostics
* Unix APC preservation CI test

既存の画像resource、placement、GPU texture cacheを再利用してください。

## F4-A: Source Rectangle

Kitty placement keysとして以下を追加してください。

```text
x=<source x>
y=<source y>
w=<source width>
h=<source height>
```

これらはsource image pixel coordinatesです。

### Semantics

* 未指定時は画像全体
* source rectangleはplacement固有
* 同一画像resourceを複数placementで異なるcrop表示可能
* cropped textureを新規作成しない
* GPU texture全体を再uploadしない
* rendererでUV rectangleを計算する

推奨型:

```rust
pub struct ImageSourceRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}
```

`ImagePlacement`へsource rectangleを追加してください。

### Validation

reject:

* `w=0`
* `h=0`
* `x >= image_width`
* `y >= image_height`
* `x + w` overflow
* `y + h` overflow
* source rectangleが画像境界を超える

initial implementationではclampせず、`EINVAL`を返してください。

`x/y`だけが指定され、`w/h`が省略された場合は、画像右端・下端までを使用してください。

`w/h`だけが指定された場合は、`x=0`, `y=0`としてください。

## F4-B: Pixel Offset

以下を追加してください。

```text
X=<pixel x offset>
Y=<pixel y offset>
```

placementのanchor cell左上からのpixel offsetです。

推奨型:

```rust
pub struct ImagePixelOffset {
    pub x: u16,
    pub y: u16,
}
```

### Validation

* `X < cell_width`
* `Y < cell_height`
* cell pixel sizeが0の場合はreject
* overflowしない
* viewport clippingを適用
* resize後もcell-relative offsetとして維持

offsetを適用した結果、画像が隣接セルへはみ出すことは許可してください。

destination rectangle計算はrendererまたはrender-plan生成層で行ってください。

## F4-C: Z-Index

以下を追加してください。

```text
z=<signed integer>
```

placement単位で保持してください。

推奨型:

```rust
pub struct ImageZIndex(pub i32);
```

### Ordering

placementを以下で安定ソートしてください。

1. z-index ascending
2. insertion sequence ascending

同一z-indexでは既存placementの挿入順を維持してください。

最低限、描画passを以下へ分離してください。

```text
negative-z images
cell backgrounds
zero-z images
selection
glyphs
positive-z images
underline/cursor/final overlays
```

既存renderer構造上、この厳密な順序が難しい場合は、その理由を文書化し、少なくともnegative / zero / positiveの3層を実現してください。

## Parser

Kitty command parserへ以下を追加してください。

```text
x
y
w
h
X
Y
z
```

要件:

* 小文字・大文字を区別
* 重複キーの扱いを既存方針へ統一
* 不正整数
* 負のunsigned値
* i32 overflow
* u32 overflow
* 未知キーは既存方針どおり
* malformed commandでpanicしない

## Placement Replacement

同一非ゼロ `(image_id, placement_id)` を再配置した場合:

* source rectangleを更新
* pixel offsetを更新
* z-indexを更新
* 古いplacementを残さない
* texture resourceは再利用する

## Damage

次の場合に旧領域と新領域の両方をdamageしてください。

* crop変更
* pixel offset変更
* z-index変更
* placement replacement
* resize

矩形damageが未実装の場合、画像変更時のみfull damage fallbackを許可します。

通常文字入力を常時full damageにしないでください。

## Renderer

texture cache keyにsource rectangleを含めないでください。

quad generationで以下を生成してください。

```rust
pub struct ImageQuad {
    pub destination: PixelRect,
    pub uv: UvRect,
    pub z_index: i32,
    pub insertion_order: u64,
}
```

source rectangleから正規化UVを生成してください。

浮動小数点誤差により隣接pixelが混入しないか確認してください。

texture samplerの既存設定を確認し、crop境界でbleedingが起きる場合はUVまたはsampler設定を調整してください。

## Tests

### Parser

* x/y/w/h all specified
* x/y only
* w/h only
* X/Y
* negative z
* zero z
* positive z
* invalid unsigned value
* invalid signed value
* overflow
* duplicate keys
* uppercase X/Y distinct from lowercase x/y

### Source Rectangle

* full image default
* valid crop
* crop at right/bottom edge
* zero width
* zero height
* x outside image
* y outside image
* x + w overflow
* y + h overflow
* crop exceeding image bounds
* same image with multiple crops
* named placement replacement updates crop

### Pixel Offset

* zero offset
* maximum valid offset
* offset equal to cell size rejected
* zero cell size
* clipping at viewport edge
* resize recalculation
* named placement replacement updates offset

### Z-Index

* negative before backgrounds
* zero at normal image layer
* positive after glyphs
* stable ordering at same z
* placement replacement updates z
* delete removes correct layer
* scroll preserves ordering

### Renderer

* UV calculation
* destination pixel rectangle
* source crop does not create new texture
* texture cache reuse
* viewport clipping with offset
* transparent cropped PNG
* no glyph pipeline regression

## Documentation

Update `docs/image-compatibility.md` with:

* supported `x,y,w,h`
* supported `X,Y`
* supported `z`
* source bounds behavior
* pixel offset limits
* image/text layering behavior
* current remaining unsupported Kitty features

## Validation

Run:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --workspace --release
git diff --check
```

Report:

* changed files
* parser changes
* placement model changes
* UV calculation
* z-order implementation
* damage behavior
* tests added
* known limitations
* validation results
