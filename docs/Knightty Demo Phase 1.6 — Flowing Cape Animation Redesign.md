# Knightty Demo Phase 1.6 — Flowing Cape Animation Redesign

現在の`knightty-demo`について、騎士のマント表現を全面的に作り直してください。

今回の目的は、単一polygonが波打つ「旗」のような表現ではなく、キャラクターの周囲を大きな弧で取り囲み、身体の動作より遅れて追従する、演出的なマントアニメーションを作ることです。

添付された参考画像は、次の要素だけをモーションリファレンスとして使用します。

- 身体の左右へ大きく広がる布
- 手前と奥で分離した複数の布レイヤー
- 大きなS字カーブ
- 身体停止後も継続する布の動き
- 身体を追い越すfollow-through
- マントによる画面構成
- 明確な前後関係
- 布と身体の間のnegative space

参考画像のキャラクター、衣装、髪型、武器、装飾、配色、具体的な輪郭は複製しないでください。

Knightty用のオリジナルキャラクターとマントを使用してください。

---

## 1. 最初に現状を確認する

実装前に次を確認してください。

- 現在の騎士SVG
- 現在のマント形状
- SVG asset converter
- generated animation format
- animation timeline
- camera処理
- compositing処理
- Half Block preview
- contact sheet生成処理
- 現在のruntime allocation
- 現在のperformance metrics

確認後、次を簡潔に報告してください。

1. 現在のマントが単一shapeか複数shapeか
2. マントの変形方式
3. 現在のpose間補間方式
4. 再利用できるasset pipeline
5. 作り直す必要があるasset
6. runtime性能への影響見込み

その後は確認を求めず、実装してください。

---

## 2. 最重要方針

現在の騎士SVGとマントSVGは、品質改善のため作り直して構いません。

ただし、次の実装は維持してください。

- Half Block encoder
- terminal control
- frame scheduler
- metrics
- CLI
- pause/resume
- resize handling
- output buffer reuse
- asset converterの基本構造
- generated assetのversioning
- preview/contact sheet機能

今回の中心は、キャラクター全体ではなくマントの形状と動きです。

粒子、発光、カメラシェイクを増やして品質を補わないでください。

マント単体を黒い背景へ表示した状態でも、動きが美しく見える必要があります。

---

## 3. マントを複数レイヤーへ分割する

マントを単一polygonとして扱わないでください。

最低限、次の独立レイヤーへ分割してください。

```text
cape_far
cape_main
cape_near
cape_lower
ribbon_far
ribbon_near
````

### `cape_far`

* 身体の奥側を通る
* 画面奥方向へ流れる
* 最も暗い色を使用する
* 動きは少し遅い
* キャラクター本体より先に描画する

### `cape_main`

* マントの主要な面
* 肩または背中から始まる
* 画面の30〜50%程度を占める場面を作る
* 大きなS字カーブを持つ
* 一枚の三角形にしない

### `cape_near`

* 身体の手前へ回り込む布
* キャラクター本体より後に描画する
* 身体の一部を一時的に隠してよい
* 奥側の布とは異なる方向へ曲げる
* 前後関係を明確にする

### `cape_lower`

* 腰または背中の下側から広がる
* 脚とマントの間に空間を作る
* 脚を完全に覆わない
* 身体停止後に遅れて揺れる

### `ribbon_far` / `ribbon_near`

* 細長い補助布
* 主マントより大きく動いてよい
* 主マントと完全に平行にしない
* 斬撃方向を視覚的に補助する
* 細かく増やしすぎない

---

## 4. マントの固定点

マント全体を平行移動させないでください。

各布レイヤーには固定点を定義してください。

```rust
struct CapeAnchor {
    position: Vec2,
    rotation: f32,
}
```

推奨固定位置:

```text
cape_far:    far shoulder
cape_main:   upper back
cape_near:   near shoulder
cape_lower:  waist / lower back
ribbons:     shoulder ornament or upper back
```

固定点付近はほとんど動かさず、先端へ近づくほど移動量を増やしてください。

```text
root:   0〜10%
middle: 30〜60%
tip:    80〜100%
```

マントの付け根が肩から剥がれたり、画面上を滑ったりしないようにしてください。

---

## 5. Fixed-topology cape asset

マントの各レイヤーは、キーポーズ間で頂点対応を維持してください。

推奨形式:

```svg
<polygon id="cape-main" points="..." />
<polygon id="cape-near" points="..." />
<polygon id="cape-far" points="..." />
```

各キーポーズで次を統一してください。

* 同じlayer ID
* 同じ頂点数
* 同じ頂点順序
* 同じwinding
* 同じanchor相当頂点
* 自己交差しない形状

asset converterは、各polygonの頂点を抽出できるようにしてください。

runtimeでは対応する頂点同士を補間します。

```rust
current_vertex = lerp(pose_a_vertex, pose_b_vertex, interpolation);
```

ただし、すべての区間を線形補間するのではなく、sceneごとにhold、ease、overshootを選択してください。

SVG pathの汎用morph engineは実装しないでください。

---

## 6. マントの形状要件

各マントshapeは次を満たしてください。

* 直線だけで構成しない
* 外周に大きな凸曲線と凹曲線を作る
* 先端を2〜4個の大きな房へ分ける
* 細かなギザギザを大量に作らない
* 根元は狭く、中央から広げる
* 身体と布の間にnegative spaceを作る
* 布同士の間にも一部隙間を作る
* 左右対称にしない
* 全レイヤーを同じ方向へ流さない
* すべての先端を同じ長さにしない

避ける形状:

* 三角形一枚
* 半円一枚
* 頂点をsin波で均等に動かした形
* 細い帯を大量に並べた形
* 身体と一体化した大きな塊
* 小さな凹凸が連続するノイズ状の輪郭

---

## 7. マント専用キーポーズ

キャラクター本体とは別に、最低限次のマントキーポーズを作成してください。

```text
cape_idle_a
cape_idle_b
cape_pull_back
cape_anticipation
cape_slash_hold
cape_whip_forward
cape_overshoot
cape_rebound
cape_settle_a
cape_settle_b
```

最低10個の異なるマント状態を用意してください。

### `cape_idle_a`

* 重力で下方向へ垂れる
* 弱い風で片側へ流れる
* 身体との間に小さな空間がある

### `cape_idle_b`

* 先端だけが少し持ち上がる
* rootはほぼ変化させない
* idle_aと完全に反転させない

### `cape_pull_back`

* 身体が動き始めても布は元の位置へ残る
* 身体と布の間隔が広がる
* 先端は進行方向と逆へ流れる

### `cape_anticipation`

* 斬撃方向と逆へ最大限に膨らむ
* 主布が大きな弧を作る
* 手前布と奥布の方向を少し変える
* 最も大きなnegative spaceを作る

### `cape_slash_hold`

* 身体と剣は高速移動する
* マントrootだけが身体に追従する
* 中央と先端はまだ後方へ残る
* この時点でマント全体を前方へ動かさない

### `cape_whip_forward`

* 斬撃後に主マントが前方へ追いつく
* 先端が身体を追い越す
* 近景布が画面手前へ大きく回り込む
* 遠景布は少し遅らせる

### `cape_overshoot`

* 前方へ最も大きく広がる
* 参考画像のように身体の左右を布が囲む構図を作る
* キャラクター本体よりマントの面積が大きくなってよい
* 身体を完全に隠さない

### `cape_rebound`

* overshootから逆方向へ戻る
* 全レイヤーを同時に戻さない
* ribbonを最後まで動かす

### `cape_settle_a` / `cape_settle_b`

* 振幅を減衰させる
* 先端だけに小さな動きを残す
* 次のidleへ自然につなげる

---

## 8. タイミング

マントと身体で異なるタイミングを使用してください。

例として、斬撃を中心に次のように構成します。

```text
0 ms:    body anticipation開始
80 ms:   cape rootが動き始める
140 ms:  bodyが斬撃方向へ移動
180 ms:  swordが最大速度
220 ms:  bodyがfollow-throughへ到達
280 ms:  cape mainが前方へ追いつく
340 ms:  cape nearが身体を追い越す
420 ms:  cape farが最大変形
520 ms:  ribbonsが最大変形
650 ms:  cape rebound
900 ms:  main capeが停止
1100 ms: ribbonsと先端が停止
```

正確な値は調整して構いませんが、次を必ず守ってください。

* マント全体を身体と同時に動かさない
* near、main、far、ribbonのピークをずらす
* 身体停止後も最低300〜600 msは布を動かす
* 各レイヤーへ2〜6フレーム程度の位相差を作る
* smear poseの最中に布を完成位置へ移動させない

---

## 9. 補間

通常の`lerp`だけではなく、マント専用の補間を用意してください。

最低限:

```rust
fn ease_out_cubic(t: f32) -> f32;
fn ease_in_out_cubic(t: f32) -> f32;
fn overshoot(t: f32, amount: f32) -> f32;
fn damped_settle(t: f32, frequency: f32, decay: f32) -> f32;
```

`damped_settle`は先端とribbonに限定して使用してください。

物理シミュレーションの厳密な再現は不要です。

重要なのは、次の順序です。

```text
root
  ↓
main body of cloth
  ↓
outer edge
  ↓
tips
  ↓
ribbons
```

各段階が少し遅れて動くようにしてください。

---

## 10. 内部変形と全体変形を分ける

マントへ次の2種類の変形を適用してください。

### Pose morph

authored key pose間の大きな形状変化です。

例:

* 下に垂れる
* 後ろへ引かれる
* 横へ広がる
* 前へ回り込む
* 反動で戻る

### Secondary deformation

pose morphへ加える小さな変形です。

例:

* 先端のわずかな遅れ
* 弱い風
* 減衰振動
* camera方向への小さなparallax

secondary deformationだけで主アニメーションを作らないでください。

現在のような全頂点への単純なsin変形は削除するか、先端の微細変形だけに限定してください。

---

## 11. キャラクターポーズも修正する

マントだけを改善しても、身体が現在の前屈姿勢のままでは公開品質になりません。

キャラクターを次のように修正してください。

* 6〜7頭身程度
* 上半身を起こす
* 首、肩、胸、腰を判別可能にする
* 脚を長くする
* 両脚の間にnegative spaceを作る
* 武器を身体から離す
* 武器と月を重ねない
* 常時しゃがんだ姿勢にしない
* anticipation時だけ重心を下げる
* follow-throughでは身体を伸ばす

マントが大きく広がっても、身体の中心線と重心が読めるようにしてください。

---

## 12. 構図

参考画像のような布の広がりを表現するため、常に横向きの全身ショットを使用しないでください。

最低限、次の2カットを作成してください。

### Wide shot

* キャラクター全身
* マントの全体形状を見せる
* マントが左右へ広がる
* 月または光源は剣先から離す

### Dynamic medium shot

* 頭から膝付近まで
* マントが左右の画面端へ広がる
* near capeが画面手前を横切る
* far capeが身体の奥側へ流れる
* キャラクターを完全な中央へ置かない

ロゴ前の最も印象的なフレームは、dynamic medium shotにしてください。

---

## 13. 色とレイヤー順

マントの前後関係を色でも区別してください。

推奨:

```text
cape_far:    mid-tone dark
cape_main:   foreground
cape_near:   foreground light
cape_lower:  mid-tone
ribbons:     accentまたはforeground
```

描画順:

```text
far background
cape_far
far ribbon
character rear parts
cape_main
character body
weapon
cape_near
near ribbon
particles
slash highlight
```

すべての布を同じ色にしないでください。

ただし色数を増やしすぎず、既存palette内で前後関係を表現してください。

---

## 14. 低解像度対策

SVG sourceは少なくとも次の解像度を基準に作成してください。

```text
320 × 180
```

asset converterで次のpreviewを生成してください。

```text
320 × 180 source preview
160 × 90 logical preview
80 × 45 small-terminal preview
Half Block encoded preview
```

自動縮小後に次を確認してください。

* マントの各房が一つの塊になっていない
* near capeとfar capeの境界が残っている
* 身体とマントの間に隙間がある
* 脚がマントへ埋もれていない
* 布の先端が1pxノイズになっていない
* 大きなS字カーブが読める

必要なら低解像度専用に頂点位置を補正してください。

---

## 15. マント単体preview

通常のcontact sheetとは別に、マントだけを表示したcontact sheetを生成してください。

```text
target/knightty-demo-preview/
├── cape-contact-sheet.png
├── cape-motion-strip.png
├── character-contact-sheet.png
└── terminal-contact-sheet.png
```

`cape-motion-strip.png`には最低限次を横並びで表示してください。

```text
idle
pull-back
anticipation
slash-hold
whip-forward
overshoot
rebound
settle
```

マントのみを表示しても、動きの方向と位相差が読み取れる必要があります。

---

## 16. Visual acceptance criteria

次を満たさない場合、実装完了扱いにしないでください。

### 形状

* マントが単一の三角形に見えない
* 布が最低3つの大きな面に分かれて見える
* 大きなS字カーブがある
* 身体とマントの間に明確な空間がある
* 手前と奥の布を区別できる
* 布の先端が均一でない

### 動き

* 身体よりマントが遅れて動く
* 身体停止後も布が動く
* near、main、far、ribbonで最大変形時刻が異なる
* 斬撃直後に布が身体を追い越す
* overshootとreboundがある
* 最終的に振幅が減衰する
* 全布レイヤーが同じ波形で動かない

### 構図

* マントが画面の左右へ広がる印象的なフレームがある
* キャラクターが常に横向き全身ではない
* dynamic medium shotがある
* 月と剣が重ならない
* マントで身体全体を隠さない
* ロゴ前に公開用スクリーンショットとして成立するフレームがある

---

## 17. Performance要件

runtimeへcloth physics engineを追加しないでください。

禁止:

* runtime cloth simulation
* runtime SVG parsing
* 毎フレームのheap allocation
* 毎フレームの頂点配列clone
* 汎用path morph
* 大量の細分化頂点
* 布レイヤーごとの個別stdout write
* 毎フレームの乱数生成

推奨:

* 固定頂点数のpolygon
* 起動時にassetを一度展開
* pose buffer再利用
* interpolated vertex buffer再利用
* canvas再利用
* output buffer再利用
* deterministic timeline

マントレイヤー追加前後で、同条件のperformance metricsを比較してください。

---

## 18. Tests

最低限次を追加してください。

### Cape asset

* 全poseでlayer IDが一致する
* 全poseでlayerごとの頂点数が一致する
* windingが一致する
* anchor頂点が存在する
* polygonが空でない
* 極端な自己交差がない
* canvas外へ異常に出ていない

### Motion

* rootの移動量がtipより小さい
* tipがrootより遅れて最大変位へ到達する
* cape_nearとcape_farのピーク時刻が異なる
* body停止後もcape tipが動く
* overshoot後にreboundする
* settle終端で速度が十分小さい
* 同じ時刻から同じ頂点位置を得られる

### Visual structure

* マントと身体の間に一定量のbackground pixelが存在する
* nearとfarのpaletteが異なる
* anticipationの横幅がidleより大きい
* overshootの横幅がslash-holdより大きい
* 80×45へ縮小しても複数の布領域が残る

---

## 19. 完了報告

完了時に次を報告してください。

### 現状分析

* 以前のマントが安っぽく見えた原因
* 単一polygonまたは単一波形だった箇所
* 再利用した基盤

### 新しいマント構造

* レイヤー一覧
* 固定点
* 頂点数
* key pose
* 補間
* timing offset
* overshoot/rebound

### Preview

生成した次のファイルを提示してください。

* cape contact sheet
* cape motion strip
* character contact sheet
* terminal contact sheet

### Performance

変更前後を同条件で比較してください。

### 検証

実行したコマンドと結果を記載してください。

### 未確認事項

TTYや視覚確認ができなかった項目を明記してください。

---

## 20. 検証コマンド

最低限、次を実行してください。

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets
cargo test --workspace
cargo build -p knightty-demo --release
git diff --check
```

asset converterとpreviewも実行してください。

```bash
cargo run -p knightty-demo-assets -- build
cargo run -p knightty-demo-assets -- preview
```

可能なら次も手動確認してください。

```bash
cargo run -p knightty-demo --release -- --fps 60 --duration 10
```

[1]: https://www.adobe.com/creativecloud/animation/discover/principles-of-animation.html?utm_source=chatgpt.com "Understanding the 12 principles of animation"
[2]: https://docs.blender.org/manual/en/latest/physics/forces/force_fields/types/wind.html?utm_source=chatgpt.com "Wind - Blender 5.1 Manual"
