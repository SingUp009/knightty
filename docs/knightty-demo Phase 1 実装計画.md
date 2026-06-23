# knightty-demo Phase 1 実装計画

## Summary
`docs/performance-demo.md`に従い、Knightty本体とは独立したRustバイナリcrate `crates/demo`を追加します。既存の`core`/`render`/`app`にはデモ固有コードを入れず、workspace登録とlock更新だけを最小変更にします。

## Key Changes
- `crates/demo`を追加し、package名は`knightty-demo`、実行は`cargo run -p knightty-demo --release -- --fps 60`にするs。
- CLIは既存方針に合わせて手書きparserにし、`--fps`、`--duration`、`--no-stats`、`--help`を実装する。`--fps 0`はuncapped、上限は`240`として異常値を拒否する。
- 依存は`thiserror = "2.0.18"`と、raw mode/input/resize/terminal size用の小型依存`crossterm`に限定する。`clap`や画像/統計ライブラリは追加しない。
- `Canvas`、ラスタライズ、half-block encoder、terminal guard、player loop、metrics、animationを分離する。`TerminalGuard`はalternate screen/raw mode/cursor/autowrapを復旧し、通常終了、`q`、Esc、Ctrl+C相当、描画エラーで端末状態を戻す。
- アニメーションは8秒ループで、月、騎士登場、抜刀、斬撃、`KNIGHTTY` bitmap logo、粒子化を時間`t`から決定的に生成する。外部画像、動画、巨大フレーム列は使わない。
- encoderはlogical height = terminal rows * 2、width = cols.saturating_sub(1)を基本にし、ANSI True Colorと`▀`でrun単位のSGR圧縮を行う。stdout writeは1フレーム1回、flushも1回にする。
- `crates/demo/README.md`に目的、実行方法、キー操作、CLI option、release推奨理由、Phase 1制限、将来diff update予定を書く。

## Test Plan
- Headless unit testを追加する。
  - Canvas: set/get、範囲外set、clear、circle、line、polygon。
  - Encoder: same-color、top/bottom差分、odd height、最小/空相当、SGR run圧縮、UTF-8 `▀`。
  - Animation: `t = 0.00, 0.20, 0.45, 0.60, 0.75, 0.95`のhash/count/bounding box、同時刻決定性、小canvas非panic、keyframe差分。
  - Timing: clamp、segment、easing、frame skip計算。
- 検証コマンドは実装後に実行する。
  - `cargo fmt --all -- --check`
  - `cargo clippy --workspace --all-targets`
  - `cargo test --workspace`
  - `git diff --check`
  - `cargo build -p knightty-demo --release`
- TTYが使える環境では`--fps 30/60/120`と`--duration 10`、pause/resume、resize、`q`/Esc終了、端末復旧、統計表示を手動確認する。TTY不可ならその項目は未確認として報告する。

## Assumptions
- 指示書は`AGENTS.md`確認となっているが、現ツリーには`AGENT.md`のみ存在するため、`AGENT.md`を正として扱う。
- 既存ワークツリーには未コミット変更があるため、実装ではそれらを戻さず、必要な`Cargo.toml`/`Cargo.lock`変更は現在状態の上に最小追加する。
- `core`、`render`、`app`は原則変更しない。workspace member追加に必要なroot `Cargo.toml`とlock更新、新規`crates/demo`配下だけを触る。
