# Knightty Demo Phase 1.5 — Visual Direction Upgrade

既に実装済みの`knightty-demo`について、描画性能と端末制御機構を維持したまま、アニメーションのビジュアル品質を大幅に改善してください。

今回の目的は、機能追加やベンチマーク方式の変更ではありません。

現在のプロシージャルな騎士が「円、線、polygonを組み合わせた図形」に見える問題を解消し、低解像度端末上でも映画的で、一目でKnightty固有のデモと分かるアニメーションへ更新することが目的です。

---

## 1. 作業開始前の確認

最初に以下を確認してください。

- リポジトリの`AGENT.md`または`AGENTS.md`
- `docs/performance-demo.md`
- `crates/demo/README.md`
- `crates/demo/src`以下の全実装
- 現在のanimation timeline
- Canvasとrasterizer
- Half Block encoder
- terminal guard
- frame scheduler
- metrics
- headless tests
- workspaceの依存管理方針
- 現在の未コミット変更

既存のPhase 1実装を前提に作業してください。

実装前に、次を簡潔に報告してください。

1. 現在のアニメーション構造
2. 再利用する部分
3. 置き換える部分
4. 追加するasset pipeline
5. 予定するファイル変更

その後は確認を求めず、実装と検証を進めてください。

---

## 2. 最重要方針

次の既存機構は、明確な不具合がない限り変更しないでください。

- CLI
- raw modeと入力処理
- alternate screen
- terminal state restoration
- pause/resume
- resize handling
- frame scheduler
- performance metrics
- Half Block encoder
- ANSI True Color出力
- stdout buffer reuse
- 1フレーム1回のwrite/flush
- terminal sizeに応じたlogical canvas
- Phase 1のheadless tests

変更対象は主に次です。

- `animation`
- 騎士の入力データ
- 背景のレイヤー構成
- カメラ演出
- 斬撃演出
- KNIGHTTYロゴ
- asset変換ツール
- animation-specific tests

今回の改善によって、PTYへ送るデータ量、フレームスケジューリング、Half Blockエンコード性能を大きく悪化させないでください。

---

## 3. アートディレクション

次の要素を参考にしますが、既存作品のキャラクター、構図、フレーム、ロゴを直接複製しないでください。

### 参考にする要素

- 高コントラストなシルエット
- 大胆な余白
- 左右非対称の構図
- 前景、中景、遠景による奥行き
- 黒と淡色の反転を使った場面転換
- 少ないキーポーズによる力強い動き
- 斬撃時のsmear pose
- 動作前の静止とanticipation
- 動作後に残るマントと粒子
- 画面構成による映画的なカット

### 直接使用してはいけないもの

- Fateシリーズのキャラクターデザイン
- セイバーの髪型、鎧、剣、衣装、輪郭
- Bad Apple!!の既存フレーム
- Samurai Jackのキャラクターや具体的なカット
- LIMBO、INSIDE、Katana ZERO等の画像や抽出アセット
- インターネットから取得した画像、GIF、動画、SVG
- 既存作品のトレース
- 既存ロゴの改変

すべてKnightty用のオリジナル造形にしてください。

---

## 4. Knighttyの騎士デザイン

騎士は、細部ではなくシルエットで識別できるデザインにしてください。

### 必須のシルエット要素

- 左右非対称の兜
- 兜から後方へ伸びる短い飾り
- 大きな片側の肩当て
- 細い腰
- 膝下まで広がるマント
- 身長の約75〜90%の長剣
- 剣先付近の小さな欠け、または特徴的な形状
- 前後で形状が異なる脚部
- 接地感のある足
- 明確な前方方向

### 避ける造形

- 頭を単純な円で表現する
- 胴体を単純な長方形で表現する
- 腕と脚を同じ太さの線で表現する
- 剣を単なる一本の直線にする
- マントを三角形一枚にする
- 左右対称にする
- 全パーツを同じ大きさのpolygonで作る
- 顔を細かく描く
- 鎧へ細かい模様を入れる

160×90相当の低解像度へ縮小しても、騎士だと認識できることを優先してください。

---

## 5. プロシージャル騎士からauthored key poseへ移行する

現在の騎士を、実行時にcircle、line、polygonを組み合わせて構築する方式から、手作りSVGキーポーズを事前変換して利用する方式へ変更してください。

背景、霧、粒子、斬撃、カメラは引き続きプロシージャル生成して構いません。

### 役割分担

#### SVGで作成するもの

- 騎士本体
- 兜
- 肩
- 胴体
- 腕
- 脚
- マントの基本形状
- 剣
- 抜刀姿勢
- anticipation pose
- slash smear pose
- follow-through pose
- KNIGHTTYロゴ

#### Rustで生成するもの

- 月
- 雲
- 霧
- 草や岩の小さな動き
- 粒子
- 風
- 斬撃残像
- 画面反転
- ディザリング
- カメラ移動
- 微小なマント端の揺れ
- ロゴの分解
- シーン間のwipe

---

## 6. SVG asset構成

次のような構成を推奨します。

```text
crates/demo/
├── assets/
│   ├── source/
│   │   ├── knight_idle_a.svg
│   │   ├── knight_idle_b.svg
│   │   ├── knight_enter_a.svg
│   │   ├── knight_enter_b.svg
│   │   ├── knight_enter_c.svg
│   │   ├── knight_reach.svg
│   │   ├── knight_draw_a.svg
│   │   ├── knight_draw_b.svg
│   │   ├── knight_anticipation.svg
│   │   ├── knight_slash_smear.svg
│   │   ├── knight_follow_a.svg
│   │   ├── knight_follow_b.svg
│   │   └── knightty_logo.svg
│   ├── generated/
│   │   └── animation.kfa
│   └── README.md
└── src/
````

既存構成に合わせて変更して構いません。

### SVGの制約

各SVGは次を満たしてください。

* viewBoxは全ファイルで統一する
* 推奨viewBoxは`0 0 160 90`
* 外部画像を参照しない
* 外部フォントを参照しない
* `<text>`を使用しない
* filter、blur、mask等の複雑なSVG機能へ依存しない
* path、polygon、rect、circle程度に限定する
* strokeよりfillを優先する
* paletteに存在する固定色だけを使う
* alpha blendingを前提にしない
* 背景を透明にする
* 画面外の巨大なpathを作らない
* 不要なmetadataを含めない

### SVG palette

正確に次の色だけを使用してください。

```text
transparent
background marker: #11111B
foreground:        #CDD6F4
accent:            #B4BEFE
mid-tone:          #6C7086
```

`background marker`は原則としてSVG asset内では使用せず、透明領域として扱って構いません。

変換時に未知の色を検出した場合は黙って近似せず、エラーにしてください。

---

## 7. Asset変換ツール

通常実行時にSVGを読み込まないでください。

通常のrelease binaryにSVG parserや画像decoderを含めないでください。

SVGは開発時に次の流れで変換します。

```text
SVG source
  ↓
Rust製asset converter
  ↓
indexed raster frames
  ↓
delta/RLE encoding
  ↓
animation.kfa
  ↓
include_bytes!
```

### 推奨コマンド

既存workspaceに`xtask`がある場合は再利用してください。

```bash
cargo xtask demo-assets
```

`xtask`が存在しない場合は、過剰なworkspace変更を避け、次のような専用ツールを追加してください。

```bash
cargo run -p knightty-demo-assets
```

または既存方針に合う小規模なbin targetを使用してください。

### 変換ライブラリ

Rust内でSVGをラスタライズできるライブラリを使用してください。

要件:

* 外部の`inkscape`や`aseprite`コマンドを必須にしない
* CIでも再生成可能
* Windows、Linuxで同じ結果になる
* 固定サイズへ決定的にラスタライズする
* anti-aliasing結果を最終的にindexed paletteへ量子化する
* 不明色や不正SVGをエラーにする
* 変換結果が決定的である
* asset converterの依存を通常のdemo runtimeへ持ち込まない

依存バージョンは、現在のworkspaceと互換性のある安定版を選択してください。

---

## 8. Generated asset format

既存の`.kfa`形式がある場合は拡張してください。

ない場合は、簡潔なversioned formatを追加してください。

例:

```text
magic:       "KFA1"
width:       u16
height:      u16
palette_len: u8
frame_count: u16
frame table
compressed frame data
```

各フレームはindexed colorとします。

```rust
#[repr(u8)]
enum AssetColor {
    Transparent = 0,
    Foreground = 1,
    Accent = 2,
    MidTone = 3,
}
```

### 圧縮

最低限、次を検討してください。

* 前フレームとの差分
* unchanged run
* transparent run
* repeated color run
* row boundary

ただし、圧縮率のために実装を過剰に複雑化しないでください。

重要なのは次です。

* 起動時に一度だけ展開する
* animation loop内で解凍しない
* runtimeの各フレームでheap allocationしない
* corrupted assetでpanicしない
* version mismatchを明確なエラーにする

---

## 9. Preview出力

asset converterへ静止画preview機能を追加してください。

推奨コマンド:

```bash
cargo run -p knightty-demo-assets -- preview
```

生成物の例:

```text
target/knightty-demo-preview/
├── 00-distant-shot.png
├── 01-knight-closeup.png
├── 02-anticipation.png
├── 03-slash.png
├── 04-logo.png
└── contact-sheet.png
```

preview生成物をGit管理対象にする必要はありません。

### Contact sheet

最低限、次の5状態を一枚へまとめてください。

1. 月と遠景
2. 騎士の背面クローズアップ
3. 抜刀直前
4. 斬撃
5. KNIGHTTYロゴ

Contact sheetには、最終的なHalf Block相当解像度へ縮小した結果も含めてください。

SVG原寸だけではなく、実際の端末解像度でシルエットが読めるか確認できるようにしてください。

---

## 10. 静止画品質を先に成立させる

アニメーションを追加する前に、次の5つの代表フレームが静止画として成立するようにしてください。

### Frame A: 遠景

* 月を画面右上に配置
* 騎士を画面左下へ小さく配置
* 画面中央は大きく空ける
* 前景、中景、遠景を分ける
* 月と騎士を中央揃えにしない

### Frame B: 背面クローズアップ

* 騎士の肩と兜を大きく表示
* マントが画面の約30〜45%を占める
* 月光で輪郭だけを出す
* 左右非対称にする
* 顔の詳細を描かない

### Frame C: Anticipation

* 重心を斬撃方向と逆へ移す
* 膝を少し曲げる
* 剣先を下げる
* 肩と腰の向きをずらす
* マントを斬撃方向と逆へ膨らませる
* 次にどちらへ動くか分かる姿勢にする

### Frame D: Slash smear

* 通常poseの中間補間にしない
* 斬撃専用の歪んだsilhouetteを使用する
* 剣と腕を一体化した大きな形状として扱う
* 主斬撃線と副斬撃線を分ける
* 1〜2フレームだけ表示する

### Frame E: Logo

* `KNIGHTTY`が明確に読める
* 5×7の汎用bitmap fontを使用しない
* 専用のvector wordmarkを使用する
* 剣の切断線とロゴの形状を関連付ける
* 斬撃方向にわずかに傾ける
* 端末上で文字間が潰れない

これらの静止画が成立していない状態で、粒子やカメラシェイクを増やして品質を誤魔化さないでください。

---

## 11. 背景を3層へ分割する

現在の単純な背景を次の3層へ変更してください。

```text
Far background
Mid background
Foreground
```

### Far background

* 月
* 空
* 大きく低周波な雲
* 微細な星または粒子
* 動きは非常に遅い

### Mid background

* 霧
* 遠方の岩
* 遠方の木または塔のsilhouette
* camera移動に対して小さくparallaxする

### Foreground

* 草
* 岩
* マントの一部
* 画面端を覆う黒いshape
* camera移動に対して大きくparallaxする

### Parallax係数例

```text
far:        0.10
mid:        0.35
character:  1.00
foreground: 1.30
```

値は画面に合わせて調整してください。

---

## 12. 見かけ上の階調

現在のpaletteを基礎にしつつ、ディザリングで見かけ上の階調を増やしてください。

例:

```text
background solid
background/foreground 25%
background/foreground 50%
background/foreground 75%
foreground solid
mid-tone
accent
```

### 要件

* ordered ditheringを使用してよい
* Bayer 4×4程度で十分
* 毎フレーム完全にパターンを変えない
* カメラに対してパターンが不自然に泳がない
* キャラクターの主要輪郭には過剰に使用しない
* 主に霧、月の周辺、雲、遠景へ使用する
* 細かすぎて端末上でノイズに見えないようにする

dither関数は決定的にしてください。

---

## 13. カメラシステム

論理キャンバス上に簡潔な2Dカメラを導入してください。

例:

```rust
struct Camera {
    position: Vec2,
    zoom: f32,
    shake: Vec2,
}
```

### 必要な機能

* translation
* uniform zoom
* optional shake
* reference canvasからoutput canvasへの変換
* aspect ratio維持
* deterministic result

### 使用箇所

* 遠景から騎士へのゆっくりしたpush-in
* 背面クローズアップへのcut
* 抜刀前の完全静止
* 斬撃時の2〜3 logical pixelの短いshake
* ロゴ表示時の安定
* 粒子化時の緩やかなpull-back

常時カメラを揺らさないでください。

camera shakeは斬撃の瞬間だけに限定してください。

---

## 14. アニメーションフレームレートの分離

端末出力は引き続き30、60、120 FPS等で動作します。

ただしキャラクターposeは、出力FPSと同じ頻度で滑らかに補間しないでください。

### キャラクター

* 約12〜15 FPS相当
* pose holdを使用する
* key pose間の補間は限定的
* anticipationでは長めに停止する
* smear frameは極端に短くする
* follow-throughでは本体を早く止める

### 60 FPSで更新するもの

* カメラ
* 霧
* 粒子
* 月の微細な変化
* マント端
* 斬撃残光
* logo wipe
* dithering thresholdの緩やかな変化

キャラクター本体まで常に`lerp`すると、重量感が失われるため避けてください。

---

## 15. 新しい8秒タイムライン

全体は引き続き約8秒で自然にループさせてください。

### Scene 1: Distant Moon

```text
0.00〜1.15秒
```

* 巨大な月
* 左下の小さな騎士
* 前景の草または岩
* ゆっくりした霧
* 画面中央に大きな余白
* 騎士はほぼ静止

### Scene 2: Close-up

```text
1.15〜2.45秒
```

* 背面または斜め背面のクローズアップへcut
* 兜、肩、マントの輪郭を強調
* 月光で細いrim lightを表現
* マント端のみ遅れて動く
* 剣の柄を画面内へ入れる

### Scene 3: Hand on Hilt

```text
2.45〜3.30秒
```

* 手を柄へ置く
* 身体をわずかに沈める
* 霧の動きを弱める
* 音はないが、静止による「溜め」を作る
* cameraもほぼ停止する

### Scene 4: Anticipation

```text
3.30〜3.72秒
```

* 重心を斬撃方向と逆へ移動
* 剣先を下げる
* 肩と腰を逆方向へ捻る
* マントを逆方向へ膨らませる
* accent色を剣先へ集中させる

### Scene 5: Slash

```text
3.72〜3.86秒
```

* 専用smear poseへ切り替える
* 斬撃は非常に短くする
* 主線は太く、副線は細くする
* camera shakeを短時間だけ適用する
* 全画面白フラッシュを使わない
* 点滅を繰り返さない

### Scene 6: Follow-through

```text
3.86〜4.70秒
```

* 騎士本体は早めに停止
* マント、霧、粒子だけが遅れて追従
* 斬撃線が画面を二分する
* 斬撃線を境界に背景の明暗を反転させる
* 月が一瞬分断されたように見せる

### Scene 7: KNIGHTTY

```text
4.70〜6.20秒
```

* 斬撃方向のwipeでロゴを出す
* 専用vector wordmarkを使用する
* 騎士は小さな背景silhouetteとして残してよい
* camera shakeを停止する
* ロゴが読める静止時間を最低0.7秒確保する

### Scene 8: Dissolve

```text
6.20〜8.00秒
```

* ロゴ輪郭から粒子が剥がれる
* 大粒子は遅く、小粒子は速くする
* 全粒子を同時に動かさない
* 粒子を月へ吸い込む
* 最終的にScene 1の月面ノイズへ接続する
* ループ境界で全画面clearや大きなjumpを起こさない

---

## 16. マントアニメーション

単一のsin波で全頂点を動かさないでください。

マントを少なくとも次の3領域へ分けてください。

```text
root
middle
tip
```

### 動き

* rootは肩に固定
* middleは小さく遅れて動く
* tipは大きく、さらに遅れて動く
* 斬撃前は逆方向へ膨らむ
* 斬撃後は本体停止後も追従する
* 常時同じ周期で揺らさない

例として、領域ごとに異なる振幅と遅延を使用してください。

```text
root amplitude:   0.0〜0.2
middle amplitude: 0.4〜0.8
tip amplitude:    1.0〜2.0
```

数値はlogical pixel基準で調整してください。

マント全体を変形する必要がある場合は、SVG poseを複数用意し、Rust側の変形だけに依存しないでください。

---

## 17. 粒子

粒子は固定seedから決定的に生成してください。

各粒子に次の属性を持たせてください。

```rust
struct ParticleSeed {
    origin: Vec2,
    size: u8,
    delay: f32,
    lifetime: f32,
    speed: f32,
    curvature: f32,
    phase: f32,
    color: PaletteIndex,
}
```

### 要件

* 毎フレーム乱数を生成しない
* 同じ時刻から同じ画面を生成する
* 大きさを2〜3段階に分ける
* delayを分散する
* lifetimeを分散する
* 直線移動だけにしない
* 月へ吸収される直前に速度を少し上げる
* 画面全体へ均等に配置しない
* silhouetteの重要部分を覆いすぎない

---

## 18. KNIGHTTYロゴ

現在の5×7または同等の汎用bitmapロゴは廃止してください。

`KNIGHTTY`専用のvector wordmarkを作成してください。

### デザイン要件

* 大文字
* 太いstroke感
* 一部に斜めの切断面
* `K`または`T`へ剣を連想させる形状
* 文字間隔は狭すぎない
* Half Block変換後にも読める
* `I`が消えない
* 2つの`T`を判別できる
* `Y`の下端が潰れない
* 斬撃線と同じ角度を使用する
* 既存フォントをoutline化して使用しない
* 文字をオリジナルpathで構成する

ロゴassetから、構成pixel座標を取得できるようにしてください。

これをwipeとparticle dissolveに使用します。

---

## 19. Performance要件

ビジュアル改善後も、Phase 1の性能特性を維持してください。

### Runtime hot path

禁止:

* runtime SVG parsing
* runtime PNG decoding
* フレームごとのasset decompression
* フレームごとの`Vec`生成
* フレームごとの`String`生成
* フレームごとのhash map構築
* pixelごとのstdout write
* animation loop内のログ
* 毎フレームの乱数生成器初期化
* key pose全体のclone

推奨:

* assetを起動時に一度展開
* pose bufferを再利用
* compositing bufferを再利用
* particle seedを起動時に生成
* logo pixel listを起動時に構築
* camera transformを小さな値型として扱う
* frame output bufferを引き続き再利用

### 性能回帰の基準

同じ端末サイズ、FPS、durationで、Phase 1と比較してください。

最低限比較:

```text
Rendered frames
Estimated dropped frames
Bytes written
Average bytes/frame
Encode time p50
Encode time p95
Frame time p50
Frame time p95
Frame time p99
Peak asset memory
```

可能なら、変更前後を同一環境で比較してください。

明確な理由なく、encode time p95またはframe time p95を大幅に悪化させないでください。

アニメーション生成時間が増えた場合は、原因を完了報告へ記載してください。

---

## 20. Tests

既存テストを維持したうえで、次を追加してください。

### Asset converter

* 全SVGが読み込める
* viewBoxが統一されている
* 未知色を拒否する
* 外部参照を拒否する
* frame sizeが一致する
* asset生成が決定的
* 同じ入力から同じbinary hashになる
* invalid magicを拒否する
* unsupported versionを拒否する
* truncated dataを拒否する
* corrupted runを拒否する

### Pose assets

各poseについて次を検証してください。

* foreground pixelが存在する
* bounding boxが空ではない
* canvas外へ出ていない
* knight pose同士が完全一致していない
* sword pixelが存在する
* anticipationとfollow-throughの重心が異なる
* smear poseのbounding boxが通常poseより広い

### Silhouette readability proxy

画像認識は不要です。

代わりに次の構造的条件をテストしてください。

* head、shoulder、torso、cape、swordの各regionにpixelが存在する
* swordのmajor axisがposeごとに期待方向を向く
* silhouetteのconnected component数が異常に多くない
* 極端に細い孤立pixelが過剰でない
* 160×90から80×45へ縮小しても一定pixel数が残る
* foregroundとaccentが両方存在するposeがある

### Animation timeline

代表時刻:

```text
0.00
0.80
1.50
2.80
3.50
3.78
4.20
5.30
6.80
7.70
7.99
```

各時刻で次を確認してください。

* canvas hash
* active scene
* active pose
* camera state
* logo visibility
* particle count
* palette別pixel count
* bounding box

### Loop

* `t = 0.0`と`duration`が同一状態になる
* `duration - epsilon`から`0.0`への差分が過剰でない
* 最終粒子位置が月付近へ収束する
* camera位置がloop境界でjumpしない

テストを単純なsnapshot更新だけで通さず、重要な構造条件もassertしてください。

---

## 21. Visual validation

可能な環境でpreviewを生成し、次を確認してください。

### Silhouette

* 騎士と認識できる
* 杖や旗ではなく剣と認識できる
* 頭、肩、胴、脚が分離して見える
* マントと身体が一体の塊に見えない
* anticipationとfollow-throughが異なる
* smear poseが通常poseの補間に見えない

### Composition

* 常に騎士が中央にいない
* 月と騎士の位置関係が画面ごとに変わる
* 余白がある
* 前景、中景、遠景が判別できる
* 斬撃前に一度画面が静まる
* logo表示中は読みやすい

### Terminal

* Half Block変換後も輪郭が読める
* 小さい端末でも主要shapeが消えない
* ditherが文字化けやちらつきに見えない
* 右端でautowrapしない
* 最終行でscrollしない
* resize後にも構図が維持される
* pause中に表示が崩れない

TTYまたは画像previewを利用できない場合は、未確認として明記してください。確認していないものを確認済みと報告しないでください。

---

## 22. Documentation

次を更新してください。

* `crates/demo/README.md`
* 必要なら`docs/performance-demo.md`
* `crates/demo/assets/README.md`

### 記載内容

* Phase 1.5の目的
* authored key pose方式
* SVG assetの制約
* asset再生成コマンド
* preview生成コマンド
* generated assetを直接編集しないこと
* 既存作品のassetを含まないこと
* 通常実行時にSVGを解析しないこと
* performance comparison方法
* asset変更時に必要な検証
* Aseprite等を使う場合も最終的にSVGへ書き出すこと
* 外部作品のトレースや同梱を禁止すること

---

## 23. 今回の非目標

今回、次は実装しないでください。

* FateまたはBad Apple!!のasset
* AI生成画像の同梱
* runtime PNG、GIF、動画decoder
* ffmpeg
* Kitty Graphics Protocol
* Sixel
* Braille renderer
* diff update
* audio
* shader
* GPU particle
* skeletal animation framework
* general-purpose SVG animation engine
* arbitrary user asset loading
* configuration fileによるtheme変更
* 複数キャラクター
* 複数アニメーション
* Knightty本体への組み込み
* benchmark protocol自体の変更

Phase 1.5では、単一のオリジナルアニメーションを高品質化することへ集中してください。

---

## 24. 実装品質

重視すること:

* 既存基盤の再利用
* シルエットの静止画品質
* 決定的なasset変換
* 決定的なanimation
* runtime allocation削減
* 小さく明確なasset format
* 端末サイズへの適応
* animationとasset pipelineの分離
* corrupted assetへの安全な処理
* テスト可能性
* visual preview可能性

避けること:

* 過剰なtrait階層
* 汎用ゲームエンジン化
* runtime asset loaderの一般化
* `unsafe`
* 巨大な生成Rust source
* base64 asset埋め込み
* SVG文字列を直接Rustへ埋め込む
* generated binaryを手編集する
* フレーム単位の大量clone
* art qualityを粒子量だけで補う
* 既存作品に酷似したデザイン

---

## 25. 検証コマンド

最低限、次を実行してください。

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets
cargo test --workspace
git diff --check
cargo build -p knightty-demo --release
```

asset converterを追加した場合は、assetの再生成も確認してください。

例:

```bash
cargo run -p knightty-demo-assets -- build
cargo run -p knightty-demo-assets -- preview
```

再生成後に差分が発生しないことを確認してください。

```bash
git diff --exit-code -- crates/demo/assets/generated
```

WindowsとLinuxの双方を実行できない場合は、実行できた環境のみ正確に報告してください。

---

## 26. 手動実行

可能なTTY環境で次を確認してください。

```bash
cargo run -p knightty-demo --release -- --fps 30 --duration 10
cargo run -p knightty-demo --release -- --fps 60 --duration 10
cargo run -p knightty-demo --release -- --fps 120 --duration 10
```

確認項目:

* 遠景が映画的に見える
* 騎士のsilhouetteが明確
* close-upが遠景と異なる
* anticipationに溜めがある
* slashが一瞬で完了する
* smear poseが認識できる
* follow-throughに重量感がある
* マントが本体より遅れて止まる
* logoが読みやすい
* dissolveが均一でない
* loop境界が目立たない
* 端末状態が復旧する
* resizeでpanicしない
* pause/resumeが動く
* performance statsが表示される

---

## 27. 完了報告

完了時は次の形式で報告してください。

### 調査結果

* 既存Phase 1の再利用箇所
* 変更したanimation architecture
* visual qualityが低かった主因

### Asset pipeline

* source asset
* converter
* generated format
* runtime load
* preview方法

### Visual changes

* 騎士デザイン
* key poses
* 背景レイヤー
* camera
* slash
* logo
* particle
* loop

### 変更ファイル

新規・変更・削除に分けて記載してください。

### 実行方法

通常実行、asset再生成、preview生成を記載してください。

### Performance comparison

変更前と変更後の結果を、同条件で可能な範囲で比較してください。

### 検証結果

各コマンドの成否を記載してください。

### 手動確認

確認できた項目と確認できなかった項目を分けてください。

### 残っている制限

未実装、またはvisual上さらに改善可能な点を明記してください。

---

## 28. 受け入れ条件

以下をすべて満たした場合に完了とします。

* 既存のHalf Block rendererを維持している
* 既存terminal controlを維持している
* 既存metricsを維持している
* runtimeでSVGを解析していない
* runtimeで画像をdecodeしていない
* オリジナルSVG key poseが追加されている
* 最低10個の有意に異なるkey poseがある
* anticipation poseがある
* slash smear poseがある
* follow-through poseがある
* 騎士が低解像度でも認識できる
* 汎用5×7ロゴが専用wordmarkへ置換されている
* 前景、中景、遠景が存在する
* camera cutまたはcamera push-inが存在する
* キャラクターposeにholdがある
* 斬撃が短時間で完了する
* マントが本体より遅れて停止する
* 粒子が固定seedで決定的
* 8秒前後で自然にloopする
* asset converterが決定的
* generated assetの破損を安全に拒否する
* previewを生成できる
* contact sheetを生成できる
* animation loop内に新規の重大なallocationがない
* 既存の30/60/120 FPS実行が維持される
* 既存のpause、resize、終了操作が維持される
* 終了後にterminal stateが復旧する
* visual testsとheadless testsが追加されている
* `cargo fmt`が成功する
* `cargo clippy`が成功する
* `cargo test`が成功する
* release buildが成功する
* `git diff --check`でwhitespace errorがない


[1]: https://www.aseprite.org/cli/?utm_source=chatgpt.com "Aseprite - Docs - Cli"
