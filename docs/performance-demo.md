# Knightty Animation Demo — Phase 1 実装指示

Knighttyで端末描画性能を確認するための、独立したアニメーションデモCLIを実装してください。

今回実装するのは、既存作品の映像やキャラクターを使用しない、Knighttyオリジナルの白黒／低色数シルエットアニメーションです。

デザインは次のループとします。

1. 暗い画面に月が現れる
2. マントをなびかせた騎士が現れる
3. 騎士が剣を抜く
4. 画面を斜めに斬る
5. 斬撃の軌跡から `KNIGHTTY` ロゴが現れる
6. ロゴが粒子化する
7. 粒子が月へ戻り、最初のフレームへ自然にループする

このデモは見た目だけでなく、PTY、VT parser、セル更新、Unicode glyph描画、色属性処理、GPU描画を通過する性能テスト用ワークロードとして設計してください。

---

## 1. 作業開始前の確認

最初に次を確認してください。

- リポジトリルートの `AGENTS.md`
- workspaceの `Cargo.toml`
- 既存crate構成
- 既存のエラー処理方針
- 既存のCLI引数処理ライブラリ
- workspace dependencyの管理方法
- formatter、clippy、testの既存設定

既存方針を優先し、不要な依存関係や独自規約を増やさないでください。

実装前に簡潔な作業計画を提示し、その後は確認を求めず実装を進めてください。

---

## 2. 実装範囲

workspaceへ独立したバイナリcrateを追加してください。

推奨package名:

```text
knightty-demo
````

推奨配置:

```text
crates/demo/
├── Cargo.toml
└── src/
    ├── main.rs
    ├── cli.rs
    ├── terminal.rs
    ├── player.rs
    ├── canvas.rs
    ├── raster.rs
    ├── encoder.rs
    ├── metrics.rs
    └── animation/
        ├── mod.rs
        └── knight.rs
```

既存のworkspace命名規則と異なる場合は、既存規則へ合わせて構いません。

このcrateはKnightty本体とは別プロセスとして実行します。

```bash
cargo run -p knightty-demo --release
```

Knightty内のシェルからこのコマンドを起動することで、通常のPTY出力経路を通して描画してください。

### 今回変更しないもの

原則として、次のcrateには変更を加えないでください。

* `crates/core`
* `crates/render`
* `crates/app`

workspaceへのcrate登録など、ビルドに必要な最小限の変更は許容します。

デモ固有コードをKnightty本体へ混入させないでください。

---

## 3. 著作物と外部アセット

次の素材をリポジトリへ追加しないでください。

* Fateシリーズの画像やキャラクターフレーム
* Bad Apple!の映像フレーム
* Ghostty公式アニメーションの複製フレーム
* インターネットから取得した画像、GIF、動画
* 出典や再配布条件が不明な素材

今回のアニメーションはRustコードでプロシージャル生成してください。

画像ファイル、動画ファイル、巨大なテキストフレーム列は使用しないでください。

---

## 4. 描画モデル

### 4.1 Indexed Canvas

アニメーションは端末セルへ直接描画せず、いったん低解像度の論理キャンバスへ描画してください。

例えば次の型を用意します。

```rust
pub struct Canvas {
    width: usize,
    height: usize,
    pixels: Vec<PaletteIndex>,
}
```

パレットは最低限、次の3色を扱えるようにしてください。

```rust
pub enum PaletteIndex {
    Background,
    Foreground,
    Accent,
}
```

内部表現は性能上妥当なら`u8`でも構いません。

キャンバスサイズは原則として次のようにします。

```text
logical width  = terminal columns
logical height = terminal rows * 2
```

Unicode half blockを使用するため、1端末セルで縦2ピクセルを表現します。

ただし、右端のautowrapによるスクロールを避けるため、必要なら描画幅を次のようにしてください。

```text
logical width = terminal columns - 1
```

極端に小さい端末でもunderflowしないよう、`saturating_sub`等を使用してください。

---

## 5. Half Blockエンコーダー

論理キャンバスの縦2ピクセルを、1つの端末セルへ変換してください。

基本glyphは次です。

```text
▀
```

各セルについて:

```text
top    = canvas[x, y * 2]
bottom = canvas[x, y * 2 + 1]
```

変換規則:

* `top != bottom`

  * foregroundを`top`
  * backgroundを`bottom`
  * glyphとして`▀`
* `top == bottom`

  * backgroundをその色
  * glyphとして空白

同一のforeground/background組み合わせが連続する場合は、SGRシーケンスを繰り返さないでください。

フレーム全体を単純に各セル単位で次のように出力しないでください。

```text
SGR + glyph + SGR + glyph + SGR + glyph ...
```

最低限、連続run単位で属性をまとめてください。

### デフォルト色

低彩度で暗いKnightty向けパレットを使用してください。

推奨値:

```text
background = #11111b
foreground = #cdd6f4
accent     = #b4befe
```

ただし、色値は一箇所に定義し、後から設定可能にしやすい構造にしてください。

ANSI True Colorを使用します。

```text
ESC[38;2;<r>;<g>;<b>m
ESC[48;2;<r>;<g>;<b>m
```

---

## 6. ラスタライズ機能

外部画像ライブラリを導入せず、今回必要な最小限のラスタライズ処理を実装してください。

最低限必要な機能:

```rust
Canvas::clear(...)
Canvas::set(...)
Canvas::get(...)
Canvas::fill_rect(...)
Canvas::fill_circle(...)
Canvas::fill_polygon(...)
Canvas::draw_line(...)
Canvas::draw_thick_line(...)
```

### 要件

* キャンバス外への描画は安全にクリップする
* 負の座標を扱える内部APIにする
* polygonは凹形状まで対応する必要はない
* scanlineまたは同等の単純な塗りつぶしでよい
* フレームごとの過剰なheap allocationを避ける
* `Vec`等のバッファは可能な範囲で再利用する
* panicを発生させない

座標はアニメーション側では正規化座標、または固定のreference canvas座標で定義してください。

推奨reference canvas:

```text
160 × 90
```

実際の端末サイズへaspect ratioを維持してスケールし、中央配置してください。

---

## 7. アニメーション構成

アニメーション全体は約8秒のシームレスなループとしてください。

```text
duration = 8.0 seconds
```

各フレームは、経過時間を正規化した値から生成します。

```rust
let t = elapsed_seconds.rem_euclid(duration) / duration;
```

フレーム番号に依存して状態を更新するのではなく、原則として時間`t`から決定的にフレームを生成してください。

これにより、フレームドロップ時に古いフレームを飛ばせるようにします。

### 7.1 タイムライン

#### Scene A: 月の出現

```text
0.00 <= t < 0.15
```

* 背景は暗色
* 画面右上寄りに月を表示
* 月はforeground色
* fade-inはディザリングまたは粒子密度で表現
* 単純な透明度ブレンドは不要
* 数個の微小粒子を周囲に表示

#### Scene B: 騎士の登場

```text
0.10 <= t < 0.38
```

* 騎士が画面左側から中央へ移動
* 月を背にしたシルエットにする
* 頭、兜、胴体、脚、マント、剣を単純な幾何形状で構成
* 写実的である必要はない
* 一目で「剣を持った騎士」と分かる輪郭を優先
* マントは時間に応じて波打たせる
* 騎士の移動にはease-outを使う
* 足元に少量の粒子または風の線を入れる

#### Scene C: 抜刀

```text
0.34 <= t < 0.55
```

* 騎士が剣へ手を伸ばす
* 剣を斜め上方向へ抜く
* 腕と剣の角度を補間する
* マントは動きに少し遅れて追従させる
* 剣先はaccent色にする
* 動きの開始と終了は滑らかにする

#### Scene D: 斬撃

```text
0.54 <= t < 0.65
```

* 騎士が左上から右下、または左下から右上へ大きく斬る
* 斬撃は高速にする
* 2〜3フレーム相当の残像を時間関数で生成
* 斬撃線はaccent色
* 斬撃の瞬間に月や粒子が一瞬だけ分割されたように見せる
* 画面を激しく点滅させない
* 全画面白フラッシュは使用しない

#### Scene E: KNIGHTTYロゴ

```text
0.63 <= t < 0.84
```

* 斬撃線を境界として`KNIGHTTY`ロゴが現れる
* ロゴは中央配置
* 独自の小型bitmap fontをコード内に定義する
* 外部フォントや画像は使用しない
* 文字は最低でも5×7程度のドットフォントにする
* 表示領域に応じて整数倍で拡大する
* 小さい端末では倍率1まで縮小する
* 斬撃方向に沿ったwipeで出現させる
* ロゴ表示後、騎士は背景のシルエットとして残してよい

#### Scene F: 粒子化とループ

```text
0.82 <= t <= 1.00
```

* ロゴの構成ピクセルを粒子として分解する
* 粒子は月の方向へ吸い込まれる
* 疑似乱数は固定seedの決定的な関数を使う
* 実行ごとに異なる結果にしない
* 最後のフレームが最初の月の出現へ自然につながるようにする
* ループ境界で大きな位置ジャンプを起こさない

---

## 8. 補間関数

最低限、次の補間関数を用意してください。

```rust
fn clamp01(value: f32) -> f32;
fn lerp(a: f32, b: f32, t: f32) -> f32;
fn smoothstep(t: f32) -> f32;
fn ease_in_cubic(t: f32) -> f32;
fn ease_out_cubic(t: f32) -> f32;
fn ease_in_out_cubic(t: f32) -> f32;
```

scene固有の時間は次のような関数で正規化してください。

```rust
fn segment_t(t: f32, start: f32, end: f32) -> f32;
```

タイムライン内へマジックナンバーを無秩序に散在させないでください。

---

## 9. 騎士シルエット

騎士はコード上の幾何形状で構築してください。

推奨レイヤー:

1. 後方のマント
2. 後ろ脚
3. 胴体
4. 前脚
5. 肩
6. 頭
7. 兜
8. 腕
9. 剣
10. 剣の残像
11. 前景粒子

最低限、次の特徴を持たせてください。

* 兜または頭部の突起
* 肩幅のある胴体
* 後方へ広がるマント
* 明確な剣身
* 地面へ接地している姿勢
* 抜刀前と斬撃時で異なるpose

マントは複数頂点のpolygonとし、各頂点のY座標をsin関数等で変化させてください。

例:

```rust
offset = amplitude * sin(time * frequency + vertex_phase);
```

ただし、細かく波打ちすぎてノイズに見えないようにしてください。

---

## 10. Bitmapロゴ

`KNIGHTTY`の各文字をコード内のbitmapとして定義してください。

例:

```rust
const K: [&str; 7] = [
    "#...#",
    "#..#.",
    "#.#..",
    "##...",
    "#.#..",
    "#..#.",
    "#...#",
];
```

実際には文字列ベースでもbit maskベースでも構いません。

推奨文字:

```text
K N I G H T T Y
```

次を満たしてください。

* 全文字の高さを統一する
* 文字間隔を一定にする
* 端末幅に収まる最大整数倍率を計算する
* 収まらない場合は短縮せず、最小端末サイズ案内へ切り替える
* ロゴ描画処理をアニメーション本体から分離する
* wipeまたはparticle変換用に、ロゴを構成する論理ピクセル座標を列挙できるようにする

---

## 11. ターミナル制御

起動時に次の状態へ移行してください。

```text
ESC[?1049h  alternate screen
ESC[?25l    hide cursor
ESC[2J      clear
ESC[H       cursor home
```

必要に応じてautowrapを一時的に無効化できます。

```text
ESC[?7l
```

終了時は、成功、エラー、panicのどの場合でも最低限次を復旧してください。

```text
ESC[0m
ESC[?7h
ESC[?25h
ESC[?1049l
```

### TerminalGuard

RAII形式のguardを実装してください。

```rust
struct TerminalGuard {
    // ...
}
```

`Drop`で復旧処理を実行してください。

要件:

* 通常終了で復旧する
* `q`終了で復旧する
* Escape終了で復旧する
* Ctrl+C相当で可能な限り復旧する
* 描画処理がエラーを返しても復旧する
* 二重復旧しても問題が起きない
* 復旧時のエラーでpanicしない

既存依存関係に適切なterminal/raw-modeライブラリがある場合はそれを再利用してください。

ない場合は、このデモcrateに限定してクロスプラットフォーム対応の小さな依存関係を追加して構いません。

---

## 12. 入力

デモ実行中は、最低限次の入力に対応してください。

```text
q       終了
Escape  終了
Space   pause/resume
```

raw modeを使用し、キー入力にEnterを要求しないでください。

pause中は同じフレームを維持し、busy loopにしないでください。

pause解除後、停止時間分だけアニメーションが飛ばないよう、animation clockを補正してください。

---

## 13. リサイズ

実行中の端末リサイズに対応してください。

* 端末サイズを起動時だけで固定しない
* resize eventを利用できる場合は利用する
* 利用できない場合は低頻度で再取得する
* サイズ変更時にcanvasと出力バッファを再構築する
* 毎フレーム不要にallocationしない
* aspect ratioを維持する
* 描画を中央配置する
* resize後に画面をclearする
* panicしない

### 小さい端末

端末が小さすぎる場合は、アニメーションの代わりに中央へ次を表示してください。

```text
Knightty Demo

Terminal is too small.
Resize to at least 60 x 24.

Press q or Esc to exit.
```

最低サイズは実装したロゴとレイアウトに合わせて調整して構いません。

端末が再び十分な大きさになったら、アニメーションへ戻ってください。

---

## 14. フレームスケジューリング

デフォルトは60 FPSとしてください。

```text
--fps 60
```

次の指定を受け付けてください。

```bash
knightty-demo --fps 30
knightty-demo --fps 60
knightty-demo --fps 120
knightty-demo --fps 0
```

`--fps 0`はuncappedとします。

単純に各フレームで固定時間sleepすると、処理時間分のドリフトが累積します。

開始時刻から計算した絶対deadlineを使用してください。

概念:

```rust
deadline = start + frame_duration * frame_index;
```

現在時刻がdeadlineを超えている場合は、古いフレームを順番に全描画せず、現在時刻に対応するアニメーション状態へ進んでください。

ただし、rendered frame数とdropped frame相当数は計測してください。

sleep精度を補うための長時間busy spinは避けてください。

必要であれば、deadline直前の非常に短い区間のみyieldまたはspinを使用できますが、CPU使用率を不必要に上げないでください。

---

## 15. CLI

最低限、次のオプションを実装してください。

```text
--fps <number>
--duration <seconds>
--no-stats
--help
```

例:

```bash
cargo run -p knightty-demo --release -- --fps 60
```

```bash
cargo run -p knightty-demo --release -- --fps 120 --duration 10
```

### オプション仕様

#### `--fps`

* default: `60`
* `0`: uncapped
* 不正値は明確なエラー
* 異常に大きな値は適切に拒否または上限設定

#### `--duration`

* 指定なしの場合は手動終了までループ
* 指定した秒数が経過したら自動終了
* `0`や負数相当の入力を適切に処理

#### `--no-stats`

* 終了後の性能統計を表示しない

CLI parserは既存workspaceで使用しているものがあれば再利用してください。

この小規模CLIのためだけに巨大な依存関係を追加しないでください。

---

## 16. 出力方式

Phase 1では全画面再描画方式を実装してください。

毎フレーム:

1. encoder用バッファをclearする
2. cursor homeを追加する
3. canvasをhalf-blockへ変換する
4. ANSI出力を連続した1つのbufferへ構築する
5. stdoutへ可能な限り1回でwriteする
6. 1フレームにつき1回flushする

概念:

```rust
buffer.clear();
buffer.extend_from_slice(b"\x1b[H");
encoder.encode(&canvas, &mut buffer);
stdout.write_all(&buffer)?;
stdout.flush()?;
```

セルごとに`print!`や`println!`を呼ばないでください。

フレームごとに新しい`String`や`Vec`を作らず、容量を確保したbufferを再利用してください。

### 改行

端末下端でスクロールを発生させないでください。

次のいずれかを選択してください。

* 各行をcursor positioningで描画する
* autowrapを無効化する
* 右端1列を予約する
* 最終行にnewlineを出力しない

実装後、画面が下方向へ流れないことを手動確認してください。

---

## 17. 性能統計

終了後、alternate screenを復旧してから統計を表示してください。

最低限:

```text
Duration
Target FPS
Rendered frames
Estimated dropped frames
Bytes written
Average bytes/frame
Average encode time
p50 frame time
p95 frame time
p99 frame time
```

例:

```text
Knightty Demo Results
---------------------
Duration:           10.002 s
Target FPS:         60
Rendered frames:    598
Dropped frames:     2
Bytes written:      18.4 MiB
Average bytes/frame: 32.1 KiB
Encode time p50:    0.42 ms
Frame time p50:     16.67 ms
Frame time p95:     17.21 ms
Frame time p99:     20.84 ms
```

### 計測上の注意

* アニメーション画面内にFPS表示を重ねない
* 統計表示そのものを計測対象へ含めない
* terminal setup時間を描画時間へ含めない
* terminal restore後に結果を表示する
* percentile計算は外部statisticsライブラリなしで実装してよい
* サンプル数が少ない場合にもpanicしない
* uncapped時はTarget FPSを`uncapped`と表示する

計測用サンプルを無制限に保持しないでください。

今回の最大実行時間を事実上無制限とする場合は、一定数のring buffer、reservoir sampling、または集計方法を選択してください。

ただしPhase 1として過度に複雑にせず、妥当な上限付きbufferでも構いません。

---

## 18. エラー処理

次を適切なエラーとして扱ってください。

* stdoutがTTYではない
* terminal sizeを取得できない
* raw modeを開始できない
* stdout writeに失敗した
* 不正なCLI引数
* canvas allocationに失敗するほど異常な端末サイズ
* durationやfpsが非現実的な値

エラーメッセージは、terminal stateを復旧した後に表示してください。

通常のユーザー入力や端末リサイズでpanicしないでください。

---

## 19. テスト

TTYを必要としないheadless unit testを追加してください。

最低限必要なテスト:

### Canvas

* 正常な座標へset/getできる
* 範囲外setがpanicしない
* clearで全ピクセルが指定色になる
* circleの中心と外側が期待どおり
* lineの始点と終点が描画される
* polygonが代表的な内部点を塗りつぶす

### Half Block Encoder

* topとbottomが同色の場合
* topとbottomが異なる場合
* odd heightのcanvas
* 空canvasまたは最小canvas
* 同一色runで不要なSGRが増えない
* UTF-8の`▀`が正しく出力される

### Animation

代表的な時刻で決定的な結果になることを確認してください。

```text
t = 0.00
t = 0.20
t = 0.45
t = 0.60
t = 0.75
t = 0.95
```

各時刻について、次のいずれかを検証してください。

* palette別pixel count
* canvas hash
* 特定座標のpixel
* bounding box

単に「panicしなかった」だけのテストにしないでください。

次も検証してください。

* 同じ`t`から同じフレームが生成される
* `t = 0.0`と`1.0`付近でループが大きく破綻しない
* 騎士、斬撃、ロゴのkeyframeが互いに異なる
* 小さいcanvasでもpanicしない

### Timing helper

* segment正規化
* clamp
* easingの始点と終点
* durationからframe indexを計算する処理
* frame遅延時のskip計算

---

## 20. ドキュメント

`crates/demo/README.md`、または既存ドキュメント構成に適切な説明を追加してください。

最低限記載する内容:

* デモの目的
* 実行方法
* キー操作
* CLI option
* release buildを推奨する理由
* ベンチマーク結果は端末サイズ、OS、GPU、フォント、FPSに依存すること
* 既存作品の映像を含まないオリジナルアニメーションであること
* Phase 1はfull-frame rewriteのみであること
* 将来diff update等を追加する予定であること

READMEへ巨大なスクリーンショットや動画を追加しないでください。

---

## 21. 今回の非目標

今回、次は実装しないでください。

* FateやBad Apple!のフレーム変換
* GIF、MP4、WebMの読み込み
* ffmpeg連携
* Kitty Graphics Protocol
* Sixel
* iTerm2 image protocol
* Braille renderer
* ASCII濃淡renderer
* 差分セル更新
* 複数のアニメーションテーマ
* 設定ファイル
* 音声再生
* 本体GUIのメニュー項目
* Knightty本体への組み込みサブコマンド
* GPUタイムの直接取得
* renderer内部のinstrumentation
* 自動ベンチマーク比較
* CI上での実TTY integration test

将来拡張を阻害しない構造にはしてください。ただし、未使用の抽象化や過剰なtrait階層を先に作らないでください。

---

## 22. 実装品質

次を重視してください。

* 読みやすいRust
* 明確な責務分割
* 決定的なフレーム生成
* 端末状態の確実な復旧
* hot pathでのallocation削減
* cellごとのI/O回避
* クロスプラットフォーム性
* テスト可能性
* 将来のdiff renderer追加余地

避けるもの:

* `unsafe`の追加
* グローバルmutable state
* 毎フレームの巨大allocation
* 過剰なclone
* 描画ループ内のログ出力
* タイミング依存で不安定なunit test
* OS固有処理の無秩序な分岐
* terminal escape sequenceの各所への直書き

escape sequenceは定数またはterminal moduleへ集約してください。

---

## 23. 将来拡張を考慮する境界

Phase 1では実装しませんが、encoderは将来的に次を追加できる構造にしてください。

```rust
pub enum UpdateMode {
    Full,
    // Future:
    // Differential,
}
```

rendererも将来的に次を追加可能な構造を意識してください。

```rust
pub enum CellEncoding {
    HalfBlock,
    // Future:
    // Braille,
    // Ascii,
}
```

ただし、未使用variantを実際に追加してwarningを発生させる必要はありません。

現在必要なのはHalf Block + Full Rewriteだけです。

---

## 24. 手動確認

実装後、可能な環境で次を実行してください。

```bash
cargo run -p knightty-demo --release
```

```bash
cargo run -p knightty-demo --release -- --fps 30 --duration 10
```

```bash
cargo run -p knightty-demo --release -- --fps 60 --duration 10
```

```bash
cargo run -p knightty-demo --release -- --fps 120 --duration 10
```

確認項目:

* 月が表示される
* 騎士のシルエットと認識できる
* マントが自然に動く
* 抜刀が認識できる
* 斬撃が明確
* `KNIGHTTY`が読める
* 粒子化から自然にループする
* 画面下端でスクロールしない
* リサイズでpanicしない
* pause/resumeが機能する
* `q`で終了する
* Escapeで終了する
* 終了後にカーソルが復元される
* 終了後に通常画面へ戻る
* shellの表示属性が壊れない
* 統計がalternate screen退出後に表示される

Knightty内で実行できる場合は、Knightty内でも確認してください。

---

## 25. 検証コマンド

最低限、次を実行してください。

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets
cargo test --workspace
git diff --check
```

可能ならrelease buildも確認してください。

```bash
cargo build -p knightty-demo --release
```

既存workspaceにより厳しい検証コマンドがある場合は、それも実行してください。

警告やテスト失敗を残したまま完了扱いにしないでください。

---

## 26. 完了報告

実装完了後は、次の形式で報告してください。

### 実装内容

* 追加・変更したファイル
* アニメーション構成
* canvasとhalf-block encoderの構造
* terminal guardの構造
* frame schedulerの構造
* 性能統計の内容

### 実行方法

具体的なコマンドを記載してください。

### 検証結果

実行したコマンドと結果を記載してください。

### 手動確認結果

実際に確認できた項目と、環境上確認できなかった項目を分けてください。

### 残っている制限

Phase 1で未実装のものを明記してください。

---

## 27. 受け入れ条件

以下をすべて満たした場合に完了とします。

* `knightty-demo`がworkspace内でビルドできる
* 外部画像や著作物を使用していない
* アニメーションがRustコードから生成される
* 月、騎士、抜刀、斬撃、ロゴ、粒子化が存在する
* 約8秒で自然にループする
* Half Blockによる縦2倍解像度描画を使用している
* ANSI True Colorを使用している
* 1セルごとにstdout writeしていない
* 出力bufferをフレーム間で再利用している
* デフォルト60 FPSで動作する
* 30/60/120/uncappedを指定できる
* `q`とEscapeで終了できる
* Spaceでpause/resumeできる
* terminal resizeへ対応している
* 小さい端末でpanicしない
* 終了後に端末状態が復旧する
* 終了後に性能統計が表示される
* headless unit testが追加されている
* `cargo fmt`が成功する
* `cargo clippy`が成功する
* `cargo test`が成功する
* `git diff --check`でwhitespace errorがない

[1]: https://pkg.go.dev/github.com/ashish0kumar/gostty?utm_source=chatgpt.com "gostty command - github.com/ashish0kumar ..."
[2]: https://invisible-island.net/xterm/ctlseqs/ctlseqs.html?utm_source=chatgpt.com "ctlseqs(ms)"

---

## Phase 1.5 Visual Direction Notes

Phase 1.5 keeps the Phase 1 terminal workload intact: full-frame Half Block output, ANSI True Color, raw-mode controls, alternate-screen restoration, resize handling, scheduler, and final metrics stay in place.

The visual layer now uses authored Knightty SVG key poses for the knight and `KNIGHTTY` wordmark. SVG sources are converted during development into `crates/demo/assets/generated/animation.kfa`, and the runtime includes that binary asset with `include_bytes!`. The release demo must not parse SVG or decode images at runtime.

Asset regeneration:

```bash
cargo run -p xtask -- demo-assets build
```

Preview generation:

```bash
cargo run -p xtask -- demo-assets preview
```

When changing assets, verify the converter rejects unknown colors and external references, regenerate `animation.kfa`, confirm `git diff --exit-code -- crates/demo/assets/generated`, and run the normal Rust checks. Performance comparisons should use the same terminal size, FPS, and duration, then compare rendered frames, dropped frames, bytes written, average bytes/frame, encode time percentiles, frame time percentiles, and release build behavior.

## Phase 1.6 Flowing Cape Notes

Phase 1.6 keeps the same benchmark surface: full-frame Half Block output, ANSI True Color, raw-mode controls, alternate-screen restoration, resize handling, scheduler, and final metrics. The visual asset pipeline now writes KFA2. Body and logo frames remain pre-rasterized, while the cape is stored as six fixed-topology vector layers and morphed deterministically at runtime.

Asset commands:

```bash
cargo run -p xtask -- demo-assets build
cargo run -p xtask -- demo-assets preview
```

The source SVG viewBox is `0 0 320 180`; the runtime reference canvas remains `160 x 90`. Runtime code does not parse SVG, decode PNG/GIF/video, or allocate large cape buffers in the animation loop. The preview command writes cape, character, terminal, 80 x 45, and Half Block-style PNGs under `target/knightty-demo-preview/`.

The cape animation is original Knightty-authored vector work. Reference images may be used only for abstract motion ideas such as large cloth arcs, delayed tips, and foreground/background layering. Do not copy or trace characters, costume details, silhouettes, colors, weapons, UI, logos, fonts, or composition from existing works.
