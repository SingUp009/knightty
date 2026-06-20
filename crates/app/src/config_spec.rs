use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::{Value, json};

use crate::config::{
    AppConfig, DEFAULT_HYPERLINK_ALLOWED_SCHEMES, DEFAULT_INITIAL_COLS, DEFAULT_INITIAL_ROWS,
    DEFAULT_SCROLL_MULTIPLIER, DEFAULT_SCROLLBACK_LINES, MAX_SCROLL_MULTIPLIER,
    MAX_SCROLLBACK_LINES,
};

const SINCE_VERSION: &str = "0.1.0";

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ConfigOption {
    pub key: &'static str,
    pub category: &'static str,
    #[serde(rename = "type")]
    pub value_type: &'static str,
    pub default: Value,
    pub description: &'static str,
    pub examples: Vec<String>,
    #[serde(rename = "validValues", skip_serializing_if = "Option::is_none")]
    pub valid_values: Option<Vec<&'static str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub range: Option<String>,
    pub reload: ReloadBehavior,
    pub platform: Platform,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub security: Option<&'static str>,
    pub since: &'static str,
    #[serde(skip_serializing_if = "is_false")]
    pub deprecated: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReloadBehavior {
    Runtime,
    NewTerminal,
    Restart,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    All,
    Windows,
    Linux,
    Macos,
}

pub fn config_options() -> Vec<ConfigOption> {
    let default_config = AppConfig::default();
    let renderer_config = default_config.renderer_config();
    let mut options = vec![
        ConfigOption {
            key: "font.family",
            category: "Font",
            value_type: "string",
            default: Value::Null,
            description: "Renderer に渡すフォントファミリー名。未指定時はレンダラーの既定フォントを使います。",
            examples: vec![toml_example(
                "font",
                &[("family", "\"CaskaydiaCove Nerd Font\"")],
            )],
            valid_values: None,
            range: None,
            reload: ReloadBehavior::Restart,
            platform: Platform::All,
            security: None,
            since: SINCE_VERSION,
            deprecated: false,
        },
        ConfigOption {
            key: "font.size",
            category: "Font",
            value_type: "float",
            default: json!(renderer_config.cell_metrics.font_size),
            description: "セルメトリクスの基準になるフォントサイズ。0 より大きい有限値を指定します。",
            examples: vec![toml_example("font", &[("size", "18.0")])],
            valid_values: None,
            range: Some("finite > 0".to_owned()),
            reload: ReloadBehavior::Restart,
            platform: Platform::All,
            security: None,
            since: SINCE_VERSION,
            deprecated: false,
        },
        ConfigOption {
            key: "font.line_height",
            category: "Font",
            value_type: "float",
            default: Value::Null,
            description: "セルの行高。未指定時はフォントサイズに合わせたレンダラー既定値を使います。",
            examples: vec![toml_example("font", &[("line_height", "22.0")])],
            valid_values: None,
            range: Some("finite > 0".to_owned()),
            reload: ReloadBehavior::Restart,
            platform: Platform::All,
            security: None,
            since: SINCE_VERSION,
            deprecated: false,
        },
        ConfigOption {
            key: "window.initial_cols",
            category: "Window",
            value_type: "int",
            default: json!(default_config.initial_cols()),
            description: "起動時に作成する端末グリッドの列数。0 より大きい値を指定します。",
            examples: vec![toml_example("window", &[("initial_cols", "120")])],
            valid_values: None,
            range: Some("1..".to_owned()),
            reload: ReloadBehavior::Restart,
            platform: Platform::All,
            security: None,
            since: SINCE_VERSION,
            deprecated: false,
        },
        ConfigOption {
            key: "window.initial_rows",
            category: "Window",
            value_type: "int",
            default: json!(default_config.initial_rows()),
            description: "起動時に作成する端末グリッドの行数。0 より大きい値を指定します。",
            examples: vec![toml_example("window", &[("initial_rows", "36")])],
            valid_values: None,
            range: Some("1..".to_owned()),
            reload: ReloadBehavior::Restart,
            platform: Platform::All,
            security: None,
            since: SINCE_VERSION,
            deprecated: false,
        },
        ConfigOption {
            key: "render.wgpu_backend",
            category: "Render",
            value_type: "enum",
            default: json!("auto"),
            description: "wgpu の描画バックエンド。環境変数 KNIGHTTY_WGPU_BACKEND がある場合はそちらが優先されます。",
            examples: vec![toml_example("render", &[("wgpu_backend", "\"gl\"")])],
            valid_values: Some(vec!["auto", "vulkan", "dx12", "gl"]),
            range: None,
            reload: ReloadBehavior::Restart,
            platform: Platform::All,
            security: None,
            since: SINCE_VERSION,
            deprecated: false,
        },
        ConfigOption {
            key: "terminal.scrollback_lines",
            category: "Terminal",
            value_type: "int",
            default: json!(default_config.scrollback_lines()),
            description: "保持するスクロールバック行数。0 から 100000 までの値を指定できます。",
            examples: vec![toml_example("terminal", &[("scrollback_lines", "20000")])],
            valid_values: None,
            range: Some(format!("0..={MAX_SCROLLBACK_LINES}")),
            reload: ReloadBehavior::Restart,
            platform: Platform::All,
            security: None,
            since: SINCE_VERSION,
            deprecated: false,
        },
        ConfigOption {
            key: "terminal.scroll_multiplier",
            category: "Terminal",
            value_type: "int",
            default: json!(default_config.scroll_multiplier()),
            description: "通常スクロール時に 1 wheel step あたり移動する行数。1 から 100 までの値を指定できます。",
            examples: vec![toml_example("terminal", &[("scroll_multiplier", "5")])],
            valid_values: None,
            range: Some(format!("1..={MAX_SCROLL_MULTIPLIER}")),
            reload: ReloadBehavior::Restart,
            platform: Platform::All,
            security: None,
            since: SINCE_VERSION,
            deprecated: false,
        },
        ConfigOption {
            key: "shell.program",
            category: "Shell",
            value_type: "string",
            default: Value::Null,
            description: "起動する shell program。未指定時は platform の既定 shell を使います。",
            examples: vec![toml_example("shell", &[("program", "\"pwsh\"")])],
            valid_values: None,
            range: Some("non-empty when set".to_owned()),
            reload: ReloadBehavior::NewTerminal,
            platform: Platform::All,
            security: Some(
                "外部コマンド起動につながるため、信頼できる shell program だけを指定してください。",
            ),
            since: SINCE_VERSION,
            deprecated: false,
        },
        ConfigOption {
            key: "shell.args",
            category: "Shell",
            value_type: "list<string>",
            default: json!(default_config.shell.args.clone()),
            description: "shell program に渡す引数の配列。",
            examples: vec![toml_example(
                "shell",
                &[("program", "\"pwsh\""), ("args", "[\"-NoLogo\"]")],
            )],
            valid_values: None,
            range: None,
            reload: ReloadBehavior::NewTerminal,
            platform: Platform::All,
            security: Some(
                "shell program に渡されるため、引数には信頼できる値だけを指定してください。",
            ),
            since: SINCE_VERSION,
            deprecated: false,
        },
        ConfigOption {
            key: "hyperlink.open_on_ctrl_click",
            category: "Hyperlink",
            value_type: "bool",
            default: json!(default_config.hyperlink_open_on_ctrl_click()),
            description: "Ctrl click で OSC 8 hyperlink を開くかどうか。",
            examples: vec![toml_example(
                "hyperlink",
                &[("open_on_ctrl_click", "false")],
            )],
            valid_values: None,
            range: None,
            reload: ReloadBehavior::Restart,
            platform: Platform::All,
            security: Some(
                "有効にすると許可された scheme の URL を OS の既定ハンドラーで開きます。",
            ),
            since: SINCE_VERSION,
            deprecated: false,
        },
        ConfigOption {
            key: "hyperlink.allowed_schemes",
            category: "Hyperlink",
            value_type: "list<string>",
            default: json!(default_config.hyperlink_allowed_schemes()),
            description: "Ctrl click で開くことを許可する URL scheme。空配列にすると hyperlink open は無効になります。",
            examples: vec![toml_example(
                "hyperlink",
                &[("allowed_schemes", "[\"https\"]")],
            )],
            valid_values: None,
            range: None,
            reload: ReloadBehavior::Restart,
            platform: Platform::All,
            security: Some(
                "許可した scheme は外部アプリケーション起動につながるため、必要な scheme のみに絞ってください。",
            ),
            since: SINCE_VERSION,
            deprecated: false,
        },
        ConfigOption {
            key: "hyperlink.underline_on_hover",
            category: "Hyperlink",
            value_type: "bool",
            default: json!(default_config.hyperlink_underline_on_hover()),
            description: "hover 中の hyperlink に underline 表示を付けるかどうか。",
            examples: vec![toml_example(
                "hyperlink",
                &[("underline_on_hover", "false")],
            )],
            valid_values: None,
            range: None,
            reload: ReloadBehavior::Restart,
            platform: Platform::All,
            security: None,
            since: SINCE_VERSION,
            deprecated: false,
        },
    ];

    options.extend(graphics_config_options(&default_config));
    options.extend(appearance_config_options(&default_config));
    options.sort_by_key(|option| option.key);
    options
}

fn graphics_config_options(default_config: &AppConfig) -> Vec<ConfigOption> {
    let graphics = &default_config.graphics;
    let security =
        Some("端末アプリケーションから受信する画像データに対するメモリ使用量の上限です。");
    vec![
        option(
            "graphics.enabled",
            "Graphics",
            "bool",
            json!(graphics.enabled),
            "iTerm2 OSC 1337 inline PNG image rendering を有効にします。",
            vec![toml_example("graphics", &[("enabled", "false")])],
            None,
            None,
            ReloadBehavior::Runtime,
            Platform::All,
            Some("無効にすると OSC 1337 payload はデコードせず安全に破棄されます。"),
        ),
        option(
            "graphics.max_encoded_bytes",
            "Graphics",
            "int",
            json!(graphics.max_encoded_bytes),
            "Base64 文字列と復号後 PNG データに許可する最大バイト数。",
            vec![toml_example(
                "graphics",
                &[("max_encoded_bytes", "16777216")],
            )],
            None,
            Some("1..".to_owned()),
            ReloadBehavior::Runtime,
            Platform::All,
            security,
        ),
        option(
            "graphics.max_decoded_bytes",
            "Graphics",
            "int",
            json!(graphics.max_decoded_bytes),
            "PNG を RGBA8 へ展開した後に許可する最大バイト数。",
            vec![toml_example(
                "graphics",
                &[("max_decoded_bytes", "134217728")],
            )],
            None,
            Some("1..".to_owned()),
            ReloadBehavior::Runtime,
            Platform::All,
            security,
        ),
        option(
            "graphics.max_width",
            "Graphics",
            "int",
            json!(graphics.max_width),
            "デコードを許可する PNG の最大幅。",
            vec![toml_example("graphics", &[("max_width", "8192")])],
            None,
            Some("1..".to_owned()),
            ReloadBehavior::Runtime,
            Platform::All,
            security,
        ),
        option(
            "graphics.max_height",
            "Graphics",
            "int",
            json!(graphics.max_height),
            "デコードを許可する PNG の最大高さ。",
            vec![toml_example("graphics", &[("max_height", "8192")])],
            None,
            Some("1..".to_owned()),
            ReloadBehavior::Runtime,
            Platform::All,
            security,
        ),
        option(
            "graphics.max_pixels",
            "Graphics",
            "int",
            json!(graphics.max_pixels),
            "デコードを許可する PNG の最大総ピクセル数。",
            vec![toml_example("graphics", &[("max_pixels", "32000000")])],
            None,
            Some("1..".to_owned()),
            ReloadBehavior::Runtime,
            Platform::All,
            security,
        ),
        option(
            "graphics.max_images",
            "Graphics",
            "int",
            json!(graphics.max_images),
            "同時に保持する inline image resource の最大数。",
            vec![toml_example("graphics", &[("max_images", "128")])],
            None,
            Some("1..".to_owned()),
            ReloadBehavior::Runtime,
            Platform::All,
            security,
        ),
        option(
            "graphics.max_gpu_bytes",
            "Graphics",
            "int",
            json!(graphics.max_gpu_bytes),
            "inline image texture に許可する推定 GPU バイト数の合計。",
            vec![toml_example("graphics", &[("max_gpu_bytes", "268435456")])],
            None,
            Some("1..".to_owned()),
            ReloadBehavior::Runtime,
            Platform::All,
            security,
        ),
    ]
}

fn appearance_config_options(default_config: &AppConfig) -> Vec<ConfigOption> {
    let appearance = default_config.renderer_appearance();
    let mut options = vec![
        option(
            "theme",
            "Appearance",
            "string",
            json!(knightty_render::DEFAULT_THEME_NAME),
            "Built-in theme name. Unknown names fall back to the default theme with a warning.",
            vec![raw_example("theme = \"Catppuccin Mocha\"\n")],
            Some(knightty_render::builtin_theme_names().to_vec()),
            None,
            ReloadBehavior::Runtime,
            Platform::All,
            None,
        ),
        option(
            "theme.light",
            "Appearance",
            "string",
            json!("Catppuccin Latte"),
            "Light theme name used by the theme pair config.",
            vec![toml_example("theme", &[("light", "\"Catppuccin Latte\"")])],
            Some(knightty_render::builtin_theme_names().to_vec()),
            None,
            ReloadBehavior::Runtime,
            Platform::All,
            None,
        ),
        option(
            "theme.dark",
            "Appearance",
            "string",
            json!("Catppuccin Mocha"),
            "Dark theme name used by the theme pair config.",
            vec![toml_example("theme", &[("dark", "\"Catppuccin Mocha\"")])],
            Some(knightty_render::builtin_theme_names().to_vec()),
            None,
            ReloadBehavior::Runtime,
            Platform::All,
            None,
        ),
        option(
            "theme.mode",
            "Appearance",
            "enum",
            json!("system"),
            "Theme pair selection mode. System mode currently falls back to the dark theme.",
            vec![toml_example("theme", &[("mode", "\"system\"")])],
            Some(vec!["system", "light", "dark"]),
            None,
            ReloadBehavior::Runtime,
            Platform::All,
            None,
        ),
        option(
            "window.opacity",
            "Window",
            "float",
            json!(appearance.window.opacity),
            "Window background opacity. Values are clamped to 0.0 through 1.0.",
            vec![toml_example("window", &[("opacity", "0.90")])],
            None,
            Some("0.0..=1.0".to_owned()),
            ReloadBehavior::Restart,
            Platform::All,
            None,
        ),
        option(
            "window.padding_x",
            "Window",
            "int",
            json!(appearance.window.padding_x),
            "Horizontal padding in physical pixels.",
            vec![toml_example("window", &[("padding_x", "12")])],
            None,
            Some("0..".to_owned()),
            ReloadBehavior::Restart,
            Platform::All,
            None,
        ),
        option(
            "window.padding_y",
            "Window",
            "int",
            json!(appearance.window.padding_y),
            "Vertical padding in physical pixels.",
            vec![toml_example("window", &[("padding_y", "10")])],
            None,
            Some("0..".to_owned()),
            ReloadBehavior::Restart,
            Platform::All,
            None,
        ),
        option(
            "window.unfocused_opacity",
            "Window",
            "float",
            json!(appearance.window.unfocused_opacity),
            "Optional opacity multiplier used for unfocused window dimming.",
            vec![toml_example("window", &[("unfocused_opacity", "0.92")])],
            None,
            Some("0.0..=1.0".to_owned()),
            ReloadBehavior::Runtime,
            Platform::All,
            None,
        ),
        option(
            "window.blur",
            "Window",
            "bool",
            json!(appearance.window.blur),
            "Requests platform blur when opacity is below 1.0. Unsupported platforms continue without blur.",
            vec![toml_example("window", &[("blur", "true")])],
            None,
            None,
            ReloadBehavior::Restart,
            Platform::All,
            None,
        ),
        option(
            "window.blur_radius",
            "Window",
            "int",
            json!(appearance.window.blur_radius),
            "Requested blur radius, clamped by the renderer appearance layer.",
            vec![toml_example("window", &[("blur_radius", "20")])],
            None,
            Some(format!("0..={}", knightty_render::MAX_BLUR_RADIUS)),
            ReloadBehavior::Restart,
            Platform::All,
            None,
        ),
        option(
            "window.windows.backdrop",
            "Window",
            "enum",
            json!("none"),
            "Windows backdrop request. Unsupported environments warn and continue without the effect.",
            vec![toml_example("window.windows", &[("backdrop", "\"mica\"")])],
            Some(vec!["none", "acrylic", "mica", "tabbed"]),
            None,
            ReloadBehavior::Restart,
            Platform::Windows,
            None,
        ),
        option(
            "cursor.style",
            "Cursor",
            "enum",
            json!(appearance.cursor.style.as_str()),
            "Cursor shape used when the terminal application has not requested another style.",
            vec![toml_example("cursor", &[("style", "\"bar\"")])],
            Some(vec!["block", "bar", "underline", "hollow_block"]),
            None,
            ReloadBehavior::Runtime,
            Platform::All,
            None,
        ),
        option(
            "cursor.blink",
            "Cursor",
            "bool",
            json!(appearance.cursor.blink),
            "Whether the cursor should blink.",
            vec![toml_example("cursor", &[("blink", "true")])],
            None,
            None,
            ReloadBehavior::Runtime,
            Platform::All,
            None,
        ),
        option(
            "hyperlink.hover_underline",
            "Hyperlink",
            "bool",
            json!(appearance.hyperlink.hover_underline),
            "Alias for hyperlink.underline_on_hover.",
            vec![toml_example("hyperlink", &[("hover_underline", "true")])],
            None,
            None,
            ReloadBehavior::Runtime,
            Platform::All,
            None,
        ),
        option(
            "hyperlink.hover_foreground",
            "Hyperlink",
            "color",
            Value::Null,
            "Optional #RRGGBB foreground override for hovered hyperlinks.",
            vec![toml_example(
                "hyperlink",
                &[("hover_foreground", "\"#89b4fa\"")],
            )],
            None,
            Some("#RRGGBB".to_owned()),
            ReloadBehavior::Runtime,
            Platform::All,
            None,
        ),
        option(
            "hyperlink.hover_background",
            "Hyperlink",
            "color",
            Value::Null,
            "Optional #RRGGBB background override for hovered hyperlinks.",
            vec![toml_example(
                "hyperlink",
                &[("hover_background", "\"#313244\"")],
            )],
            None,
            Some("#RRGGBB".to_owned()),
            ReloadBehavior::Runtime,
            Platform::All,
            None,
        ),
        option(
            "panes.inactive_opacity",
            "Panes",
            "float",
            json!(appearance.panes.inactive_opacity),
            "Opacity multiplier reserved for inactive pane dimming.",
            vec![toml_example("panes", &[("inactive_opacity", "0.75")])],
            None,
            Some("0.0..=1.0".to_owned()),
            ReloadBehavior::Runtime,
            Platform::All,
            None,
        ),
        option(
            "panes.inactive_tint",
            "Panes",
            "color",
            json!("#181825"),
            "Tint color used by inactive or unfocused dimming overlays.",
            vec![toml_example("panes", &[("inactive_tint", "\"#181825\"")])],
            None,
            Some("#RRGGBB".to_owned()),
            ReloadBehavior::Runtime,
            Platform::All,
            None,
        ),
        option(
            "background.kind",
            "Background",
            "enum",
            json!("solid"),
            "Background mode.",
            vec![toml_example("background", &[("kind", "\"gradient\"")])],
            Some(vec!["solid", "gradient", "image"]),
            None,
            ReloadBehavior::Restart,
            Platform::All,
            None,
        ),
        option(
            "background.gradient.orientation",
            "Background",
            "enum",
            json!("vertical"),
            "Gradient direction.",
            vec![toml_example(
                "background.gradient",
                &[("orientation", "\"vertical\"")],
            )],
            Some(vec!["vertical", "horizontal"]),
            None,
            ReloadBehavior::Runtime,
            Platform::All,
            None,
        ),
        option(
            "background.gradient.colors",
            "Background",
            "list<color>",
            json!([]),
            "Gradient color stops. At least two #RRGGBB values are required.",
            vec![toml_example(
                "background.gradient",
                &[("colors", "[\"#1e1e2e\", \"#181825\", \"#11111b\"]")],
            )],
            None,
            Some("at least two #RRGGBB colors".to_owned()),
            ReloadBehavior::Runtime,
            Platform::All,
            None,
        ),
        option(
            "background.image.path",
            "Background",
            "path",
            Value::Null,
            "PNG background image path. Relative paths resolve from the config file directory.",
            vec![toml_example(
                "background.image",
                &[("path", "\"wallpapers/knightty.png\"")],
            )],
            None,
            Some("PNG file".to_owned()),
            ReloadBehavior::Restart,
            Platform::All,
            None,
        ),
        option(
            "background.image.opacity",
            "Background",
            "float",
            json!(0.25),
            "Background image opacity. Values are clamped to 0.0 through 1.0.",
            vec![toml_example("background.image", &[("opacity", "0.25")])],
            None,
            Some("0.0..=1.0".to_owned()),
            ReloadBehavior::Runtime,
            Platform::All,
            None,
        ),
        option(
            "background.image.fit",
            "Background",
            "enum",
            json!("cover"),
            "Background image fit mode.",
            vec![toml_example("background.image", &[("fit", "\"cover\"")])],
            Some(vec!["contain", "cover", "stretch", "tile", "center"]),
            None,
            ReloadBehavior::Runtime,
            Platform::All,
            None,
        ),
        option(
            "background.image.tint",
            "Background",
            "color",
            Value::Null,
            "Optional #RRGGBB readability tint for image backgrounds.",
            vec![toml_example("background.image", &[("tint", "\"#1e1e2e\"")])],
            None,
            Some("#RRGGBB".to_owned()),
            ReloadBehavior::Runtime,
            Platform::All,
            None,
        ),
        option(
            "background.image.tint_opacity",
            "Background",
            "float",
            json!(0.60),
            "Tint opacity for image backgrounds. Values are clamped to 0.0 through 1.0.",
            vec![toml_example(
                "background.image",
                &[("tint_opacity", "0.60")],
            )],
            None,
            Some("0.0..=1.0".to_owned()),
            ReloadBehavior::Runtime,
            Platform::All,
            None,
        ),
        option(
            "effects.retro_crt",
            "Effects",
            "bool",
            json!(appearance.effects.retro_crt),
            "Reserved retro CRT effect flag. Disabled by default.",
            vec![toml_example("effects", &[("retro_crt", "false")])],
            None,
            None,
            ReloadBehavior::Restart,
            Platform::All,
            None,
        ),
        option(
            "effects.scanlines",
            "Effects",
            "bool",
            json!(appearance.effects.scanlines),
            "Reserved scanline effect flag. Disabled by default.",
            vec![toml_example("effects", &[("scanlines", "false")])],
            None,
            None,
            ReloadBehavior::Restart,
            Platform::All,
            None,
        ),
    ];

    options.extend(color_options());
    options.extend(search_options(appearance.search));
    options.extend(tab_options(&appearance.tabs));
    options
}

fn color_options() -> Vec<ConfigOption> {
    let mut options = Vec::new();
    for (key, description) in [
        (
            "colors.background",
            "Override the theme default background color.",
        ),
        (
            "colors.foreground",
            "Override the theme default foreground color.",
        ),
        (
            "colors.selection_background",
            "Override the selection background color.",
        ),
        (
            "colors.selection_foreground",
            "Override the selected text foreground color.",
        ),
        ("colors.cursor", "Override the cursor body color."),
        (
            "colors.cursor_text",
            "Override text color under a block cursor.",
        ),
    ] {
        options.push(option(
            key,
            "Colors",
            "color",
            Value::Null,
            description,
            vec![raw_example(&format!("{key} = \"#1e1e2e\"\n"))],
            None,
            Some("#RRGGBB".to_owned()),
            ReloadBehavior::Runtime,
            Platform::All,
            None,
        ));
    }

    for table in ["colors.normal", "colors.bright"] {
        for name in [
            "black", "red", "green", "yellow", "blue", "magenta", "cyan", "white",
        ] {
            options.push(option(
                Box::leak(format!("{table}.{name}").into_boxed_str()),
                "Colors",
                "color",
                Value::Null,
                "Override one ANSI palette color.",
                vec![toml_example(table, &[(name, "\"#89b4fa\"")])],
                None,
                Some("#RRGGBB".to_owned()),
                ReloadBehavior::Runtime,
                Platform::All,
                None,
            ));
        }
    }

    options
}

fn search_options(search: knightty_render::SearchAppearance) -> Vec<ConfigOption> {
    vec![
        color_option_with_default(
            "search.foreground",
            "Search",
            search.foreground,
            "Foreground for non-current search matches.",
        ),
        color_option_with_default(
            "search.background",
            "Search",
            search.background,
            "Background for non-current search matches.",
        ),
        color_option_with_default(
            "search.selected_foreground",
            "Search",
            search.selected_foreground,
            "Foreground for the current search match.",
        ),
        color_option_with_default(
            "search.selected_background",
            "Search",
            search.selected_background,
            "Background for the current search match.",
        ),
    ]
}

fn tab_options(tabs: &knightty_render::TabAppearance) -> Vec<ConfigOption> {
    vec![
        option(
            "tabs.enabled",
            "Tabs",
            "bool",
            json!(tabs.enabled),
            "Enable tab bar rendering when tab support is present.",
            vec![toml_example("tabs", &[("enabled", "true")])],
            None,
            None,
            ReloadBehavior::Restart,
            Platform::All,
            None,
        ),
        option(
            "tabs.show_when_single",
            "Tabs",
            "bool",
            json!(tabs.show_when_single),
            "Show the tab bar even when only one tab exists.",
            vec![toml_example("tabs", &[("show_when_single", "false")])],
            None,
            None,
            ReloadBehavior::Restart,
            Platform::All,
            None,
        ),
        option(
            "tabs.style",
            "Tabs",
            "enum",
            json!("minimal"),
            "Tab bar visual style. Only minimal is currently rendered.",
            vec![toml_example("tabs", &[("style", "\"minimal\"")])],
            Some(vec!["minimal", "separator", "powerline", "slant"]),
            None,
            ReloadBehavior::Restart,
            Platform::All,
            None,
        ),
        color_option_with_default(
            "tabs.active_background",
            "Tabs",
            tabs.active_background,
            "Active tab background color.",
        ),
        color_option_with_default(
            "tabs.active_foreground",
            "Tabs",
            tabs.active_foreground,
            "Active tab foreground color.",
        ),
        color_option_with_default(
            "tabs.inactive_background",
            "Tabs",
            tabs.inactive_background,
            "Inactive tab background color.",
        ),
        color_option_with_default(
            "tabs.inactive_foreground",
            "Tabs",
            tabs.inactive_foreground,
            "Inactive tab foreground color.",
        ),
    ]
}

fn color_option_with_default(
    key: &'static str,
    category: &'static str,
    color: knightty_render::Rgba,
    description: &'static str,
) -> ConfigOption {
    option(
        key,
        category,
        "color",
        json!(format!("#{:02x}{:02x}{:02x}", color.r, color.g, color.b)),
        description,
        vec![raw_example(&format!(
            "{key} = \"#{:02x}{:02x}{:02x}\"\n",
            color.r, color.g, color.b
        ))],
        None,
        Some("#RRGGBB".to_owned()),
        ReloadBehavior::Runtime,
        Platform::All,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
fn option(
    key: &'static str,
    category: &'static str,
    value_type: &'static str,
    default: Value,
    description: &'static str,
    examples: Vec<String>,
    valid_values: Option<Vec<&'static str>>,
    range: Option<String>,
    reload: ReloadBehavior,
    platform: Platform,
    security: Option<&'static str>,
) -> ConfigOption {
    ConfigOption {
        key,
        category,
        value_type,
        default,
        description,
        examples,
        valid_values,
        range,
        reload,
        platform,
        security,
        since: SINCE_VERSION,
        deprecated: false,
    }
}

pub fn default_config_toml() -> String {
    let default_config = AppConfig::default();
    let metrics = default_config.renderer_config().cell_metrics;
    let schemes = DEFAULT_HYPERLINK_ALLOWED_SCHEMES
        .iter()
        .map(|scheme| format!("\"{scheme}\""))
        .collect::<Vec<_>>()
        .join(", ");

    format!(
        "\
# Knightty configuration.
# This file is generated from Rust defaults. Optional unset values are left as comments.

theme = \"Catppuccin Mocha\"

[font]
# family = \"CaskaydiaCove Nerd Font\"
size = {font_size:.1}
# line_height = {line_height:.1}

[window]
initial_cols = {initial_cols}
initial_rows = {initial_rows}
opacity = 1.0
padding_x = 0
padding_y = 0
unfocused_opacity = 1.0
blur = false
blur_radius = 20

[window.windows]
backdrop = \"none\"

[cursor]
style = \"block\"
blink = true

[colors]
# background = \"#1e1e2e\"
# foreground = \"#cdd6f4\"
# selection_background = \"#45475a\"
# selection_foreground = \"#cdd6f4\"
# cursor = \"#f5e0dc\"
# cursor_text = \"#1e1e2e\"

[background]
kind = \"solid\"

[background.gradient]
orientation = \"vertical\"
colors = []

[background.image]
# path = \"wallpapers/knightty.png\"
opacity = 0.25
fit = \"cover\"
# tint = \"#1e1e2e\"
tint_opacity = 0.60

[search]
foreground = \"#1e1e2e\"
background = \"#f9e2af\"
selected_foreground = \"#1e1e2e\"
selected_background = \"#fab387\"

[panes]
inactive_opacity = 1.0
inactive_tint = \"#181825\"

[tabs]
enabled = false
show_when_single = false
style = \"minimal\"
active_background = \"#313244\"
active_foreground = \"#cdd6f4\"
inactive_background = \"#181825\"
inactive_foreground = \"#7f849c\"

[effects]
retro_crt = false
scanlines = false

[render]
wgpu_backend = \"auto\"

[terminal]
scrollback_lines = {scrollback_lines}
scroll_multiplier = {scroll_multiplier}

[graphics]
enabled = {graphics_enabled}
max_encoded_bytes = {graphics_max_encoded_bytes}
max_decoded_bytes = {graphics_max_decoded_bytes}
max_width = {graphics_max_width}
max_height = {graphics_max_height}
max_pixels = {graphics_max_pixels}
max_images = {graphics_max_images}
max_gpu_bytes = {graphics_max_gpu_bytes}

[shell]
# program = \"pwsh\"
args = []

[hyperlink]
open_on_ctrl_click = {open_on_ctrl_click}
allowed_schemes = [{schemes}]
underline_on_hover = {underline_on_hover}
",
        font_size = metrics.font_size,
        line_height = metrics.line_height,
        initial_cols = DEFAULT_INITIAL_COLS,
        initial_rows = DEFAULT_INITIAL_ROWS,
        scrollback_lines = DEFAULT_SCROLLBACK_LINES,
        scroll_multiplier = DEFAULT_SCROLL_MULTIPLIER,
        graphics_enabled = default_config.graphics.enabled,
        graphics_max_encoded_bytes = default_config.graphics.max_encoded_bytes,
        graphics_max_decoded_bytes = default_config.graphics.max_decoded_bytes,
        graphics_max_width = default_config.graphics.max_width,
        graphics_max_height = default_config.graphics.max_height,
        graphics_max_pixels = default_config.graphics.max_pixels,
        graphics_max_images = default_config.graphics.max_images,
        graphics_max_gpu_bytes = default_config.graphics.max_gpu_bytes,
        open_on_ctrl_click = default_config.hyperlink_open_on_ctrl_click(),
        underline_on_hover = default_config.hyperlink_underline_on_hover(),
    )
}

pub fn config_reference_json() -> String {
    format!(
        "{}\n",
        serde_json::to_string_pretty(&config_options())
            .expect("config reference metadata should serialize")
    )
}

pub fn generated_config_reference_path(workspace_root: impl AsRef<Path>) -> PathBuf {
    workspace_root
        .as_ref()
        .join("docs")
        .join("generated")
        .join("config-reference.json")
}

pub fn generated_default_config_path(workspace_root: impl AsRef<Path>) -> PathBuf {
    workspace_root
        .as_ref()
        .join("docs")
        .join("generated")
        .join("default-config.toml")
}

fn toml_example(table: &str, assignments: &[(&str, &str)]) -> String {
    let mut example = format!("[{table}]\n");
    for (key, value) in assignments {
        example.push_str(key);
        example.push_str(" = ");
        example.push_str(value);
        example.push('\n');
    }
    example
}

fn raw_example(example: &str) -> String {
    example.to_owned()
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::fs;
    use std::path::Path;

    use serde_json::json;

    use super::{
        config_options, config_reference_json, default_config_toml,
        generated_config_reference_path, generated_default_config_path,
    };
    use crate::config::{
        AppConfig, DEFAULT_INITIAL_COLS, DEFAULT_INITIAL_ROWS, DEFAULT_SCROLL_MULTIPLIER,
        DEFAULT_SCROLLBACK_LINES, MAX_SCROLL_MULTIPLIER, MAX_SCROLLBACK_LINES,
        parse_wgpu_backend_name, supported_config_keys,
    };

    #[test]
    fn default_config_toml_parses_to_effective_defaults() {
        let config: AppConfig = toml::from_str(&default_config_toml()).expect("parse default TOML");
        let default_config = AppConfig::default();

        assert_eq!(config.initial_cols(), default_config.initial_cols());
        assert_eq!(config.initial_rows(), default_config.initial_rows());
        assert_eq!(config.scrollback_lines(), default_config.scrollback_lines());
        assert_eq!(
            config.scroll_multiplier(),
            default_config.scroll_multiplier()
        );
        assert_eq!(config.renderer_config(), default_config.renderer_config());
        assert_eq!(
            config.hyperlink_allowed_schemes(),
            default_config.hyperlink_allowed_schemes()
        );
        assert_eq!(
            config.hyperlink_open_on_ctrl_click(),
            default_config.hyperlink_open_on_ctrl_click()
        );
        assert_eq!(
            config.hyperlink_underline_on_hover(),
            default_config.hyperlink_underline_on_hover()
        );
    }

    #[test]
    fn config_options_include_every_supported_key_once() {
        let metadata_keys = config_options()
            .into_iter()
            .map(|option| option.key)
            .collect::<BTreeSet<_>>();
        let supported_keys = supported_config_keys()
            .iter()
            .copied()
            .collect::<BTreeSet<_>>();

        assert_eq!(metadata_keys, supported_keys);
    }

    #[test]
    fn config_options_are_sorted_by_key() {
        let keys = config_options()
            .into_iter()
            .map(|option| option.key)
            .collect::<Vec<_>>();
        let mut sorted_keys = keys.clone();
        sorted_keys.sort_unstable();

        assert_eq!(keys, sorted_keys);
    }

    #[test]
    fn documented_defaults_match_effective_defaults() {
        let default_config = AppConfig::default();
        let options = config_options();

        assert_eq!(
            option_default(&options, "font.size"),
            json!(default_config.renderer_config().cell_metrics.font_size)
        );
        assert_eq!(
            option_default(&options, "font.line_height"),
            serde_json::Value::Null
        );
        assert_eq!(
            option_default(&options, "window.initial_cols"),
            json!(DEFAULT_INITIAL_COLS)
        );
        assert_eq!(
            option_default(&options, "window.initial_rows"),
            json!(DEFAULT_INITIAL_ROWS)
        );
        assert_eq!(
            option_default(&options, "render.wgpu_backend"),
            json!("auto")
        );
        assert_eq!(
            option_default(&options, "terminal.scrollback_lines"),
            json!(DEFAULT_SCROLLBACK_LINES)
        );
        assert_eq!(
            option_default(&options, "terminal.scroll_multiplier"),
            json!(DEFAULT_SCROLL_MULTIPLIER)
        );
        assert_eq!(option_default(&options, "shell.program"), json!(null));
        assert_eq!(option_default(&options, "shell.args"), json!([]));
        assert_eq!(
            option_default(&options, "hyperlink.open_on_ctrl_click"),
            json!(default_config.hyperlink_open_on_ctrl_click())
        );
        assert_eq!(
            option_default(&options, "hyperlink.allowed_schemes"),
            json!(default_config.hyperlink_allowed_schemes())
        );
        assert_eq!(
            option_default(&options, "hyperlink.underline_on_hover"),
            json!(default_config.hyperlink_underline_on_hover())
        );
    }

    #[test]
    fn documented_ranges_match_validation_constants() {
        let options = config_options();

        assert_eq!(
            option_range(&options, "terminal.scrollback_lines"),
            Some(format!("0..={MAX_SCROLLBACK_LINES}"))
        );
        assert_eq!(
            option_range(&options, "terminal.scroll_multiplier"),
            Some(format!("1..={MAX_SCROLL_MULTIPLIER}"))
        );
        assert_eq!(
            option_range(&options, "window.initial_cols"),
            Some("1..".to_owned())
        );
        assert_eq!(
            option_range(&options, "window.initial_rows"),
            Some("1..".to_owned())
        );
    }

    #[test]
    fn documented_wgpu_values_match_parser() {
        let options = config_options();
        let values = options
            .iter()
            .find(|option| option.key == "render.wgpu_backend")
            .and_then(|option| option.valid_values.as_ref())
            .expect("wgpu valid values");

        for value in values {
            assert_eq!(parse_wgpu_backend_name(value), Some(*value));
        }
    }

    #[test]
    fn checked_in_generated_config_reference_is_current() {
        let workspace_root = workspace_root();
        let path = generated_config_reference_path(workspace_root);
        let checked_in = fs::read_to_string(&path).expect("read checked-in config reference JSON");

        assert_eq!(checked_in, config_reference_json());
    }

    #[test]
    fn checked_in_generated_default_config_is_current() {
        let workspace_root = workspace_root();
        let path = generated_default_config_path(workspace_root);
        let checked_in = fs::read_to_string(&path).expect("read checked-in default config TOML");

        assert_eq!(checked_in, default_config_toml());
    }

    fn workspace_root() -> &'static Path {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .expect("workspace root")
    }

    fn option_default(options: &[super::ConfigOption], key: &str) -> serde_json::Value {
        options
            .iter()
            .find(|option| option.key == key)
            .unwrap_or_else(|| panic!("missing option {key}"))
            .default
            .clone()
    }

    fn option_range(options: &[super::ConfigOption], key: &str) -> Option<String> {
        options
            .iter()
            .find(|option| option.key == key)
            .unwrap_or_else(|| panic!("missing option {key}"))
            .range
            .clone()
    }
}
