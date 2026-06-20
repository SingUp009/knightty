**Phase 4-F2: Kitty Graphics Protocol — Direct Transmission MVP**

ただし、いきなり Kitty の全仕様を実装するのではなく、次の垂直スライスに限定します。

### 対応範囲

```text
ESC _ G <control-data> ; <payload> ESC \
```

実装する機能:

* APC シーケンスの受信
* `a=T`: 転送と表示を同時実行
* `a=t`: 転送のみ
* `a=p`: 転送済み画像の配置
* `a=d,d=i`: ID 指定削除
* `f=100`: PNG
* `t=d`: direct transmission
* `i=<image-id>`
* `p=<placement-id>`
* `c=<columns>`
* `r=<rows>`
* `C=0/1`: カーソル移動制御
* `q=0/1/2`: 応答抑制
* `m=0`: 単一チャンクのみ
* 成功・失敗レスポンス

Kitty は画像データと placement を明確に分離しており、同じ画像を複数箇所へ配置できます。画像 ID と placement ID の組で配置を識別し、同じ組を再指定した場合は置換する仕様です。現在の F1 で作った画像本体・論理配置・GPU キャッシュの分離を、そのまま活用できます。([Kovid Goyal's Software Projects][1])

## 最大の課題

現在も `alacritty/vte` の APC 対応 issue は open で、カスタム APC/SOS/PM parsing の PR も未マージです。したがって、`alacritty_terminal` のイベントだけでは Kitty シーケンスを取得できない可能性が高いです。([GitHub][2])

ここでは **Knightty 側に小さな入力ストリーム・プリプロセッサを置く**のが妥当です。

```text
PTY bytes
   │
   ▼
GraphicsEscapeRouter
   ├── Kitty APC → Kitty parser
   └── その他    → alacritty vte parser
```

### 避けるべき実装

* `alacritty/vte` をプロジェクト内へ丸ごと fork
* PNG Base64 の途中データを `String` として無制限に蓄積
* APC 全般を無条件に飲み込む
* byte ごとに `Vec` を再確保
* renderer に Kitty control-data parser を置く
* Kitty の image ID を現在の内部 `ImageId` と同一視する

外部アプリケーションの ID と Knightty 内部 ID は分離します。

```rust
pub struct KittyImageKey {
    pub client_id: u32,
}

pub struct InternalImageId(u64);

pub struct KittyPlacementKey {
    pub client_image_id: u32,
    pub placement_id: u32,
}
```

## 実装フェーズ

### F2-A: APC transport

最初は画像を描画せず、以下だけ実装します。

* `ESC _` で APC 開始
* `ESC \` で終了
* Kitty の `G` APC だけ捕捉
* Kitty 以外の APC は破棄または既存 parser 方針に従う
* APC 外のバイト列が以前と完全に同じ形で VTE に流れる
* fragmented read 対応
* split `ESC` / `\` 対応
* payload 上限
* reset/recovery

状態機械は最低限、次の形になります。

```rust
enum StreamState {
    Ground,
    Escape,
    Apc {
        kind_seen: bool,
        buffer: Vec<u8>,
        escape_pending: bool,
    },
}
```

ただし実装上は、通常の ESC シーケンスを壊さないよう、`ESC _ G` が確定するまではバイトを保留する必要があります。

### F2-B: Kitty command parser

control-data は純粋関数として分離します。

```rust
struct KittyCommand<'a> {
    action: KittyAction,
    format: KittyFormat,
    transmission: KittyTransmission,
    image_id: Option<u32>,
    placement_id: Option<u32>,
    columns: Option<u16>,
    rows: Option<u16>,
    cursor_movement: bool,
    quiet: KittyQuiet,
    more_chunks: bool,
    payload: &'a [u8],
}
```

未知キーは原則無視できますが、既知キーの不正値はエラーにします。

```text
_Ga=T,f=100,t=d,i=42,c=20,r=10;<base64 PNG>
```

### F2-C: F1 基盤への接続

```text
Kitty command
   ↓
Base64 decode
   ↓
既存 PNG decoder
   ↓
内部 ImageStore
   ↓
Kitty ID → InternalImageId mapping
   ↓
既存 ImagePlacement
   ↓
既存 GPU texture cache
```

PNG デコード・寸法制限・GPU byte quota は F1 の実装を再利用し、iTerm2 と Kitty で別の検証経路を作らないようにします。

## このフェーズでは見送るもの

次は F2 MVP に含めない方がよいです。

* `m=1` のチャンク転送
* `f=24` RGB
* `f=32` RGBA
* `t=f` ファイル転送
* `t=t` 一時ファイル
* `t=s` shared memory
* source rectangle `x,y,w,h`
* pixel offsets `X,Y`
* z-index
* Unicode placeholders
* animation
* relative placement
* image number `I=`
* 全 delete selector

特に chunking は partial upload の寿命、再送、削除時の中断、quota 管理を伴うため、単一チャンクが安定してから独立フェーズにすべきです。

## 完了条件

F2 MVP の smoke test は、以下が確認できれば十分です。

```powershell
$esc = [char]27
$png = [Convert]::ToBase64String(
    [IO.File]::ReadAllBytes(".\docs\fixtures\small.png")
)

[Console]::Write(
    $esc + "_Ga=T,f=100,t=d,i=42,c=10,r=5;" +
    $png +
    $esc + "\"
)
```

確認項目:

1. 画像が表示される
2. 同じ `i=42` の再転送で旧画像と placement が置換される
3. `a=p,i=42` で再配置できる
4. 存在しない ID に `a=p` すると `ENOENT` が返る
5. `q=1` で成功応答だけ抑制される
6. `q=2` で全応答が抑制される
7. `a=d,d=i,i=42` で削除される
8. OSC 1337 は引き続き動作する
9. 通常の UTF-8、OSC、CSI、IME 入力に回帰がない

[1]: https://sw.kovidgoyal.net/kitty/graphics-protocol/?utm_source=chatgpt.com "Terminal graphics protocol - kitty - Kovid's software projects"
[2]: https://github.com/alacritty/vte/issues?utm_source=chatgpt.com "Issues · alacritty/vte"
