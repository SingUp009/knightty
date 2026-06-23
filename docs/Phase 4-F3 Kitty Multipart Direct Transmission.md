F2 の実装範囲は成立しています。互換性資料にも、`a=T/t/p/d`、画像・placement ID、`C`、`q`、エラー応答、soft delete、再転送時の置換が整理されています。

ただし、次へ進む前に **2点だけ確認・修正候補**があります。

## 先に確認する点

### 1. `q=2` の意味

Kitty 仕様では次の意味です。

| 値     | 応答                       |
| ----- | ------------------------ |
| `q=0` | 成功・失敗とも応答                |
| `q=1` | 成功応答を抑制し、失敗は応答           |
| `q=2` | **失敗応答も抑制**し、結果として全応答を抑制 |

実装が「`q=2` は失敗だけ抑制するが成功は返す」となっていないか、テストで確認してください。仕様文面だけを見ると誤読しやすい箇所です。([Kovid Goyal's Software Projects][1])

推奨テスト:

```rust
#[test]
fn quiet_zero_emits_success_and_error() {}

#[test]
fn quiet_one_suppresses_success_only() {}

#[test]
fn quiet_two_suppresses_success_and_error() {}
```

### 2. smoke test が「出力成功」で終わっている

> PowerShell smoke script: 正常にKittyシーケンスを出力

これは transport script の検証にはなりますが、Knightty の end-to-end 描画検証にはまだ弱いです。

最低限、Knightty 上で次を目視確認してください。

* PNG が表示される
* 画像後の通常テキストが壊れない
* 同一 `(i,p)` の再配置で二重表示にならない
* `a=t` 後に `a=p` で表示される
* `a=d,d=i` 後に placement が消える
* soft delete 後に `a=p` で再表示できる
* `C=1` でカーソル位置が変わらない
* 不正 PNG に対して `EBADPNG`
* `graphics.enabled=false` では decode しない
* OSC 1337 が引き続き表示できる

## 次のフェーズ

次は **Phase 4-F3: Kitty Multipart Direct Transmission** が適切です。

```text
m=1  continuation
m=0  final chunk
```

現実の Kitty 対応アプリケーションでは、APC のサイズ制約を避けるため分割転送が重要です。最初のチャンクだけに通常の control-data があり、継続チャンクでは基本的に `m` と payload を扱います。転送完了前に delete 命令を受けた場合、未完成 upload は破棄する必要があります。([Kovid Goyal's Software Projects][1])

### F3 の対応範囲

* `t=d`
* `f=100`
* PNG
* `m=1` / `m=0`
* `i=<id>` 必須
* `a=t` と `a=T`
* partial upload のサイズ制限
* upload timeout または明示的な回収条件
* 同一 ID への新規転送による旧 partial upload の中断
* delete 命令による partial upload 中断
* final chunk 受信時だけ Base64 decode・PNG decode
* chunk 単位では応答せず、完了または失敗時に応答

### 推奨データモデル

```rust
struct PartialKittyUpload {
    image_id: u32,
    action: KittyAction,
    format: KittyFormat,
    placement: PendingPlacement,
    quiet: KittyQuiet,
    encoded: Vec<u8>,
}
```

ただし `HashMap<u32, PartialKittyUpload>` に無制限に保持してはいけません。

```rust
struct PartialUploadLimits {
    max_uploads: usize,
    max_encoded_bytes_per_upload: usize,
    max_total_encoded_bytes: usize,
}
```

推奨初期値:

```text
max_uploads = 16
max_encoded_bytes_per_upload = graphics.max_encoded_bytes
max_total_encoded_bytes = 32 MiB
```

## 状態遷移

```text
first chunk: m=1
    ↓
create PartialKittyUpload
    ↓
continuation: m=1
    ↓
append with checked quota
    ↓
final chunk: m=0
    ↓
append
    ↓
Base64 decode
    ↓
PNG decode
    ↓
atomic image replacement
```

重要なのは、**final decode が成功するまで既存画像を削除しないこと**です。

現在の画像 ID を再転送すると、仕様上は旧画像と全 placement を置換します。ただし、不完全または壊れた新規転送で旧画像まで失うと扱いにくいため、実装内部では以下の順序にします。

```text
受信・検証
→ Base64 decode
→ PNG decode
→ quota予約
→ 旧画像とplacementを削除
→ 新画像をcommit
```

これは外部からは仕様どおりの置換として見えつつ、失敗時には旧状態を保てます。

## 必須テスト

### transport

```text
ESC が read 境界で分割
ST の ESC と \ が別 read
control-data と payload が別 read
複数 APC が1 readに連結
通常UTF-8の途中に APC
不完全 APC 後の stream recovery
```

### multipart

```text
2 chunks
多数の小さい chunks
空の中間 chunk
空の最終 chunk
継続先が存在しない
同一IDで別upload開始
最大値ちょうど
1 byte超過
total quota超過
final chunkの不正Base64
final PNG decode失敗
deleteによるpartial abort
```

### atomicity

```text
既存画像への成功再転送
既存画像への失敗再転送
失敗時に旧placementを維持
成功時に旧placementを全削除
```

## その次の優先順位

F3 完了後は、次の順序がよいです。

1. **source rectangle と pixel offset**

   * `x,y,w,h`
   * `X,Y`
2. **z-index**

   * `z`
   * 負値をテキスト下へ描画
3. **追加 delete selector**

   * `d=a/A`
   * `d=p/P`
   * `d=x/X`
   * `d=y/Y`
   * `d=z/Z`
4. **raw RGB/RGBA**

   * `f=24`
   * `f=32`
   * `s`, `v` の寸法検証
5. Unicode placeholders
6. animation
7. Sixel

Unicode placeholdersはセル属性、foreground/underline color、結合文字、スクロールバックとの統合が必要なため、現時点では後回しが適切です。z-indexとsource rectangleの方が、現在の placement/renderer モデルを自然に拡張できます。([Kovid Goyal's Software Projects][1])

**結論として、`q` の応答テストと実描画 smoke を確認した後、F3 multipart direct transmission に進めます。**

[1]: https://sw.kovidgoyal.net/kitty/graphics-protocol/?utm_source=chatgpt.com "Terminal graphics protocol - kitty - Kovid's software projects"
