F4 は完了として扱ってよいです。設計上の重要点もすべて反映されています。

* source rectangleはintersectionで解決
* crop寸法を基準に配置サイズを計算
* offsetは受信時に検証し、resize後も保持
* z-indexは4層
* 同一z-indexはclient image ID順
* cropごとのtexture生成なし
* edge UV + `ClampToEdge`
* 画像変更時だけFull damage

412 testsとrelease buildまで通っているため、ここで追加の構造変更は不要です。

## 次のフェーズ

次は **Phase 4-F5: Kitty raw RGB/RGBA transmission** が自然です。

対応形式:

```text
f=24  # RGB, 3 bytes/pixel
f=32  # RGBA, 4 bytes/pixel
s=<pixel width>
v=<pixel height>
```

対象はまずdirect transmissionのみとします。

```text
t=d
m=0 / multipart
a=t / a=T
```

PNGとは違い、raw形式には画像寸法情報が含まれないため、`s`と`v`を必須にします。

## 実装方針

デコード後の共通経路を次の形に揃えるとよいです。

```rust
enum KittyImageFormat {
    Png,
    Rgb24 {
        width: u32,
        height: u32,
    },
    Rgba32 {
        width: u32,
        height: u32,
    },
}
```

最終的にはすべて既存の形式へ変換します。

```rust
struct DecodedImage {
    width: u32,
    height: u32,
    rgba: Arc<[u8]>,
}
```

### RGB24

```text
R G B R G B ...
```

を次へ展開します。

```text
R G B 255 R G B 255 ...
```

### RGBA32

データをそのままRGBA8として使用します。

## 必須検証

期待するpayload長をchecked arithmeticで計算します。

```rust
let channels = match format {
    Rgb24 => 3,
    Rgba32 => 4,
};

let expected = width
    .checked_mul(height)
    .and_then(|pixels| pixels.checked_mul(channels))
    .ok_or(Error::ImageTooLarge)?;
```

次はすべてreject対象です。

* `s`または`v`なし
* `s=0`
* `v=0`
* payloadが期待値より短い
* payloadが期待値より長い
* `width × height × channels` overflow
* `max_pixels`超過
* RGBA変換後の`max_decoded_bytes`超過
* multipart全体のencoded quota超過

## PNG経路との共通化

推奨構造:

```rust
fn decode_kitty_image(
    command: &KittyCommand,
    decoded_payload: &[u8],
    limits: &ImageLimits,
) -> Result<DecodedImage, KittyError> {
    match command.format {
        KittyFormat::Png => decode_png(decoded_payload, limits),
        KittyFormat::Rgb24 => decode_rgb24(
            decoded_payload,
            command.width_pixels,
            command.height_pixels,
            limits,
        ),
        KittyFormat::Rgba32 => decode_rgba32(
            decoded_payload,
            command.width_pixels,
            command.height_pixels,
            limits,
        ),
    }
}
```

placement、crop、offset、z、quota、GPU uploadは既存処理をそのまま通します。

## 注意点

### Alphaはstraight alphaを維持

`f=32`のRGBAをpremultiplyしないでください。

現在の画像pipelineがstraight alpha前提なら、そのままアップロードします。premultiplied alphaを使っている場合は、PNG側との一貫性を確認する必要があります。

### `s`と`v`はsource image寸法

これらは配置サイズではありません。

```text
s/v = source pixel dimensions
c/r = destination cell dimensions
```

混同しないよう、内部名は明確に分離します。

```rust
source_pixel_width
source_pixel_height
placement_columns
placement_rows
```

### cropとの連携

F4のsource rectangleはraw画像にもそのまま適用します。

```text
f=32,s=100,v=100,x=10,y=10,w=40,h=40
```

この場合、100×100のRGBA画像を登録し、その40×40領域だけを表示します。

## テスト項目

### parser

* `f=24,s=2,v=2`
* `f=32,s=2,v=2`
* `s`なし
* `v`なし
* zero dimension
* overflow
* duplicate `s` / `v`
* PNGでの不要な` s/v`
* unsupported format

### decode

* 1×1 RGB
* 1×1 RGBA
* RGB→RGBA変換
* alpha 0
* alpha 128
* alpha 255
* exact payload length
* short payload
* long payload
* pixel count limit
* decoded byte limit
* multiplication overflow

### multipart

* RGBの分割転送
* RGBAの分割転送
* pixel境界と無関係なchunk分割
* final chunk後のみdecode
  -失敗時に旧画像を維持
  -成功時にatomic replacement

### render integration

* RGB画像表示
* RGBA透過
* raw画像へのcrop
* offset
  -4層z-order
* texture cache reuse
* PNG経路に回帰なし

## 手動スモーク

小さな2×2画像を使うと確認しやすいです。

```text
赤 緑
青 白
```

RGBA bytes:

```text
255,0,0,255
0,255,0,255
0,0,255,255
255,255,255,255
```

これをBase64化し、次で送信します。

```text
ESC_Ga=T,f=32,s=2,v=2,c=20,r=10,i=50;<payload>ESC\
```

Windows ConPTYではAPCが消えるため、実描画smokeはLinux環境で行い、Windowsではparserからplacementまでの統合テストを使用します。

## F5後の候補

F5完了後は次の優先順位です。

1. Kitty delete selector拡張
2. image number `I`
3. Unicode placeholder
4. animation
5. Sixel

まずは **raw RGB/RGBA** を実装するのが、現在の画像パイプラインを最小変更で拡張でき、他端末との互換範囲も広げられる選択です。
