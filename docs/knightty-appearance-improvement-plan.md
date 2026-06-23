# Knightty Appearance Improvement Plan

## 目的

Knightty の外観を段階的に改善し、開発中のモチベーションを上げる。
最初は **Catppuccin テーマ、半透明、padding、cursor style、selection color** のような低リスクな項目から入り、後続フェーズで light/dark theme、hover/search/split/tab の視覚表現、最後に blur / acrylic / mica / gradient / background image など OS・GPU・compositor 依存が強い機能を扱う。

参考方針:

* Ghostty は `theme`、`background-opacity`、`window-padding-x/y`、`cursor-style`、`background-blur` などを設定項目として持つ。Ghostty の `background-opacity` は `0..=1` に clamp され、`cursor-style` は `block` / `bar` / `underline` / `block_hollow` を扱う。([Ghostty][1])
* kitty は `background_opacity`、`window_padding_width`、`cursor_shape`、`tab_bar_style`、`background_image` などを持ち、見た目設定の幅が広い。([Kovid's Software Projects][2])
* WezTerm は `window_background_opacity`、`window_padding`、inactive pane の HSB 調整、background image、gradient などを持つ。透明化は compositor support に依存し、`window_background_opacity != 1.0` は render performance に影響し得る。([WezTerm][3])
* Windows Terminal は profile ごとの `opacity`、`useAcrylic`、`backgroundImage`、`padding`、cursor shape、experimental retro/pixel shader 系の設定を持つ。([Microsoft Learn][4])
* Catppuccin は Latte / Frappé / Macchiato / Mocha の 4 flavor を持ち、Mocha は最も暗い variant。([Catppuccin][5])

---

## 全体方針

### 優先順位

1. **Phase 1: Low-risk visual boost**

   * Built-in Catppuccin Mocha
   * Configurable colors
   * Window opacity
   * Window padding
   * Cursor style / blink
   * Selection color

2. **Phase 2: Polished terminal UX**

   * Catppuccin 4 flavor 対応
   * light/dark theme pair
   * hyperlink hover style
   * search highlight colors
   * unfocused window / split dimming
   * dynamic config reload の土台

3. **Phase 3: Showcase visuals**

   * background gradient
   * background image
   * blur
   * Windows Acrylic / Mica
   * tab bar styling
   * optional retro shader / CRT shader

### 実装原則

* Phase 1 では **blur / acrylic / mica / background image / shader は実装しない**。
* テーマは renderer 内に直書きせず、config resolution 後の `ResolvedTheme` のような構造に集約する。
* 透明化は readability と performance に影響するため、default は `opacity = 1.0` のままにする。
* unknown theme は panic させず、明示的 error または safe fallback にする。
* renderer と terminal state を分離し、ANSI color palette と UI accent colors を混同しない。
* color parsing / opacity clamp / padding clamp / theme lookup は unit test で固定する。

---

# Phase 1: Low-risk visual boost

## ゴール

Knightty を短時間で見栄えよくする。
まずは **Catppuccin Mocha + opacity + padding + cursor + selection** を実装する。

推奨初期見た目:

```toml
theme = "Catppuccin Mocha"

[window]
opacity = 0.90
padding_x = 12
padding_y = 10

[cursor]
style = "bar"
blink = true
```

---

## Non-goals

Phase 1 では以下を実装しない。

* background blur
* Windows Acrylic / Mica
* macOS vibrancy / material
* background image
* background gradient
* tab bar
* split pane UI
* shader effects
* theme auto switching
* config hot reload

---

## 設定仕様

### `theme`

```toml
theme = "Catppuccin Mocha"
```

要件:

* `theme: Option<String>` を config に追加する。
* `None` の場合は既存の default theme を使う。
* `"Catppuccin Mocha"` を built-in theme として認識する。
* theme name は最初は case-sensitive でよい。
* unknown theme は以下のどちらかにする。

  * 起動時に明示的 error
  * warning log を出して default theme fallback

推奨は **warning + fallback**。開発中の設定ミスで起動不能になると体験が悪い。

---

### `[window]`

```toml
[window]
opacity = 1.0
padding_x = 0
padding_y = 0
```

要件:

* `opacity: f32`

  * default: `1.0`
  * valid range: `0.0..=1.0`
  * 範囲外は clamp する
* `padding_x: u32`

  * default: `0`
  * 左右同一 padding
* `padding_y: u32`

  * default: `0`
  * 上下同一 padding

補足:

* Ghostty の `background-opacity` も `0..=1` の opacity として扱われ、範囲外は clamp される。([Ghostty][1])
* Ghostty の `window-padding-x/y` は terminal cells と window border の間の padding である。([Ghostty][1])
* Windows Terminal も window padding を appearance profile setting として持つ。([Microsoft Learn][4])

---

### `[cursor]`

```toml
[cursor]
style = "block"
blink = true
```

要件:

```rust
enum CursorStyle {
    Block,
    Bar,
    Underline,
    HollowBlock,
}
```

設定値:

```toml
"block"
"bar"
"underline"
"hollow_block"
```

要件:

* `style` default: `"block"`
* `blink` default: `true`
* renderer 側で style ごとに描画を分ける。
* 既存の terminal escape sequence による cursor style override がある場合、最終的には application-requested style を優先する。ただし Phase 1 では config default だけでも可。

参考:

* Ghostty は `block` / `bar` / `underline` / `block_hollow` を cursor style として持つ。([Ghostty][1])
* kitty も `block` / `beam` / `underline` を cursor shape として持つ。([Kovid's Software Projects][2])
* Windows Terminal も cursor shape を appearance setting として持つ。([Microsoft Learn][4])

---

### `[colors]`

```toml
[colors]
background = "#1e1e2e"
foreground = "#cdd6f4"
selection_background = "#45475a"
selection_foreground = "#cdd6f4"
cursor = "#f5e0dc"
cursor_text = "#1e1e2e"

[colors.normal]
black = "#45475a"
red = "#f38ba8"
green = "#a6e3a1"
yellow = "#f9e2af"
blue = "#89b4fa"
magenta = "#f5c2e7"
cyan = "#94e2d5"
white = "#bac2de"

[colors.bright]
black = "#585b70"
red = "#f38ba8"
green = "#a6e3a1"
yellow = "#f9e2af"
blue = "#89b4fa"
magenta = "#f5c2e7"
cyan = "#94e2d5"
white = "#a6adc8"
```

要件:

* `Color` は `#RRGGBB` を parse できること。
* Phase 1 では alpha 付き `#RRGGBBAA` は不要。
* `normal[8]` / `bright[8]` を ANSI 16 色 palette に対応させる。
* `colors` の明示指定は built-in theme より優先する。
* `selection_background` / `selection_foreground` は selection rendering に使う。
* `cursor` / `cursor_text` は cursor rendering に使う。

Catppuccin Mocha の代表色:

| Role      | Hex       |
| --------- | --------- |
| base      | `#1e1e2e` |
| mantle    | `#181825` |
| crust     | `#11111b` |
| text      | `#cdd6f4` |
| surface1  | `#45475a` |
| rosewater | `#f5e0dc` |
| red       | `#f38ba8` |
| green     | `#a6e3a1` |
| yellow    | `#f9e2af` |
| blue      | `#89b4fa` |
| pink      | `#f5c2e7` |
| teal      | `#94e2d5` |

Catppuccin Mocha は dark variant として使いやすく、base / mantle / crust のような背景段階を持つ。([Catppuccin][5])

---

## 実装タスク

### 1. Config model を拡張

対象候補:

* `crates/app/src/config.rs`
* 必要に応じて `crates/core/src/lib.rs`
* 必要に応じて `crates/render/src/lib.rs`

追加する構造体例:

```rust
#[derive(Debug, Clone)]
pub struct AppearanceConfig {
    pub theme: Option<String>,
    pub colors: ColorConfig,
    pub window: WindowAppearanceConfig,
    pub cursor: CursorConfig,
}

#[derive(Debug, Clone)]
pub struct WindowAppearanceConfig {
    pub opacity: f32,
    pub padding_x: u32,
    pub padding_y: u32,
}

#[derive(Debug, Clone)]
pub struct CursorConfig {
    pub style: CursorStyle,
    pub blink: bool,
}
```

---

### 2. Theme resolution を追加

目的:

* config parse 後に `ResolvedTheme` を作る。
* renderer は config raw values ではなく `ResolvedTheme` を参照する。

構造体例:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RgbColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

#[derive(Debug, Clone)]
pub struct ResolvedTheme {
    pub background: RgbColor,
    pub foreground: RgbColor,
    pub selection_background: RgbColor,
    pub selection_foreground: RgbColor,
    pub cursor: RgbColor,
    pub cursor_text: RgbColor,
    pub normal: [RgbColor; 8],
    pub bright: [RgbColor; 8],
}
```

要件:

* `builtin_theme("Catppuccin Mocha") -> Option<ResolvedTheme>`
* `resolve_theme(config) -> ResolvedTheme`
* user-defined colors は built-in theme を override する。
* unknown theme は warning + default fallback。

---

### 3. Renderer に theme を反映

要件:

* clear color に `theme.background` を使う。
* default cell foreground に `theme.foreground` を使う。
* default cell background に `theme.background` を使う。
* ANSI 0〜15 に `theme.normal` / `theme.bright` を使う。
* selection rectangle に `theme.selection_background` を使う。
* selected text foreground に `theme.selection_foreground` を使う。
* cursor body に `theme.cursor` を使う。
* cursor 上の text に `theme.cursor_text` を使う。

---

### 4. Padding を grid origin に反映

要件:

* grid origin を `padding_x`, `padding_y` 分だけ内側にずらす。
* viewport size から padding 分を差し引いて columns / rows を計算する。
* padding が大きすぎる場合でも panic しない。
* usable width / height が 0 以下になる場合は minimum grid size に丸めるか、warning を出して描画を skip する。

実装上の注意:

```text
usable_width  = window_width  - padding_left - padding_right
usable_height = window_height - padding_top  - padding_bottom
```

Phase 1 では左右・上下同値でよい。

---

### 5. Window opacity を反映

要件:

* `opacity < 1.0` の場合、window transparent flag を有効化する。
* renderer clear color の alpha に `opacity` を反映する。
* `opacity == 1.0` の場合は従来どおり不透明。
* OS blur は扱わない。

注意:

* WezTerm は `window_background_opacity` を `0.0..=1.0` の alpha として扱い、compositing support がある環境で背景透過を実現する。`1.0` 以外は render performance に影響し得る。([WezTerm][3])
* 透明化は OS / compositor / GPU backend に依存するため、失敗しても terminal 機能を壊さないこと。

---

## Phase 1 tests

### Unit tests

* `opacity` が `0.0..=1.0` に clamp される。
* `#RRGGBB` が parse できる。
* invalid color string は error になる。
* `"Catppuccin Mocha"` の theme lookup が成功する。
* Catppuccin Mocha の主要色が期待値と一致する。

  * background: `#1e1e2e`
  * foreground: `#cdd6f4`
  * cursor: `#f5e0dc`
* unknown theme の fallback / error behavior が固定されている。
* `CursorStyle` parse:

  * `block`
  * `bar`
  * `underline`
  * `hollow_block`

### Rendering tests

* padding が grid origin に反映される。
* selection color が theme から取得される。
* cursor color が theme から取得される。
* ANSI color index 0〜15 が theme palette に対応する。

### Manual smoke test

```bash
cargo run -p app
```

確認項目:

* background が Catppuccin Mocha になる。
* text が読みやすい。
* cursor が bar style で表示される。
* selection が見える。
* padding が左右上下に反映される。
* opacity 0.90 で背面が少し見える。
* opacity 1.0 に戻すと完全不透明になる。

---

## Phase 1 acceptance criteria

* `cargo fmt --all -- --check` が通る。
* `cargo clippy --workspace --all-targets` が通る。
* `cargo test --workspace` が通る。
* Catppuccin Mocha を config から指定できる。
* opacity / padding / cursor style / selection color が config から変更できる。
* unknown theme で panic しない。
* 既存の keyboard / hyperlink / PTY routing test を壊さない。

---

# Phase 2: Polished terminal UX

## ゴール

Phase 1 の外観を「常用できる品質」に上げる。
テーマ flavor、light/dark 切り替え、hover/search/focus/split の視覚表現を整える。

---

## Non-goals

Phase 2 では以下を原則扱わない。

* OS blur
* Windows Acrylic / Mica
* background image
* shader effects
* 完全な tab/split 実装

ただし、split rendering が既に存在する場合は inactive split/pane dimming のみ扱ってよい。

---

## 追加設定仕様

### Catppuccin 4 flavor

```toml
theme = "Catppuccin Mocha"
```

対応 theme:

```text
Catppuccin Latte
Catppuccin Frappe
Catppuccin Macchiato
Catppuccin Mocha
```

要件:

* built-in theme registry に 4 flavor を追加する。
* default は Mocha のままでよい。
* flavor ごとに `ResolvedTheme` を持つ。
* ANSI 16 色 mapping を flavor ごとに定義する。
* 各 flavor の主要色を snapshot / unit test で固定する。

Catppuccin は 1 light variant と 3 dark variants を持つため、light/dark 切り替えの基盤に使いやすい。([Catppuccin][5])

---

### Light/dark theme pair

```toml
[theme]
light = "Catppuccin Latte"
dark = "Catppuccin Mocha"
mode = "system" # "system" | "light" | "dark"
```

要件:

* Phase 1 の `theme = "..."` との互換性を維持する。
* `mode = "light"` の場合は `light` を使う。
* `mode = "dark"` の場合は `dark` を使う。
* `mode = "system"` は OS theme detection が可能な場合のみ system に追従する。
* OS theme detection が未実装または unavailable の場合は `dark` fallback でよい。

参考:

* Ghostty は `theme` で light/dark theme pair を指定でき、desktop environment theme に応じて使用 theme を変える。([Ghostty][1])
* WezTerm も system appearance に応じた color scheme 選択例を案内している。([WezTerm][3])

---

### Hyperlink hover style

```toml
[hyperlink]
hover_underline = true
hover_foreground = "#89b4fa"
hover_background = "#313244"
```

要件:

* hover 中の link に underline を付ける。
* hover 中の foreground / background を theme から指定可能にする。
* link hit-test の既存実装を壊さない。
* disallowed scheme のリンクは hover style を出してよいが、open はしない。
* pointer cursor との整合性を保つ。

---

### Search highlight colors

```toml
[search]
foreground = "#1e1e2e"
background = "#f9e2af"
selected_foreground = "#1e1e2e"
selected_background = "#fab387"
```

要件:

* 通常 search match と current match を分ける。
* selection と search highlight が重なった場合の優先順位を決める。

推奨優先順位:

```text
cursor > selection > current_search_match > search_match > cell_style > default_theme
```

Ghostty も search foreground/background と selected search foreground/background を設定項目として持つ。([Ghostty][1])

---

### Unfocused window / pane dimming

```toml
[window]
unfocused_opacity = 0.92

[panes]
inactive_opacity = 0.75
inactive_tint = "#181825"
```

要件:

* OS window が unfocused のとき、必要なら terminal 全体を少し dim する。
* split/pane がある場合、inactive pane に dim overlay をかける。
* Phase 2 では HSB 変換までは不要。単純な alpha overlay でよい。
* split/pane が未実装なら config schema と renderer utility のみでもよい。

参考:

* Ghostty は unfocused split opacity / fill / divider color を持つ。([Ghostty][1])
* WezTerm は inactive pane を見分けやすくするため HSB multiplier による dim / desaturation を持つ。([WezTerm][3])
* kitty は inactive text alpha や active/inactive border color を持つ。([Kovid's Software Projects][2])

---

### Dynamic config reload の土台

要件:

* Phase 2 では完全な hot reload でなくてよい。
* `AppearanceConfig` と `ResolvedTheme` の差し替え境界を明確にする。
* renderer が次フレームから新しい `ResolvedTheme` を参照できる構造にする。
* window recreation が必要な設定と runtime update 可能な設定を分類する。

分類例:

| 設定           | runtime update | 備考                    |
| ------------ | -------------: | --------------------- |
| colors       |            yes | renderer state 差し替え   |
| cursor style |            yes | terminal override に注意 |
| padding      |          maybe | grid resize が必要       |
| opacity      |          maybe | window/surface 側の都合あり |
| blur         |             no | Phase 3               |
| backdrop     |             no | Phase 3               |

---

## Phase 2 tasks

### 1. Built-in theme registry

```rust
pub struct BuiltinTheme {
    pub name: &'static str,
    pub flavor: Option<CatppuccinFlavor>,
    pub theme: ResolvedTheme,
}
```

要件:

* theme name list を取得できるようにする。
* error message に available themes を出せるようにする。
* `knightty --list-themes` のような CLI は任意。

---

### 2. Theme pair resolution

```rust
pub enum ThemeMode {
    System,
    Light,
    Dark,
}

pub struct ThemePairConfig {
    pub light: String,
    pub dark: String,
    pub mode: ThemeMode,
}
```

要件:

* 旧形式 `theme = "Catppuccin Mocha"` を維持する。
* 新形式 `[theme]` がある場合は新形式を優先する。
* system mode が取れない場合は dark fallback。

---

### 3. Render layer priority を整理

要件:

* selection / search / hover / cursor の描画優先順位を固定する。
* できれば `CellVisual` または `ResolvedCellStyle` のような中間表現を作る。
* render code 内に if 分岐を散らさない。

例:

```rust
pub struct CellVisual {
    pub fg: RgbColor,
    pub bg: RgbColor,
    pub underline: bool,
    pub inverse: bool,
}
```

---

### 4. Hyperlink hover style

要件:

* hovered hyperlink range に underline を付ける。
* hover foreground/background を theme-aware にする。
* Ctrl+Left open の既存 smoke test を壊さない。
* hover 中の pointer cursor は維持する。

---

### 5. Search highlight style

要件:

* search result range に highlight を適用する。
* current match と other matches を区別する。
* selection との競合を test する。

---

### 6. Focus / inactive dimming

要件:

* window focus event を renderer state に反映する。
* unfocused overlay を任意で描画する。
* split/pane が存在する場合、inactive pane overlay を描画する。
* split/pane が存在しない場合は future-facing config として残してよい。

---

## Phase 2 tests

### Unit tests

* Catppuccin 4 flavor lookup。
* light/dark pair resolution。
* system mode fallback。
* user color override が built-in theme より優先される。
* search highlight priority。
* hyperlink hover style priority。
* unknown theme error message に available themes が含まれる。

### Integration / smoke tests

* theme 切り替え後も ANSI 16 色が崩れない。
* selection と search highlight が重なっても readable。
* hyperlink hover と selection が重なっても panic しない。
* unfocused state で dimming が有効になる。
* default config では従来と大きく挙動が変わらない。

---

## Phase 2 acceptance criteria

* Catppuccin 4 flavor を選択できる。
* light/dark theme pair の config が parse できる。
* system theme が取れない環境でも fallback する。
* hyperlink hover が視覚的に分かる。
* search highlight の通常 match / current match が区別できる。
* inactive dimming が optional に動く。
* Phase 1 の設定互換性を壊さない。
* `cargo fmt --all -- --check` が通る。
* `cargo clippy --workspace --all-targets` が通る。
* `cargo test --workspace` が通る。

---

# Phase 3: Showcase visuals

## ゴール

Knightty の外観を「見せたくなる」レベルにする。
ただし、このフェーズは OS / compositor / GPU backend 依存が増えるため、feature flag と fallback を重視する。

---

## Non-goals

Phase 3 でも以下は避ける。

* terminal correctness を犠牲にした視覚効果
* 常時高負荷な shader
* 読めない background image preset
* platform-specific code の無秩序な増殖
* unsupported environment での hard failure

---

## 追加設定仕様

### Background gradient

```toml
[background]
kind = "gradient"

[background.gradient]
orientation = "vertical" # "vertical" | "horizontal"
colors = ["#1e1e2e", "#181825", "#11111b"]
```

要件:

* `kind = "solid"` を default にする。
* gradient は renderer 内で背景 quad として描画する。
* 最初は vertical / horizontal のみ。
* cell background が default background の場合、gradient を見せる。
* explicit cell background がある場合はその色を優先する。

参考:

* WezTerm は window background gradient を appearance feature として持つ。([WezTerm][3])

---

### Background image

```toml
[background]
kind = "image"

[background.image]
path = "wallpapers/knightty.png"
opacity = 0.25
fit = "cover" # "contain" | "cover" | "stretch" | "tile" | "center"
tint = "#1e1e2e"
tint_opacity = 0.60
```

要件:

* 画像 path は config file からの相対 path を許可する。
* 対応形式は最初は PNG のみでよい。
* opacity を必ず指定可能にする。
* tint overlay を入れて可読性を確保する。
* 画像 load error では warning + solid background fallback。
* 画像は resize / cache し、毎フレーム decode しない。

参考:

* kitty は background image、layout、tint を持つ。([Kovid's Software Projects][2])
* WezTerm は background image と HSB transform を持ち、大きな画像は performance / VRAM に影響し得るとしている。([WezTerm][3])
* Windows Terminal も background image、alignment、stretch mode、opacity を profile appearance setting として持つ。([Microsoft Learn][4])

---

### Blur

```toml
[window]
blur = false
blur_radius = 20
```

要件:

* default は `blur = false`。
* `opacity < 1.0` のときのみ有効。
* unsupported environment では warning + no blur fallback。
* Linux / Windows / macOS で backend を分離する。
* blur のために renderer correctness を壊さない。

参考:

* Ghostty の `background-blur` は `true` で default intensity 20 相当になり、高い blur intensity は rendering / performance issue を起こし得る。macOS と一部 Linux desktop environment に対応する。([Ghostty][1])
* kitty の blur も platform / compositor support に依存する。([Kovid's Software Projects][2])
* WezTerm は macOS で `macos_window_background_blur` を `window_background_opacity` と組み合わせて translucent effect に使う。([WezTerm][6])

---

### Windows Acrylic / Mica

```toml
[window.windows]
backdrop = "none" # "none" | "acrylic" | "mica" | "tabbed"
```

要件:

* Windows 専用設定として分離する。
* non-Windows では parse はしても無視、または warning にする。
* `opacity < 1.0` と組み合わせる。
* unsupported Windows version では no-op fallback。
* platform abstraction を作り、renderer に Windows API details を入れない。

参考:

* WezTerm の `win32_system_backdrop` は Windows backdrop effect を扱い、効果を出すには `window_background_opacity < 1.0` が必要。([WezTerm][7])
* Windows Terminal は `opacity` と `useAcrylic` を profile appearance setting として持ち、Mica は terminal opacity が `<100` のとき terminal content に表示されると説明されている。([Microsoft Learn][4])

---

### Tab bar styling

```toml
[tabs]
enabled = true
show_when_single = false
style = "minimal" # "minimal" | "separator" | "powerline" | "slant"
active_background = "#313244"
active_foreground = "#cdd6f4"
inactive_background = "#181825"
inactive_foreground = "#7f849c"
```

要件:

* tabs 実装が未完成なら config schema のみでもよい。
* tab bar renderer は terminal grid renderer と分離する。
* 最初は `minimal` のみ実装してよい。
* powerline / slant は後続でよい。

参考:

* kitty は `fade` / `slant` / `separator` / `powerline` などの tab bar style を持つ。([Kovid's Software Projects][2])
* WezTerm は tab bar の active/inactive/new tab/hover color を細かく設定できる。([WezTerm][3])
* Windows Terminal も tab width、tab row acrylic、title bar 表示などの appearance setting を持つ。([Microsoft Learn][8])

---

### Optional retro shader / CRT shader

```toml
[effects]
retro_crt = false
scanlines = false
```

要件:

* default false。
* feature flag の下に置く。
* shader compile failure は warning + disabled fallback。
* text readability を落としすぎない。
* test / screenshots の再現性を壊さないため、CI では無効にする。

参考:

* Windows Terminal は retro terminal effect と pixel shader path を experimental feature として持ち、継続保証のない機能として扱われている。([Microsoft Learn][4])

---

## Phase 3 tasks

### 1. Platform appearance abstraction

```rust
pub trait PlatformAppearance {
    fn set_window_opacity(&self, opacity: f32);
    fn set_blur(&self, enabled: bool, radius: u32) -> Result<(), AppearanceError>;
    fn set_backdrop(&self, backdrop: WindowBackdrop) -> Result<(), AppearanceError>;
}
```

要件:

* platform-specific code を `app` 側に閉じ込める。
* renderer crate に OS API details を入れない。
* unsupported feature は error ではなく capability として扱う。

---

### 2. Background renderer を分離

```rust
pub enum BackgroundKind {
    Solid(RgbColor),
    Gradient(GradientBackground),
    Image(ImageBackground),
}
```

要件:

* background pass と cell pass を分ける。
* solid background は既存 path を維持。
* gradient / image は optional pass。
* default cell background の透明性を扱えるようにする。

---

### 3. Image asset loading

要件:

* config path から image を読み込む。
* decode は起動時または config reload 時のみ。
* GPU texture を cache する。
* load failure では fallback。
* 画像が大きすぎる場合は warning。

---

### 4. Blur / backdrop backend

要件:

* Windows:

  * backdrop: none / acrylic / mica / tabbed
* macOS:

  * blur radius
* Linux:

  * compositor support がある場合のみ blur
  * unsupported なら no-op

注意:

* Linux blur は compositor 差が大きい。
* Hyprland / KDE / GNOME / X11 / Wayland で挙動が分かれる可能性がある。
* 最初は Windows backdrop か macOS blur のように、実装しやすい platform から入る。

---

### 5. Visual regression snapshots

要件:

* theme / background / cursor / selection の snapshot を作る。
* CI では platform-dependent effect を無効化する。
* screenshot test は deterministic な solid background / fixed font / fixed DPI で行う。

---

## Phase 3 tests

### Unit tests

* gradient config parse。
* background image config parse。
* unsupported backdrop fallback。
* blur radius clamp。
* effect flags default false。
* background kind resolution。

### Rendering tests

* solid background path が従来どおり動く。
* gradient background が clear color と衝突しない。
* image background opacity / tint が反映される。
* default cell background では background が見える。
* explicit cell background は background image / gradient より優先される。

### Platform smoke tests

Windows:

* `backdrop = "none"` で通常表示。
* `backdrop = "mica"` で失敗しても起動継続。
* `opacity = 1.0` では backdrop を無理に有効化しない。
* `opacity = 0.90` で透明化が破綻しない。

macOS:

* blur off で通常表示。
* blur on で失敗しても起動継続。
* native fullscreen で挙動が破綻しない。

Linux:

* compositor 非対応で warning + fallback。
* Wayland / X11 の差で panic しない。

---

## Phase 3 acceptance criteria

* gradient background を設定できる。
* background image を設定できる。
* image load failure で fallback する。
* blur / backdrop が unsupported environment で terminal 起動を妨げない。
* tab style config が parse できる。
* shader / retro effects は default disabled。
* CI では platform-dependent effect が無効。
* `cargo fmt --all -- --check` が通る。
* `cargo clippy --workspace --all-targets` が通る。
* `cargo test --workspace` が通る。

---

# 推奨実装順

## Step 1

```text
Phase 1 のみ実装する。
```

理由:

* もっとも短時間で見た目が変わる。
* renderer / config / color pipeline の基盤になる。
* OS 依存が少ない。

---

## Step 2

```text
Phase 2 の Catppuccin 4 flavor と hover/search colors を実装する。
```

理由:

* 常用時の質が上がる。
* terminal emulator らしい polish が出る。
* link / search / selection / cursor の描画優先順位が整理される。

---

## Step 3

```text
Phase 3 の gradient を先に実装し、その後 image、最後に blur/backdrop を実装する。
```

理由:

* gradient は renderer 内で完結しやすい。
* image は asset loading と GPU texture 管理が必要。
* blur / acrylic / mica は OS 依存が最も強い。

---

# Codex 指示例

## Phase 1 指示

```text
Knightty の外観改善 Phase 1 を実装してください。

目的:
- Catppuccin Mocha を built-in theme として追加する
- window opacity / padding / cursor style / selection color を config から変更できるようにする
- blur / acrylic / mica / background image / gradient / tab bar は実装しない

要件:
1. config.rs に appearance 関連設定を追加する
   - theme: Option<String>
   - window.opacity: f32, default 1.0, clamp 0.0..=1.0
   - window.padding_x: u32, default 0
   - window.padding_y: u32, default 0
   - cursor.style: block | bar | underline | hollow_block
   - cursor.blink: bool, default true
   - colors.background / foreground / selection_background / selection_foreground / cursor / cursor_text
   - colors.normal 8 色
   - colors.bright 8 色

2. built-in theme として Catppuccin Mocha を追加する
   - background #1e1e2e
   - foreground #cdd6f4
   - cursor #f5e0dc
   - cursor_text #1e1e2e
   - selection_background #45475a
   - selection_foreground #cdd6f4
   - ANSI 16 色は Catppuccin Mocha palette を使う

3. theme resolution を追加する
   - ResolvedTheme を作る
   - renderer は ResolvedTheme を参照する
   - user-defined colors は built-in theme より優先する
   - unknown theme は warning + default fallback、または明示的 error にする

4. renderer に反映する
   - clear color に theme background を使う
   - default foreground/background に theme を反映する
   - ANSI 0..15 に normal/bright palette を反映する
   - selection / cursor color を theme から描画する
   - padding 分だけ grid origin を内側にずらす

5. window opacity を反映する
   - opacity < 1.0 の場合は transparent window を有効にする
   - renderer clear alpha に opacity を反映する
   - OS blur は実装しない

6. tests を追加する
   - opacity clamp
   - color parser
   - theme lookup
   - Catppuccin Mocha palette values
   - cursor style parser
   - padding が grid origin に反映されること
   - unknown theme behavior

検証:
- cargo fmt --all -- --check
- cargo clippy --workspace --all-targets
- cargo test --workspace
```

---

## Phase 2 指示

```text
Knightty の外観改善 Phase 2 を実装してください。

目的:
- Catppuccin 4 flavor を built-in theme として追加する
- light/dark theme pair の config を追加する
- hyperlink hover / search highlight / inactive dimming の見た目を整える
- Phase 1 の config 互換性を維持する

要件:
1. built-in theme registry を追加する
   - Catppuccin Latte
   - Catppuccin Frappe
   - Catppuccin Macchiato
   - Catppuccin Mocha
   - available theme list を取得できるようにする

2. theme config を拡張する
   - 旧形式 theme = "Catppuccin Mocha" を維持する
   - 新形式 [theme] light/dark/mode を追加する
   - mode は system | light | dark
   - system mode が取得できない場合は dark fallback

3. hyperlink hover style を追加する
   - hover_underline: bool
   - hover_foreground
   - hover_background
   - existing hyperlink open behavior を壊さない

4. search highlight colors を追加する
   - foreground
   - background
   - selected_foreground
   - selected_background
   - current match と other matches を区別する

5. render priority を整理する
   - cursor > selection > current_search_match > search_match > hyperlink_hover > cell_style > default_theme
   - 可能なら CellVisual / ResolvedCellStyle のような中間表現にまとめる

6. inactive dimming を追加する
   - window.unfocused_opacity
   - panes.inactive_opacity
   - panes.inactive_tint
   - split/pane 未実装の場合は future-facing config と renderer utility のみでよい

7. tests を追加する
   - Catppuccin 4 flavor lookup
   - light/dark pair resolution
   - system fallback
   - user color override priority
   - hyperlink hover priority
   - search highlight priority
   - selection/search/cursor overlap
   - Phase 1 config compatibility

検証:
- cargo fmt --all -- --check
- cargo clippy --workspace --all-targets
- cargo test --workspace
```

---

## Phase 3 指示

```text
Knightty の外観改善 Phase 3 を実装してください。

目的:
- background gradient / background image / blur / Windows backdrop / tab styling の土台を追加する
- unsupported environment では warning + fallback にする
- terminal correctness と readability を優先する

要件:
1. background config を追加する
   - kind = solid | gradient | image
   - solid は既存 background
   - gradient は vertical / horizontal と colors[]
   - image は path / opacity / fit / tint / tint_opacity

2. background renderer を分離する
   - background pass と cell pass を分ける
   - default cell background では background を見せる
   - explicit cell background は background より優先する

3. gradient を実装する
   - vertical / horizontal のみでよい
   - color stops は最低 2 色
   - invalid config は fallback

4. background image を実装する
   - 最初は PNG のみでよい
   - config file からの相対 path を許可する
   - 起動時または config reload 時のみ decode する
   - GPU texture を cache する
   - load failure は warning + solid fallback
   - opacity / tint を反映する

5. platform appearance abstraction を追加する
   - window opacity
   - blur
   - backdrop
   - platform-specific code を renderer crate に入れない

6. blur を追加する
   - default false
   - opacity < 1.0 のときのみ有効
   - unsupported environment では warning + no-op
   - blur_radius を clamp する

7. Windows backdrop を追加する
   - none | acrylic | mica | tabbed
   - Windows 以外では no-op または warning
   - unsupported Windows version では fallback

8. tab style config を追加する
   - enabled
   - show_when_single
   - style = minimal | separator | powerline | slant
   - 最初は minimal のみ実装でよい

9. optional effects を追加する
   - retro_crt
   - scanlines
   - default false
   - CI では無効
   - shader compile failure は warning + disabled fallback

10. tests を追加する
   - gradient config parse
   - image config parse
   - blur radius clamp
   - unsupported backdrop fallback
   - solid background compatibility
   - explicit cell background priority
   - image load failure fallback

検証:
- cargo fmt --all -- --check
- cargo clippy --workspace --all-targets
- cargo test --workspace
```

---

# 最初に採用するプリセット

```toml
theme = "Catppuccin Mocha"

[window]
opacity = 0.90
padding_x = 12
padding_y = 10

[cursor]
style = "bar"
blink = true

[colors]
background = "#1e1e2e"
foreground = "#cdd6f4"
selection_background = "#45475a"
selection_foreground = "#cdd6f4"
cursor = "#f5e0dc"
cursor_text = "#1e1e2e"
```

この preset を Phase 1 の manual smoke test で使う。
まずはこの見た目を完成させ、そのあと Phase 2 で polish、Phase 3 で派手な効果へ進む。

[1]: https://ghostty.org/docs/config/reference "Option Reference - Configuration"
[2]: https://sw.kovidgoyal.net/kitty/conf/?utm_source=chatgpt.com "kitty.conf - Kovid's software projects"
[3]: https://wezterm.org/config/appearance.html "Colors & Appearance - Wez's Terminal Emulator"
[4]: https://learn.microsoft.com/de-de/windows/terminal/customize-settings/profile-appearance "Windows-Terminal: Darstellungsprofileinstellungen | Microsoft Learn"
[5]: https://catppuccin.com/palette/?utm_source=chatgpt.com "Palette"
[6]: https://wezterm.org/config/lua/config/macos_window_background_blur.html?utm_source=chatgpt.com "macos_window_background_blur - Wez's Terminal Emulator"
[7]: https://wezterm.org/config/lua/config/win32_system_backdrop.html?utm_source=chatgpt.com "win32_system_backdrop - Wez's Terminal Emulator"
[8]: https://learn.microsoft.com/en-us/windows/terminal/customize-settings/appearance "Windows Terminal Appearance Settings | Microsoft Learn"
