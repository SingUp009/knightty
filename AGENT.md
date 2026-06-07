# AGENT.md

このリポジトリで作業するコーディングエージェント向けのガイドです。**ここに書かれた技術選定は調査済みの確定事項**であり、明確な根拠なしに蒸し返さないこと。判断を覆してよい条件は各項目に明記してある。

---

## 1. プロジェクト概要

RustでモダンなGPU描画ターミナルエミュレータを作る。

- **最優先事項**: UX と実行速度。この2つがトレードオフの判断基準になる。
- **ターゲットプラットフォーム**: Linux と Windows。macOS は対象外(将来加える可能性はあるが現時点で最適化しない)。
- **主要ユースケース**: neovim / lazygit / ssh などの TUI アプリが快適に動くこと。これが互換性要件の基準。
- **描画方式**: GPU レンダリング前提。
- **言語方針**: **特別な理由がない限り Rust のみを使う**。ビルドを単純に保つため、C/C++ への FFI や `*-sys` 系の重いネイティブ依存(Skia、HarfBuzz の C バインディング等)は避け、純 Rust のクレートを優先する。例外を入れる場合は本ファイルに理由を明記すること。

---

## 2. アーキテクチャ

レイヤごとに分けた Cargo ワークスペース。**依存の向きは一方向**で、下位ほど GUI 非依存にする。

```
app → {render, pty, proto, core}
render → core
proto → core
core → (末端、GUI非依存)
```

| クレート | 役割 | GUI依存 | 主要 dep |
|---|---|---|---|
| `core` | VT状態・グリッド・スクロールバック・ダメージ管理 | なし | `alacritty_terminal`, `vte` |
| `pty` | PTY/ConPTY 抽象 + IOスレッド | なし | `portable-pty` |
| `render` | wgpu 描画・グリフアトラス・シェイピング | なし(winit非依存) | `wgpu`, `cosmic-text` |
| `proto` | OSC/CSI 拡張(OSC 8/133, Kitty各種) | なし | — |
| `app` | バイナリ。winit で全層を束ねる | あり | 上記全部 + `winit` |

`core` を末端に保つことで、単体テストと vtebench 相当のベンチを GUI なしで回せる。

### スレッドモデル

`winit` のイベントループはメインスレッドを占有する。したがって:

- **PTY read スレッド**と **wgpu render スレッド**は `app` で spawn する。
- スレッド → メインの通知は `winit` の `EventLoopProxy` 経由。
- read スレッドは PTY イベントごとに描画要求を出さず、**約4msのバッチング間隔**で複数イベントを合体させる(これをしないと大量出力で固まる。実証済みの落とし穴)。

---

## 3. 技術選定と根拠(確定事項)

### GPU API: `wgpu`
ターゲットが Linux+Windows なので、Ghostty の強み(macOS raw Metal)は不要。wgpu 単一実装で Windows(DX12/Vulkan)と Linux(Vulkan/GLES)を賄え、保守コストが最小。WezTerm・Rio が本番実証済み。古いハードでも DX11/Vulkan フォールバックが効く。
**覆してよい条件**: macOS を最重要ターゲットに加え、かつ最低レイテンシが死活的になった場合のみ、ネイティブ Metal の二系統化を検討する。Web 版を出すなら wgpu は唯一解。

### VTパース+グリッド: `alacritty_terminal`
車輪の再発明をしない。Zed が本番採用し、`crates.io` で独立利用可能。グリッド・スクロールバック・モードを含む全状態を GUI 非依存で持つ。Rio も同系の vte を流用。
**覆してよい条件**: 原則なし。独自 VT パーサのフルスクラッチは禁止(下記参照)。

### PTY抽象: `portable-pty`(WezTerm由来)
Unix/Linux はネイティブ PTY、Windows は ConPTY を単一トレイトで抽象化。Rust 製クロスプラットフォームターミナルでは実質一択。

### テキスト: `cosmic-text`(+ 必要なら `glyphon`)
フォント発見・フォールバック・シェイピング・レイアウト・ラスタライズを一括提供。Linux/macOS/Windows フルサポート、bidi 対応、ブラウザ由来のフォールバックリストを再利用。**純 Rust**(内部は harfrust + swash + fontdb)であり、言語方針に合致する。HarfBuzz の C バインディングや Skia は使わない。wgpu と組むなら `glyphon` でアトラス管理ごと任せる選択肢もある。

### ウィンドウ: `winit`
クロスプラットフォーム標準。イベントループがメインスレッドを占有する点に注意(スレッドモデル参照)。

> 上記の選定はいずれも純 Rust クレートであり、ビルドに C/C++ ツールチェインを要求しない。この性質を維持すること。

---

## 4. ビルド・実行・テスト

```bash
# ビルド
cargo build --workspace
cargo build --release            # 性能計測は必ず release で

# 実行
cargo run -p app

# テスト(core は GUI 非依存なので CI で回せる)
cargo test -p core
cargo test --workspace

# 計測(リリースビルドのバイナリに対して)
time cat <large_file>            # IOスループット
# vtebench 相当のスループットベンチを core に用意する
```

性能の数値はすべて release ビルドで取る。debug の数値は意味を持たない。

### テスト方針

**大原則: テストコードは第一級のドキュメントである。** テストを読んだ人が、そのAPIの使い方・期待される振る舞い・対応するエスケープシーケンスを理解できることを最優先にする。検証の正しさだけでなく、**読み物としての価値**で評価する。この原則から以下が導かれる:

- **実利用と同じ書き方で呼ぶ**: テストの呼び出し側は、本物の利用コードと同じ idiomatic な形にする。テスト専用のスタブ・モック・内部いじりで「呼び出し方を歪めない」。読んだ人がそのままコピーして使える形が理想。結果として、本物の API に実バイト列を流し、観測可能な状態(グリッド)を assert する形になる。
- **名前が仕様を語る**: テスト名は挙動を文章で述べる。`test_csi` ではなく `csi_truecolor_fg_sets_cell_color` のように「何を入れると何が起きるか」を名前にする。テスト一覧がそのまま対応機能の仕様書になる。
- **粒度を細かく**: 1テスト=1挙動。`CSI 2 J`(画面クリア)、`CSI 38;2;R;G;B m`(truecolor)、`CSI 4:3 m`(undercurl)…のように、シーケンス1つ・状態遷移1つごとに分ける。エッジケース(空入力、不正パラメータ、途中で切れたシーケンス、UTF-8 境界)も独立したテストにし、それぞれが「この入力はこう扱う」という仕様書の1行になるようにする。
- **ノイズを減らす**: Arrange→Act→Assert を素直に並べ、テスト用の足場を最小化する。読んだときに「準備・実行・期待結果」が一目で分かること。
- **公開APIには doctest を使う**: 公開関数のドキュメントコメント内のコード例(` ```rust `)はコンパイル・実行される。**ドキュメントとテストが物理的に同一**になるので、第一級ドキュメントの原則を最も直接に満たす。`core` の公開 API は doctest で使用例を示す。
- **層をまたがない**: `core` のテストに wgpu や winit を持ち込まない。GUI 非依存なので「バイト列 → グリッド状態」だけで完結させ、CI でも高速に回す。
- ユニットテストは各クレートに `#[cfg(test)]` で、層をまたぐ結合は `tests/`(統合テスト)に置く。

doctest(ドキュメント兼テスト)の例 — 公開APIの使い方をそのまま示す:

```rust
/// バイト列をターミナルに流し込み、グリッド状態を更新する。
///
/// truecolor の前景色指定はセルに反映される:
/// ```
/// # use myterm_core::{Term, Color, Point};
/// let mut term = Term::new(80, 24);
/// term.feed(b"\x1b[38;2;255;0;0mX");
/// assert_eq!(term.grid()[Point::new(0, 0)].fg, Color::Rgb(255, 0, 0));
/// ```
pub fn feed(&mut self, bytes: &[u8]) { /* ... */ }
```

ユニットテストの例 — 名前が仕様を語り、実利用と同じ経路で呼ぶ:

```rust
#[test]
fn csi_truecolor_split_across_two_feeds_still_parses() {
    // シーケンスが途中で切れても状態機械が継続することを、実利用と同じ feed 経路で検証
    let mut term = Term::new(80, 24);
    term.feed(b"\x1b[38;2;255;");   // 途中まで
    term.feed(b"0;0mX");            // 残り
    assert_eq!(term.grid()[Point::new(0, 0)].fg, Color::Rgb(255, 0, 0));
}
```

(API 名は `alacritty_terminal` の実際のものに合わせる。要点は「テストを読めば使い方と仕様が分かる」こと。)

---

## 5. ドキュメントサイトと設定リファレンス(Astro)

Knightty のユーザー向けドキュメントサイトは **Astro** で作る。目的は、Ghostty の config reference のように、ユーザーが設定可能な項目を一覧し、説明・型・デフォルト値・記述例・注意事項を確認できる静的ドキュメントを提供すること。

ただし、Ghostty の UI や実装をそのまま模倣するのではなく、Knightty の設定体系を **構造化メタデータから生成する**。手書きページを増やして実装と乖離させないこと。

### 配置

Astro サイトは以下に置く。

```txt
docs/site
```

生成済み設定リファレンス JSON は以下に置く。

```txt
docs/generated/config-reference.json
```

`docs/generated/config-reference.json` がまだ存在しない初期段階のみ、Astro 側に fixture を置いてよい。

```txt
docs/site/src/data/config-reference.fixture.json
```

fixture は開発用の仮データであり、恒久的な source of truth にしてはいけない。

### 設定リファレンスの source of truth

設定項目の正は Rust 側の設定メタデータに置く。Astro 側で設定項目名・デフォルト値・説明を重複管理しない。

推奨構成:

```txt
crates/app/src/config.rs              # 実際の設定構造・読み込み
crates/app/src/config_spec.rs         # ドキュメント生成用メタデータ
docs/generated/config-reference.json  # xtask で生成
docs/site                             # Astro サイト
```

生成コマンドは以下を目標にする。

```bash
cargo run -p xtask -- generate-config-docs
```

Astro 側は `docs/generated/config-reference.json` を優先して読み、存在しない場合だけ fixture を使う。

### 設定項目のデータ shape

Astro 側では以下の TypeScript 型を基準にする。

```ts
type ConfigOption = {
  key: string;
  category: string;
  type: string;
  default: string | number | boolean | string[] | null;
  description: string;
  examples: string[];
  validValues?: string[];
  reload: "runtime" | "new-terminal" | "restart";
  platform: "all" | "windows" | "linux" | "macos";
  security?: string;
  since?: string;
  deprecated?: boolean;
};
```

Rust 側の `config_spec.rs` でも、概念的に同じ情報を保持する。

各設定項目は最低限、以下を持つこと。

- `key`: ユーザーが設定ファイルに書く名前
- `category`: 表示グルーピング
- `type`: `bool` / `int` / `float` / `string` / `list<string>` / `enum` / `color` など
- `default`: 未設定時の値
- `description`: ユーザー向け説明
- `examples`: コピペ可能な設定例。複数可
- `validValues`: enum や allowlist がある場合
- `reload`: 反映タイミング
- `platform`: 対象プラットフォーム
- `security`: URL open、clipboard、shell 起動など安全性に関わる注意
- `since`: 追加バージョン
- `deprecated`: 非推奨設定かどうか

### Astro ページ要件

設定リファレンスページは以下の URL に作る。

```txt
/config/reference/
```

最低限必要な UI:

- タイトルと短い説明
- 検索 input
- category filter
- reload behavior filter
- platform filter
- 左側ナビゲーション(category ごと)
- 設定項目カード
- 各設定項目への anchor link
- `type` 表示
- `default` 表示
- `validValues` 表示
- `examples` 表示
- `security` note 表示
- `since` 表示
- `deprecated` badge 表示

検索・フィルタ以外のために client-side JavaScript を増やさない。ドキュメントサイトとして、基本は静的 HTML を優先する。

### Astro 実装方針

- Astro + TypeScript を使う。
- React / Vue / Svelte などの UI framework は入れない。
- コンポーネントライブラリは入れない。必要になるまで素の Astro component と CSS で作る。
- CSS は CSS variables を使い、terminal-like で compact な見た目にする。
- dark theme first でよい。
- 大きなアニメーションは不要。
- `key` から URL anchor 用 slug を安全に生成する。
- データが空でもページを壊さない。
- `examples` が複数ある場合はすべて表示する。
- `security` がある設定は通常の note より目立たせる。

推奨構成:

```txt
docs/site/
  astro.config.mjs
  package.json
  tsconfig.json
  src/
    data/
      config-reference.fixture.json
    lib/
      config-reference.ts
      slug.ts
    layouts/
      DocsLayout.astro
    pages/
      index.astro
      config/
        reference.astro
    components/
      ConfigReferencePage.astro
      ConfigOptionCard.astro
      ConfigFilters.astro
      ConfigSidebar.astro
    styles/
      global.css
```

### Astro の検証コマンド

`docs/site` 内では `pnpm` を使う。

```bash
cd docs/site
pnpm install
pnpm astro check
pnpm build
```

依存が未導入・workspace 未整備などでコマンドが失敗する場合は、失敗内容を明記し、コードを中途半端に壊した状態で終えない。

### 設定追加時の必須手順

新しいユーザー設定を追加する場合は、実装と同じ PR/作業単位で以下を行う。

1. `config.rs` または既存の設定読み込み箇所に実設定を追加する。
2. parser に設定 key を追加する。
3. `config_spec.rs` に説明・型・デフォルト値・例・反映タイミングを追加する。
4. `cargo run -p xtask -- generate-config-docs` で `docs/generated/config-reference.json` を更新する。
5. Astro の `/config/reference/` で表示が崩れないことを確認する。
6. parser key と config spec key の一致をテストする。

parser に存在する key が config spec に存在しない、または config spec に存在する key が parser で解釈できない状態を許さない。

### 設定リファレンスで特に注意する項目

以下のような設定は、必ず `security` note を付ける。

- OSC 8 hyperlink を外部ブラウザで開く設定
- 許可する URL scheme
- OSC 52 clipboard
- shell integration
- 外部コマンド起動
- SSH/terminfo 自動配布
- ローカルファイルパスや作業ディレクトリを外部へ渡す機能

安全性に関わる設定では、便利さよりも明示性を優先する。allowlist・無効化方法・デフォルト挙動を必ず書く。

### Codex / coding agent への作業完了報告

ドキュメントサイト関連の作業を終えたら、最後に以下を報告する。

- 変更したファイル一覧
- 実装した UI / 生成処理
- fixture から generated JSON に切り替える方法
- 実行したコマンド
- 失敗したコマンドがあれば、その理由
- 残 TODO

---

## 6. コーディング規約

- 静的型付けの利点を活かす。`unwrap()` はプロトタイプ以外で避け、エラーは型で表現する。
- `core` には GUI・wgpu・winit を一切持ち込まない(依存の向きを壊さない)。
- 新しいエスケープシーケンス対応は `proto` に閉じ込め、`core` のグリッドへは最小の状態だけ渡す。
- ダメージトラッキングと同期更新は**最初から `core` の設計に入れる**(後付けは困難)。
- ホットパス(パース・描画ループ)にアロケーションを増やさない。グリフは個別キャッシュ。
- **依存追加は純 Rust を優先**。新しいクレートを足す前に、C/C++ ビルドや `*-sys` を引き込まないか確認する。引き込む場合は本ファイルに理由を残す。
- テストは**第一級のドキュメント**として書く(セクション4のテスト方針を参照)。新しい挙動を足したら、読めば使い方が分かる単体テスト(または公開APIなら doctest)を必ず添える。
- ユーザー設定を追加・変更したら、同じ作業単位で `config_spec.rs` と `docs/generated/config-reference.json` を更新する。設定仕様とドキュメントの乖離を残さない。

---

## 7. やってはいけないこと(重要)

調査で明確に「アンチパターン」と分かっているもの。提案・実装する前に必ず確認すること。

1. **独自 VT パーサのフルスクラッチ**。`alacritty_terminal` / `vte` で十分。ここに時間を使わない。
2. **リガチャの早期実装**。monospace セルグリッドとリガチャは本質的に相性が悪く、描画単位を `RenderableCell` から複数セルの `TextRun` へ変える=コア Grid のリファクタが必要で性能リスクが大きい。Alacritty は「1フレーム落とす価値もない」として意図的に非サポートにした。**性能はワーストケースで評価される**。フェーズ4以降に回し、入れるなら無効時は shaping を呼ばない設計にする。
3. **tmux 前提のアーキテクチャ**。ネイティブ分割を持つのに tmux をタブ/分割目的で使うと、二重 VT エミュレーション(tmux の内蔵 VTE が再パース・再発行)で GPU 高速化の利点を打ち消す。
4. **PTY イベントごとの即時描画**。大量出力で固まる。必ずバッチングする。
5. **debug ビルドでの性能判断**。
6. **理由のない C/C++ 依存の追加**。Skia、HarfBuzz の C バインディング、その他 `*-sys` でビルドを複雑にしない。純 Rust の代替を先に探す(例: シェイピングは harfbuzz C ではなく cosmic-text/rustybuzz)。
7. **テストを伴わない挙動追加**。新しいエスケープシーケンスや状態遷移を足したら、それ単体の細かいテストを実バイト列で必ず書く。
8. **Astro 側への設定リファレンス手書き固定**。設定名・デフォルト値・説明を Astro component に直接埋め込まない。初期 fixture を除き、Rust 側メタデータから生成された JSON を使う。
9. **ドキュメントサイトの過剰 SPA 化**。検索・フィルタ以外のために client-side JavaScript を増やさない。React/Vue/Svelte 等の導入も明示指示なしに行わない。

---

## 8. 実装フェーズ

| フェーズ | 内容 | 完了条件 |
|---|---|---|
| 0 | 技術選定の確定 | 本ファイルの選定で確定済み |
| 1 | コア | `core`+`pty`+最小 `app` で「文字が出るだけ」が動く |
| 2 | 描画 | グリフアトラス+キャッシュ、cosmic-text シェイピング、ダメージトラッキング、同期更新(DECSET 2026) |
| 3 | UX/互換性 | truecolor → undercurl(CSI 4:3 + 58色)+ 独自terminfo → OSC 133 → OSC 8 → Kittyキーボード → Kittyグラフィックス → ssh時terminfo自動配布 |
| 4 | マルチプレクサ・最適化 | ネイティブタブ/分割、read/render/io スレッド分離、入力遅延チューニング |
| Docs | 設定リファレンス | Rust config metadata → JSON → Astro `/config/reference/` の生成・表示が動く |

### フェーズ3の互換性チェックリスト(neovim/lazygit/ssh向け)
- [ ] トゥルーカラー `CSI 38;2;R;G;B m`
- [ ] undercurl `CSI 4:3 m` / 下線色 `CSI 58:2:R:G:B m` / リセット `CSI 59 m`
- [ ] undercurl/truecolor capability を持つ**独自 terminfo**(TERM を `xterm-256color` で上書きさせない)
- [ ] OSC 133 セマンティックプロンプト(A/B/C/D;exitcode)
- [ ] OSC 8 ハイパーリンク
- [ ] OSC 7 作業ディレクトリ報告
- [ ] Kitty キーボードプロトコル
- [ ] Kitty グラフィックスプロトコル(画像)
- [ ] ssh 時の terminfo 自動配布(Ghostty `ssh-terminfo` 方式)

### 性能目標(閾値)
- `time cat` 大ファイル: Alacritty(404ms級)に対し **2倍以内**
- メモリ: WezTerm(130MB)より軽く、Alacritty(75MB)に近づける
- 入力遅延: Alacritty(ソフトウェア計測 6.9ms)に対抗

※ベンチ数値は単一環境由来でハード依存が大きい。絶対値より「参照実装との相対」で評価する。スループット(`time cat`)とレイテンシ(キーボード→画面)は別物なので混同しない。

---

## 9. 参考リソース

- Alacritty / WezTerm / Ghostty / Rio の DeepWiki(アーキテクチャ解説)
- Mitchell Hashimoto の Ghostty Devlog(005: terminal inspector、006: SIMDパーサ)
- Warp「Adventures in Text Rendering」
- kitty graphics-protocol / performance ドキュメント
- 設計の詳細根拠はリポジトリ内の Research Report を参照

---

## 10. 設計の前提・注意

- **Ghostty は Zig 製**。設計思想は参考になるがコードは直接流用できない。`libghostty-vt` は将来 C 互換ライブラリとして FFI 利用できる可能性がある(現時点では未確定)。
- **Ghostty の Windows 対応は未実装**(ロードマップ上)。Windows の参照実装は WezTerm / Rio を見る。
- **CJK・入力メソッド**は platform 差が大きい(カーソル位置・候補ウィンドウ・合成状態)。Windows ConPTY と Unix PTY の差はシグナル処理・プロセスライフサイクルで顕在化する。早めに実機で確認する。
