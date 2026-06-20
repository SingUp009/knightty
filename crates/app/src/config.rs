use std::env;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use knightty_pty::ShellCommand;
use knightty_render::{
    BackgroundAppearance, BackgroundKind, CellMetrics, CursorAppearance, CursorStyle,
    EffectAppearance, GradientBackground, GradientOrientation, HyperlinkAppearance,
    ImageBackground, ImageFit, ImageLoadState, PaneAppearance, RendererAppearance, RendererConfig,
    ResolvedTheme, Rgba, SearchAppearance, TabAppearance, TabStyle, WindowAppearance,
    WindowBackdrop, clamp_blur_radius, clamp_opacity, resolve_theme_name,
};
use serde::{Deserialize, Deserializer};
use thiserror::Error;

pub(crate) const CONFIG_FILE_NAME: &str = "knightty.config";
pub(crate) const DEFAULT_INITIAL_COLS: usize = 80;
pub(crate) const DEFAULT_INITIAL_ROWS: usize = 24;
pub(crate) const DEFAULT_SCROLLBACK_LINES: usize = 10_000;
pub(crate) const DEFAULT_SCROLL_MULTIPLIER: usize = 3;
pub(crate) const MAX_SCROLLBACK_LINES: usize = 100_000;
pub(crate) const MAX_SCROLL_MULTIPLIER: usize = 100;
pub(crate) const DEFAULT_HYPERLINK_ALLOWED_SCHEMES: &[&str] = &["https", "http"];
pub(crate) const DEFAULT_GRAPHICS_MAX_ENCODED_BYTES: usize = 16 * 1024 * 1024;
pub(crate) const DEFAULT_GRAPHICS_MAX_DECODED_BYTES: usize = 128 * 1024 * 1024;
pub(crate) const DEFAULT_GRAPHICS_MAX_WIDTH: u32 = 8192;
pub(crate) const DEFAULT_GRAPHICS_MAX_HEIGHT: u32 = 8192;
pub(crate) const DEFAULT_GRAPHICS_MAX_PIXELS: u64 = 32_000_000;
pub(crate) const DEFAULT_GRAPHICS_MAX_IMAGES: usize = 128;
pub(crate) const DEFAULT_GRAPHICS_MAX_GPU_BYTES: usize = 256 * 1024 * 1024;
pub(crate) const SUPPORTED_CONFIG_KEYS: &[&str] = &[
    "background.gradient.colors",
    "background.gradient.orientation",
    "background.image.fit",
    "background.image.opacity",
    "background.image.path",
    "background.image.tint",
    "background.image.tint_opacity",
    "background.kind",
    "colors.background",
    "colors.bright.black",
    "colors.bright.blue",
    "colors.bright.cyan",
    "colors.bright.green",
    "colors.bright.magenta",
    "colors.bright.red",
    "colors.bright.white",
    "colors.bright.yellow",
    "colors.cursor",
    "colors.cursor_text",
    "colors.foreground",
    "colors.normal.black",
    "colors.normal.blue",
    "colors.normal.cyan",
    "colors.normal.green",
    "colors.normal.magenta",
    "colors.normal.red",
    "colors.normal.white",
    "colors.normal.yellow",
    "colors.selection_background",
    "colors.selection_foreground",
    "cursor.blink",
    "cursor.style",
    "effects.retro_crt",
    "effects.scanlines",
    "font.family",
    "font.line_height",
    "font.size",
    "graphics.enabled",
    "graphics.max_decoded_bytes",
    "graphics.max_encoded_bytes",
    "graphics.max_gpu_bytes",
    "graphics.max_height",
    "graphics.max_images",
    "graphics.max_pixels",
    "graphics.max_width",
    "hyperlink.allowed_schemes",
    "hyperlink.hover_background",
    "hyperlink.hover_foreground",
    "hyperlink.hover_underline",
    "hyperlink.open_on_ctrl_click",
    "hyperlink.underline_on_hover",
    "panes.inactive_opacity",
    "panes.inactive_tint",
    "render.wgpu_backend",
    "search.background",
    "search.foreground",
    "search.selected_background",
    "search.selected_foreground",
    "shell.args",
    "shell.program",
    "tabs.active_background",
    "tabs.active_foreground",
    "tabs.enabled",
    "tabs.inactive_background",
    "tabs.inactive_foreground",
    "tabs.show_when_single",
    "tabs.style",
    "terminal.scroll_multiplier",
    "terminal.scrollback_lines",
    "theme",
    "theme.dark",
    "theme.light",
    "theme.mode",
    "window.blur",
    "window.blur_radius",
    "window.initial_cols",
    "window.initial_rows",
    "window.opacity",
    "window.padding_x",
    "window.padding_y",
    "window.unfocused_opacity",
    "window.windows.backdrop",
];

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct AppConfig {
    pub theme: ThemeConfig,
    pub colors: ColorConfig,
    pub font: FontConfig,
    pub window: WindowConfig,
    pub cursor: CursorConfig,
    pub render: RenderConfig,
    pub terminal: TerminalConfig,
    pub graphics: GraphicsConfig,
    pub shell: ShellConfig,
    pub hyperlink: HyperlinkConfig,
    pub search: SearchConfig,
    pub panes: PanesConfig,
    pub background: BackgroundConfig,
    pub tabs: TabsConfig,
    pub effects: EffectsConfig,
    #[serde(skip)]
    pub config_dir: Option<PathBuf>,
    #[serde(skip)]
    pub source_path: Option<PathBuf>,
}

impl AppConfig {
    pub fn initial_cols(&self) -> usize {
        self.window.initial_cols.unwrap_or(DEFAULT_INITIAL_COLS)
    }

    pub fn initial_rows(&self) -> usize {
        self.window.initial_rows.unwrap_or(DEFAULT_INITIAL_ROWS)
    }

    pub fn wgpu_backend(&self) -> Option<&str> {
        self.render.wgpu_backend.as_deref()
    }

    pub fn scrollback_lines(&self) -> usize {
        self.terminal
            .scrollback_lines
            .unwrap_or(DEFAULT_SCROLLBACK_LINES)
    }

    pub fn scroll_multiplier(&self) -> usize {
        self.terminal
            .scroll_multiplier
            .unwrap_or(DEFAULT_SCROLL_MULTIPLIER)
    }

    pub fn renderer_config(&self) -> RendererConfig {
        let default_metrics = CellMetrics::default();
        let font_size = self.font.size.unwrap_or(default_metrics.font_size);
        let line_height = self
            .font
            .line_height
            .unwrap_or_else(|| font_size * default_metrics.line_height / default_metrics.font_size);

        RendererConfig {
            cell_metrics: CellMetrics::from_font_size(font_size, line_height),
            font_family: self.font.family.clone(),
            appearance: self.renderer_appearance(),
        }
    }

    pub fn renderer_appearance(&self) -> RendererAppearance {
        let theme_name = self.theme.effective_theme_name(SystemAppearance::Dark);
        let (mut theme, warning) = resolve_theme_name(theme_name.as_deref());
        if let Some(warning) = warning {
            eprintln!("knightty config: warning: {warning}");
        }
        self.colors.apply_overrides(&mut theme);

        RendererAppearance {
            theme,
            window: self.window.appearance(),
            cursor: self.cursor.appearance(),
            hyperlink: self.hyperlink.appearance(),
            search: self.search.appearance(),
            panes: self.panes.appearance(),
            background: self
                .background
                .appearance(self.config_dir.as_deref(), theme.background),
            tabs: self.tabs.appearance(),
            effects: self.effects.appearance(),
        }
    }

    pub fn shell_command(&self) -> Option<ShellCommand> {
        self.shell.program.as_ref().map(|program| ShellCommand {
            program: program.clone(),
            args: self.shell.args.clone(),
        })
    }

    pub fn hyperlink_open_on_ctrl_click(&self) -> bool {
        self.hyperlink.open_on_ctrl_click
    }

    pub fn hyperlink_allowed_schemes(&self) -> &[String] {
        &self.hyperlink.allowed_schemes
    }

    pub fn hyperlink_underline_on_hover(&self) -> bool {
        self.hyperlink.underline_on_hover
    }

    pub fn hyperlink_open_enabled(&self) -> bool {
        self.hyperlink_open_on_ctrl_click() && !self.hyperlink.allowed_schemes.is_empty()
    }

    fn validate(&self, source: Option<&Path>) -> Result<(), ConfigError> {
        validate_positive_float("font.size", self.font.size, source)?;
        validate_positive_float("font.line_height", self.font.line_height, source)?;
        validate_positive_usize("window.initial_cols", self.window.initial_cols, source)?;
        validate_positive_usize("window.initial_rows", self.window.initial_rows, source)?;
        validate_non_empty_string("shell.program", self.shell.program.as_deref(), source)?;
        validate_usize_range(
            "terminal.scrollback_lines",
            self.terminal.scrollback_lines,
            0,
            MAX_SCROLLBACK_LINES,
            source,
        )?;
        validate_usize_range(
            "terminal.scroll_multiplier",
            self.terminal.scroll_multiplier,
            1,
            MAX_SCROLL_MULTIPLIER,
            source,
        )?;
        for (field, value) in [
            (
                "graphics.max_encoded_bytes",
                self.graphics.max_encoded_bytes as u128,
            ),
            (
                "graphics.max_decoded_bytes",
                self.graphics.max_decoded_bytes as u128,
            ),
            (
                "graphics.max_gpu_bytes",
                self.graphics.max_gpu_bytes as u128,
            ),
            ("graphics.max_width", self.graphics.max_width as u128),
            ("graphics.max_height", self.graphics.max_height as u128),
            ("graphics.max_pixels", self.graphics.max_pixels as u128),
            ("graphics.max_images", self.graphics.max_images as u128),
        ] {
            if value == 0 {
                return Err(ConfigError::InvalidValue {
                    path: source.map(Path::to_path_buf),
                    field,
                    message: "expected a positive integer, got `0`".to_owned(),
                });
            }
        }
        if let Some(value) = &self.render.wgpu_backend
            && parse_wgpu_backend_name(value).is_none()
        {
            return Err(ConfigError::InvalidValue {
                path: source.map(Path::to_path_buf),
                field: "render.wgpu_backend",
                message: format!("expected one of auto, vulkan, dx12, or gl, got `{value}`"),
            });
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SystemAppearance {
    Light,
    Dark,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum ThemeConfig {
    Name(String),
    Pair(ThemePairConfig),
}

impl Default for ThemeConfig {
    fn default() -> Self {
        Self::Name(knightty_render::DEFAULT_THEME_NAME.to_owned())
    }
}

impl ThemeConfig {
    pub fn effective_theme_name(&self, system: SystemAppearance) -> Option<String> {
        match self {
            Self::Name(name) => Some(name.clone()),
            Self::Pair(pair) => Some(match pair.mode {
                ThemeMode::Light => pair.light.clone(),
                ThemeMode::Dark => pair.dark.clone(),
                ThemeMode::System => match system {
                    SystemAppearance::Light => pair.light.clone(),
                    SystemAppearance::Dark => pair.dark.clone(),
                },
            }),
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default)]
pub struct ThemePairConfig {
    pub light: String,
    pub dark: String,
    pub mode: ThemeMode,
}

impl Default for ThemePairConfig {
    fn default() -> Self {
        Self {
            light: "Catppuccin Latte".to_owned(),
            dark: "Catppuccin Mocha".to_owned(),
            mode: ThemeMode::System,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ThemeMode {
    #[default]
    System,
    Light,
    Dark,
}

impl<'de> Deserialize<'de> for ThemeMode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        match value.as_str() {
            "system" => Ok(Self::System),
            "light" => Ok(Self::Light),
            "dark" => Ok(Self::Dark),
            _ => Err(serde::de::Error::custom(
                "expected system, light, or dark for theme.mode",
            )),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct ColorConfig {
    #[serde(deserialize_with = "deserialize_optional_rgb")]
    pub background: Option<Rgba>,
    #[serde(deserialize_with = "deserialize_optional_rgb")]
    pub foreground: Option<Rgba>,
    #[serde(deserialize_with = "deserialize_optional_rgb")]
    pub selection_background: Option<Rgba>,
    #[serde(deserialize_with = "deserialize_optional_rgb")]
    pub selection_foreground: Option<Rgba>,
    #[serde(deserialize_with = "deserialize_optional_rgb")]
    pub cursor: Option<Rgba>,
    #[serde(deserialize_with = "deserialize_optional_rgb")]
    pub cursor_text: Option<Rgba>,
    pub normal: ColorTableConfig,
    pub bright: ColorTableConfig,
}

impl ColorConfig {
    fn apply_overrides(&self, theme: &mut ResolvedTheme) {
        if let Some(color) = self.background {
            theme.background = color;
        }
        if let Some(color) = self.foreground {
            theme.foreground = color;
        }
        if let Some(color) = self.selection_background {
            theme.selection_background = color;
        }
        if let Some(color) = self.selection_foreground {
            theme.selection_foreground = color;
        }
        if let Some(color) = self.cursor {
            theme.cursor = color;
        }
        if let Some(color) = self.cursor_text {
            theme.cursor_text = color;
        }
        self.normal.apply_overrides(&mut theme.normal);
        self.bright.apply_overrides(&mut theme.bright);
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct ColorTableConfig {
    #[serde(deserialize_with = "deserialize_optional_rgb")]
    pub black: Option<Rgba>,
    #[serde(deserialize_with = "deserialize_optional_rgb")]
    pub red: Option<Rgba>,
    #[serde(deserialize_with = "deserialize_optional_rgb")]
    pub green: Option<Rgba>,
    #[serde(deserialize_with = "deserialize_optional_rgb")]
    pub yellow: Option<Rgba>,
    #[serde(deserialize_with = "deserialize_optional_rgb")]
    pub blue: Option<Rgba>,
    #[serde(deserialize_with = "deserialize_optional_rgb")]
    pub magenta: Option<Rgba>,
    #[serde(deserialize_with = "deserialize_optional_rgb")]
    pub cyan: Option<Rgba>,
    #[serde(deserialize_with = "deserialize_optional_rgb")]
    pub white: Option<Rgba>,
}

impl ColorTableConfig {
    fn apply_overrides(&self, colors: &mut [Rgba; 8]) {
        let overrides = [
            self.black,
            self.red,
            self.green,
            self.yellow,
            self.blue,
            self.magenta,
            self.cyan,
            self.white,
        ];
        for (index, color) in overrides.into_iter().enumerate() {
            if let Some(color) = color {
                colors[index] = color;
            }
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct FontConfig {
    pub family: Option<String>,
    pub size: Option<f32>,
    pub line_height: Option<f32>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct WindowConfig {
    pub initial_cols: Option<usize>,
    pub initial_rows: Option<usize>,
    pub opacity: Option<f32>,
    pub padding_x: Option<u32>,
    pub padding_y: Option<u32>,
    pub unfocused_opacity: Option<f32>,
    pub blur: bool,
    pub blur_radius: Option<u32>,
    pub windows: WindowsConfig,
}

impl WindowConfig {
    fn appearance(&self) -> WindowAppearance {
        WindowAppearance {
            opacity: clamp_opacity(self.opacity.unwrap_or(1.0)),
            padding_x: self.padding_x.unwrap_or(0),
            padding_y: self.padding_y.unwrap_or(0),
            unfocused_opacity: clamp_opacity(self.unfocused_opacity.unwrap_or(1.0)),
            blur: self.blur,
            blur_radius: clamp_blur_radius(self.blur_radius.unwrap_or(20)),
            backdrop: self.windows.backdrop,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(default)]
pub struct WindowsConfig {
    #[serde(deserialize_with = "deserialize_window_backdrop")]
    pub backdrop: WindowBackdrop,
}

impl Default for WindowsConfig {
    fn default() -> Self {
        Self {
            backdrop: WindowBackdrop::None,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(default)]
pub struct CursorConfig {
    #[serde(
        default = "default_cursor_style",
        deserialize_with = "deserialize_cursor_style"
    )]
    pub style: CursorStyle,
    pub blink: bool,
}

impl Default for CursorConfig {
    fn default() -> Self {
        Self {
            style: CursorStyle::Block,
            blink: true,
        }
    }
}

impl CursorConfig {
    fn appearance(&self) -> CursorAppearance {
        CursorAppearance {
            style: self.style,
            blink: self.blink,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct RenderConfig {
    pub wgpu_backend: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct TerminalConfig {
    pub scrollback_lines: Option<usize>,
    pub scroll_multiplier: Option<usize>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default)]
pub struct GraphicsConfig {
    pub enabled: bool,
    pub max_encoded_bytes: usize,
    pub max_decoded_bytes: usize,
    pub max_width: u32,
    pub max_height: u32,
    pub max_pixels: u64,
    pub max_images: usize,
    pub max_gpu_bytes: usize,
}

impl Default for GraphicsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_encoded_bytes: DEFAULT_GRAPHICS_MAX_ENCODED_BYTES,
            max_decoded_bytes: DEFAULT_GRAPHICS_MAX_DECODED_BYTES,
            max_width: DEFAULT_GRAPHICS_MAX_WIDTH,
            max_height: DEFAULT_GRAPHICS_MAX_HEIGHT,
            max_pixels: DEFAULT_GRAPHICS_MAX_PIXELS,
            max_images: DEFAULT_GRAPHICS_MAX_IMAGES,
            max_gpu_bytes: DEFAULT_GRAPHICS_MAX_GPU_BYTES,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct ShellConfig {
    pub program: Option<String>,
    pub args: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default)]
pub struct HyperlinkConfig {
    pub open_on_ctrl_click: bool,
    #[serde(
        default = "default_hyperlink_allowed_schemes",
        deserialize_with = "deserialize_lowercase_schemes"
    )]
    pub allowed_schemes: Vec<String>,
    #[serde(alias = "hover_underline")]
    pub underline_on_hover: bool,
    #[serde(deserialize_with = "deserialize_optional_rgb")]
    pub hover_foreground: Option<Rgba>,
    #[serde(deserialize_with = "deserialize_optional_rgb")]
    pub hover_background: Option<Rgba>,
}

impl Default for HyperlinkConfig {
    fn default() -> Self {
        Self {
            open_on_ctrl_click: true,
            allowed_schemes: default_hyperlink_allowed_schemes(),
            underline_on_hover: true,
            hover_foreground: None,
            hover_background: None,
        }
    }
}

impl HyperlinkConfig {
    fn appearance(&self) -> HyperlinkAppearance {
        HyperlinkAppearance {
            hover_underline: self.underline_on_hover,
            hover_foreground: self.hover_foreground,
            hover_background: self.hover_background,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(default)]
pub struct SearchConfig {
    #[serde(
        default = "default_search_foreground",
        deserialize_with = "deserialize_rgb"
    )]
    pub foreground: Rgba,
    #[serde(
        default = "default_search_background",
        deserialize_with = "deserialize_rgb"
    )]
    pub background: Rgba,
    #[serde(
        default = "default_search_selected_foreground",
        deserialize_with = "deserialize_rgb"
    )]
    pub selected_foreground: Rgba,
    #[serde(
        default = "default_search_selected_background",
        deserialize_with = "deserialize_rgb"
    )]
    pub selected_background: Rgba,
}

impl Default for SearchConfig {
    fn default() -> Self {
        let search = SearchAppearance::default();
        Self {
            foreground: search.foreground,
            background: search.background,
            selected_foreground: search.selected_foreground,
            selected_background: search.selected_background,
        }
    }
}

impl SearchConfig {
    fn appearance(&self) -> SearchAppearance {
        SearchAppearance {
            foreground: self.foreground,
            background: self.background,
            selected_foreground: self.selected_foreground,
            selected_background: self.selected_background,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(default)]
pub struct PanesConfig {
    pub inactive_opacity: f32,
    #[serde(default = "default_pane_tint", deserialize_with = "deserialize_rgb")]
    pub inactive_tint: Rgba,
}

impl Default for PanesConfig {
    fn default() -> Self {
        let panes = PaneAppearance::default();
        Self {
            inactive_opacity: panes.inactive_opacity,
            inactive_tint: panes.inactive_tint,
        }
    }
}

impl PanesConfig {
    fn appearance(&self) -> PaneAppearance {
        PaneAppearance {
            inactive_opacity: clamp_opacity(self.inactive_opacity),
            inactive_tint: self.inactive_tint,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default)]
pub struct BackgroundConfig {
    #[serde(
        default = "default_background_kind",
        deserialize_with = "deserialize_background_kind"
    )]
    pub kind: BackgroundKindConfig,
    pub gradient: GradientConfig,
    pub image: ImageConfig,
}

impl Default for BackgroundConfig {
    fn default() -> Self {
        Self {
            kind: BackgroundKindConfig::Solid,
            gradient: GradientConfig::default(),
            image: ImageConfig::default(),
        }
    }
}

impl BackgroundConfig {
    fn appearance(&self, config_dir: Option<&Path>, fallback_color: Rgba) -> BackgroundAppearance {
        let kind = match self.kind {
            BackgroundKindConfig::Solid => BackgroundKind::Solid,
            BackgroundKindConfig::Gradient => {
                let colors = if self.gradient.colors.len() >= 2 {
                    self.gradient.colors.clone()
                } else {
                    eprintln!(
                        "knightty config: warning: background.gradient.colors requires at least two colors; falling back to solid background"
                    );
                    return BackgroundAppearance::default();
                };
                BackgroundKind::Gradient(GradientBackground {
                    orientation: self.gradient.orientation,
                    colors,
                })
            }
            BackgroundKindConfig::Image => {
                let Some(path) = self.image.path.as_ref() else {
                    eprintln!(
                        "knightty config: warning: background.kind = image requires background.image.path; falling back to solid background"
                    );
                    return BackgroundAppearance::default();
                };
                let path = resolve_config_path(config_dir, path);
                let load_state = image_load_state(&path);
                if load_state != ImageLoadState::Ready {
                    eprintln!(
                        "knightty config: warning: background image `{}` is not usable ({load_state:?}); falling back to solid background",
                        path.display()
                    );
                    return BackgroundAppearance::default();
                }
                BackgroundKind::Image(ImageBackground {
                    path,
                    opacity: clamp_opacity(self.image.opacity),
                    fit: self.image.fit,
                    tint: self.image.tint.unwrap_or(fallback_color),
                    tint_opacity: clamp_opacity(self.image.tint_opacity),
                    load_state,
                })
            }
        };

        BackgroundAppearance { kind }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BackgroundKindConfig {
    Solid,
    Gradient,
    Image,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default)]
pub struct GradientConfig {
    #[serde(
        default = "default_gradient_orientation",
        deserialize_with = "deserialize_gradient_orientation"
    )]
    pub orientation: GradientOrientation,
    #[serde(deserialize_with = "deserialize_rgb_vec")]
    pub colors: Vec<Rgba>,
}

impl Default for GradientConfig {
    fn default() -> Self {
        Self {
            orientation: GradientOrientation::Vertical,
            colors: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default)]
pub struct ImageConfig {
    pub path: Option<PathBuf>,
    pub opacity: f32,
    #[serde(
        default = "default_image_fit",
        deserialize_with = "deserialize_image_fit"
    )]
    pub fit: ImageFit,
    #[serde(deserialize_with = "deserialize_optional_rgb")]
    pub tint: Option<Rgba>,
    pub tint_opacity: f32,
}

impl Default for ImageConfig {
    fn default() -> Self {
        Self {
            path: None,
            opacity: 0.25,
            fit: ImageFit::Cover,
            tint: None,
            tint_opacity: 0.60,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default)]
pub struct TabsConfig {
    pub enabled: bool,
    pub show_when_single: bool,
    #[serde(
        default = "default_tab_style",
        deserialize_with = "deserialize_tab_style"
    )]
    pub style: TabStyle,
    #[serde(
        default = "default_tab_active_background",
        deserialize_with = "deserialize_rgb"
    )]
    pub active_background: Rgba,
    #[serde(
        default = "default_tab_active_foreground",
        deserialize_with = "deserialize_rgb"
    )]
    pub active_foreground: Rgba,
    #[serde(
        default = "default_tab_inactive_background",
        deserialize_with = "deserialize_rgb"
    )]
    pub inactive_background: Rgba,
    #[serde(
        default = "default_tab_inactive_foreground",
        deserialize_with = "deserialize_rgb"
    )]
    pub inactive_foreground: Rgba,
}

impl Default for TabsConfig {
    fn default() -> Self {
        let tabs = TabAppearance::default();
        Self {
            enabled: tabs.enabled,
            show_when_single: tabs.show_when_single,
            style: tabs.style,
            active_background: tabs.active_background,
            active_foreground: tabs.active_foreground,
            inactive_background: tabs.inactive_background,
            inactive_foreground: tabs.inactive_foreground,
        }
    }
}

impl TabsConfig {
    fn appearance(&self) -> TabAppearance {
        TabAppearance {
            enabled: self.enabled,
            show_when_single: self.show_when_single,
            style: self.style,
            active_background: self.active_background,
            active_foreground: self.active_foreground,
            inactive_background: self.inactive_background,
            inactive_foreground: self.inactive_foreground,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct EffectsConfig {
    pub retro_crt: bool,
    pub scanlines: bool,
}

impl EffectsConfig {
    fn appearance(&self) -> EffectAppearance {
        EffectAppearance {
            retro_crt: self.retro_crt,
            scanlines: self.scanlines,
        }
    }
}

fn default_hyperlink_allowed_schemes() -> Vec<String> {
    DEFAULT_HYPERLINK_ALLOWED_SCHEMES
        .iter()
        .map(|scheme| (*scheme).to_owned())
        .collect()
}

fn deserialize_lowercase_schemes<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    Vec::<String>::deserialize(deserializer).map(|schemes| {
        schemes
            .into_iter()
            .map(|scheme| scheme.to_ascii_lowercase())
            .collect()
    })
}

fn deserialize_optional_rgb<'de, D>(deserializer: D) -> Result<Option<Rgba>, D::Error>
where
    D: Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer)?
        .map(|value| Rgba::from_hex_rgb(&value).map_err(serde::de::Error::custom))
        .transpose()
}

fn deserialize_rgb<'de, D>(deserializer: D) -> Result<Rgba, D::Error>
where
    D: Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    Rgba::from_hex_rgb(&value).map_err(serde::de::Error::custom)
}

fn deserialize_rgb_vec<'de, D>(deserializer: D) -> Result<Vec<Rgba>, D::Error>
where
    D: Deserializer<'de>,
{
    Vec::<String>::deserialize(deserializer)?
        .into_iter()
        .map(|value| Rgba::from_hex_rgb(&value).map_err(serde::de::Error::custom))
        .collect()
}

fn deserialize_cursor_style<'de, D>(deserializer: D) -> Result<CursorStyle, D::Error>
where
    D: Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    CursorStyle::parse(&value)
        .ok_or_else(|| serde::de::Error::custom("expected block, bar, underline, or hollow_block"))
}

fn deserialize_window_backdrop<'de, D>(deserializer: D) -> Result<WindowBackdrop, D::Error>
where
    D: Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    match value.as_str() {
        "none" => Ok(WindowBackdrop::None),
        "acrylic" => Ok(WindowBackdrop::Acrylic),
        "mica" => Ok(WindowBackdrop::Mica),
        "tabbed" => Ok(WindowBackdrop::Tabbed),
        _ => Err(serde::de::Error::custom(
            "expected none, acrylic, mica, or tabbed",
        )),
    }
}

fn deserialize_background_kind<'de, D>(deserializer: D) -> Result<BackgroundKindConfig, D::Error>
where
    D: Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    match value.as_str() {
        "solid" => Ok(BackgroundKindConfig::Solid),
        "gradient" => Ok(BackgroundKindConfig::Gradient),
        "image" => Ok(BackgroundKindConfig::Image),
        _ => Err(serde::de::Error::custom(
            "expected solid, gradient, or image",
        )),
    }
}

fn deserialize_gradient_orientation<'de, D>(
    deserializer: D,
) -> Result<GradientOrientation, D::Error>
where
    D: Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    match value.as_str() {
        "vertical" => Ok(GradientOrientation::Vertical),
        "horizontal" => Ok(GradientOrientation::Horizontal),
        _ => Err(serde::de::Error::custom("expected vertical or horizontal")),
    }
}

fn deserialize_image_fit<'de, D>(deserializer: D) -> Result<ImageFit, D::Error>
where
    D: Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    match value.as_str() {
        "contain" => Ok(ImageFit::Contain),
        "cover" => Ok(ImageFit::Cover),
        "stretch" => Ok(ImageFit::Stretch),
        "tile" => Ok(ImageFit::Tile),
        "center" => Ok(ImageFit::Center),
        _ => Err(serde::de::Error::custom(
            "expected contain, cover, stretch, tile, or center",
        )),
    }
}

fn deserialize_tab_style<'de, D>(deserializer: D) -> Result<TabStyle, D::Error>
where
    D: Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    match value.as_str() {
        "minimal" => Ok(TabStyle::Minimal),
        "separator" => Ok(TabStyle::Separator),
        "powerline" => Ok(TabStyle::Powerline),
        "slant" => Ok(TabStyle::Slant),
        _ => Err(serde::de::Error::custom(
            "expected minimal, separator, powerline, or slant",
        )),
    }
}

fn default_cursor_style() -> CursorStyle {
    CursorStyle::Block
}

fn default_search_foreground() -> Rgba {
    SearchAppearance::default().foreground
}

fn default_search_background() -> Rgba {
    SearchAppearance::default().background
}

fn default_search_selected_foreground() -> Rgba {
    SearchAppearance::default().selected_foreground
}

fn default_search_selected_background() -> Rgba {
    SearchAppearance::default().selected_background
}

fn default_pane_tint() -> Rgba {
    PaneAppearance::default().inactive_tint
}

fn default_background_kind() -> BackgroundKindConfig {
    BackgroundKindConfig::Solid
}

fn default_gradient_orientation() -> GradientOrientation {
    GradientOrientation::Vertical
}

fn default_image_fit() -> ImageFit {
    ImageFit::Cover
}

fn default_tab_style() -> TabStyle {
    TabStyle::Minimal
}

fn default_tab_active_background() -> Rgba {
    TabAppearance::default().active_background
}

fn default_tab_active_foreground() -> Rgba {
    TabAppearance::default().active_foreground
}

fn default_tab_inactive_background() -> Rgba {
    TabAppearance::default().inactive_background
}

fn default_tab_inactive_foreground() -> Rgba {
    TabAppearance::default().inactive_foreground
}

fn resolve_config_path(config_dir: Option<&Path>, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else if let Some(config_dir) = config_dir {
        config_dir.join(path)
    } else {
        path.to_path_buf()
    }
}

fn image_load_state(path: &Path) -> ImageLoadState {
    if path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_none_or(|extension| !extension.eq_ignore_ascii_case("png"))
    {
        return ImageLoadState::UnsupportedFormat;
    }

    if path.is_file() {
        ImageLoadState::Ready
    } else {
        ImageLoadState::Missing
    }
}

pub fn load_app_config() -> Result<AppConfig, ConfigError> {
    let paths = config_paths_from_env(|key| env::var_os(key));
    load_app_config_from_paths(&paths)
}

fn load_app_config_from_paths(paths: &[PathBuf]) -> Result<AppConfig, ConfigError> {
    for path in paths {
        match fs::read_to_string(path) {
            Ok(contents) => {
                let mut config = toml::from_str::<AppConfig>(&contents).map_err(|source| {
                    ConfigError::Parse {
                        path: path.clone(),
                        source,
                    }
                })?;
                config.config_dir = path.parent().map(Path::to_path_buf);
                config.source_path = Some(path.clone());
                config.validate(Some(path))?;
                return Ok(config);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(ConfigError::Read {
                    path: path.clone(),
                    source,
                });
            }
        }
    }

    let config = AppConfig::default();
    config.validate(None)?;
    Ok(config)
}

fn config_paths_from_env(mut get_env: impl FnMut(&str) -> Option<OsString>) -> Vec<PathBuf> {
    let mut paths = Vec::new();

    if let Some(path) = get_env("KNIGHTTY_CONFIG").filter(|value| !value.is_empty()) {
        paths.push(PathBuf::from(path));
    }

    if let Some(path) = user_config_path_from_env(get_env) {
        paths.push(path);
    }

    paths
}

fn user_config_path_from_env(mut get_env: impl FnMut(&str) -> Option<OsString>) -> Option<PathBuf> {
    #[cfg(windows)]
    {
        get_env("APPDATA")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .map(|path| path.join("knightty").join(CONFIG_FILE_NAME))
    }

    #[cfg(not(windows))]
    {
        if let Some(config_home) = get_env("XDG_CONFIG_HOME").filter(|value| !value.is_empty()) {
            return Some(
                PathBuf::from(config_home)
                    .join("knightty")
                    .join(CONFIG_FILE_NAME),
            );
        }

        get_env("HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .map(|path| path.join(".config").join("knightty").join(CONFIG_FILE_NAME))
    }
}

pub fn supported_config_keys() -> &'static [&'static str] {
    SUPPORTED_CONFIG_KEYS
}

pub fn parse_wgpu_backend_name(value: &str) -> Option<&'static str> {
    match value.to_ascii_lowercase().as_str() {
        "auto" => Some("auto"),
        "vulkan" => Some("vulkan"),
        "dx12" => Some("dx12"),
        "gl" => Some("gl"),
        _ => None,
    }
}

fn validate_positive_float(
    field: &'static str,
    value: Option<f32>,
    source: Option<&Path>,
) -> Result<(), ConfigError> {
    if let Some(value) = value
        && (!value.is_finite() || value <= 0.0)
    {
        return Err(ConfigError::InvalidValue {
            path: source.map(Path::to_path_buf),
            field,
            message: format!("expected a positive number, got `{value}`"),
        });
    }
    Ok(())
}

fn validate_positive_usize(
    field: &'static str,
    value: Option<usize>,
    source: Option<&Path>,
) -> Result<(), ConfigError> {
    if matches!(value, Some(0)) {
        return Err(ConfigError::InvalidValue {
            path: source.map(Path::to_path_buf),
            field,
            message: "expected a positive integer, got `0`".to_owned(),
        });
    }
    Ok(())
}

fn validate_non_empty_string(
    field: &'static str,
    value: Option<&str>,
    source: Option<&Path>,
) -> Result<(), ConfigError> {
    if matches!(value, Some(value) if value.is_empty()) {
        return Err(ConfigError::InvalidValue {
            path: source.map(Path::to_path_buf),
            field,
            message: "expected a non-empty string".to_owned(),
        });
    }
    Ok(())
}

fn validate_usize_range(
    field: &'static str,
    value: Option<usize>,
    min: usize,
    max: usize,
    source: Option<&Path>,
) -> Result<(), ConfigError> {
    if let Some(value) = value
        && !(min..=max).contains(&value)
    {
        return Err(ConfigError::InvalidValue {
            path: source.map(Path::to_path_buf),
            field,
            message: format!("expected an integer from {min} to {max}, got `{value}`"),
        });
    }
    Ok(())
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("failed to read config `{}`: {source}", path.display())]
    Read { path: PathBuf, source: io::Error },
    #[error("failed to parse config `{}`: {source}", path.display())]
    Parse {
        path: PathBuf,
        source: toml::de::Error,
    },
    #[error("invalid config{} field `{field}`: {message}", path.as_ref().map(|path| format!(" `{}`", path.display())).unwrap_or_default())]
    InvalidValue {
        path: Option<PathBuf>,
        field: &'static str,
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use super::{
        AppConfig, CONFIG_FILE_NAME, ConfigError, config_paths_from_env,
        load_app_config_from_paths, user_config_path_from_env,
    };
    use std::collections::HashMap;
    use std::ffi::OsString;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use knightty_render::{
        BackgroundKind, CursorStyle, GradientOrientation, ImageFit, Rgba, TabStyle, WindowBackdrop,
    };

    static NEXT_ID: AtomicUsize = AtomicUsize::new(0);

    #[test]
    fn missing_config_uses_defaults() {
        let config = load_app_config_from_paths(&[temp_path("missing").join(CONFIG_FILE_NAME)])
            .expect("missing config should fall back to defaults");

        assert_eq!(config.initial_cols(), 80);
        assert_eq!(config.initial_rows(), 24);
        assert_eq!(config.renderer_config().cell_metrics.font_size, 16.0);
        assert_eq!(config.renderer_config().font_family, None);
        assert_eq!(config.scrollback_lines(), 10_000);
        assert_eq!(config.scroll_multiplier(), 3);
        assert!(config.hyperlink_open_on_ctrl_click());
        assert_eq!(
            config.hyperlink_allowed_schemes(),
            &["https".to_owned(), "http".to_owned()]
        );
        assert!(config.hyperlink_underline_on_hover());
        assert!(config.hyperlink_open_enabled());
    }

    #[test]
    fn env_config_path_is_first_candidate() {
        let paths = config_paths_from_env(env_map([
            ("KNIGHTTY_CONFIG", "C:\\custom\\knightty.config"),
            ("APPDATA", "C:\\Users\\me\\AppData\\Roaming"),
        ]));

        assert_eq!(
            paths.first(),
            Some(&PathBuf::from("C:\\custom\\knightty.config"))
        );
    }

    #[test]
    fn explicit_path_is_loaded_before_user_config_path() {
        let explicit_dir = temp_path("explicit");
        let user_dir = temp_path("user");
        fs::create_dir_all(&explicit_dir).expect("create explicit dir");
        fs::create_dir_all(&user_dir).expect("create user dir");

        let explicit_path = explicit_dir.join(CONFIG_FILE_NAME);
        let user_path = user_dir.join(CONFIG_FILE_NAME);
        fs::write(&explicit_path, "[window]\ninitial_cols = 120\n").expect("write explicit config");
        fs::write(&user_path, "[window]\ninitial_cols = 90\n").expect("write user config");

        let config = load_app_config_from_paths(&[explicit_path, user_path]).expect("load config");

        assert_eq!(config.initial_cols(), 120);
    }

    #[test]
    fn loaded_config_records_source_path_for_reload() {
        let dir = temp_path("source-path");
        fs::create_dir_all(&dir).expect("create dir");
        let path = dir.join(CONFIG_FILE_NAME);
        fs::write(&path, "[window]\ninitial_cols = 120\n").expect("write config");

        let config = load_app_config_from_paths(std::slice::from_ref(&path)).expect("load config");

        assert_eq!(config.source_path, Some(path));
    }

    #[test]
    fn invalid_toml_reports_parse_error() {
        let dir = temp_path("invalid-toml");
        fs::create_dir_all(&dir).expect("create dir");
        let path = dir.join(CONFIG_FILE_NAME);
        fs::write(&path, "[window\ninitial_cols = 80\n").expect("write invalid config");

        let error = load_app_config_from_paths(std::slice::from_ref(&path))
            .expect_err("invalid TOML errors");

        assert!(matches!(error, ConfigError::Parse { path: error_path, .. } if error_path == path));
    }

    #[test]
    fn font_settings_build_renderer_config() {
        let config: AppConfig = toml::from_str(
            r#"
[font]
family = "CaskaydiaCove Nerd Font"
size = 18
line_height = 22
"#,
        )
        .expect("parse config");

        let renderer_config = config.renderer_config();

        assert_eq!(
            renderer_config.font_family,
            Some("CaskaydiaCove Nerd Font".to_owned())
        );
        assert_eq!(renderer_config.cell_metrics.font_size, 18.0);
        assert_eq!(renderer_config.cell_metrics.line_height, 22.0);
        assert_eq!(renderer_config.cell_metrics.height, 22);
    }

    #[test]
    fn appearance_settings_build_renderer_config() {
        let config: AppConfig = toml::from_str(
            r##"
theme = "Catppuccin Mocha"

[window]
opacity = 1.5
padding_x = 12
padding_y = 10
unfocused_opacity = 0.92
blur = true
blur_radius = 999

[window.windows]
backdrop = "mica"

[cursor]
style = "bar"
blink = false

[colors]
background = "#010203"
foreground = "#040506"
selection_background = "#070809"
selection_foreground = "#0a0b0c"
cursor = "#0d0e0f"
cursor_text = "#101112"

[colors.normal]
red = "#131415"

[colors.bright]
blue = "#161718"

[hyperlink]
hover_underline = false
hover_foreground = "#89b4fa"
hover_background = "#313244"

[search]
foreground = "#010101"
background = "#020202"
selected_foreground = "#030303"
selected_background = "#040404"

[panes]
inactive_opacity = 2.0
inactive_tint = "#181825"

[background]
kind = "gradient"

[background.gradient]
orientation = "horizontal"
colors = ["#1e1e2e", "#11111b"]

[tabs]
enabled = true
show_when_single = true
style = "separator"
active_background = "#111111"
active_foreground = "#222222"
inactive_background = "#333333"
inactive_foreground = "#444444"

[effects]
retro_crt = true
scanlines = true
"##,
        )
        .expect("parse appearance config");

        let appearance = config.renderer_config().appearance;

        assert_eq!(appearance.window.opacity, 1.0);
        assert_eq!(appearance.window.padding_x, 12);
        assert_eq!(appearance.window.padding_y, 10);
        assert_eq!(appearance.window.unfocused_opacity, 0.92);
        assert!(appearance.window.blur);
        assert_eq!(appearance.window.blur_radius, 100);
        assert_eq!(appearance.window.backdrop, WindowBackdrop::Mica);
        assert_eq!(appearance.cursor.style, CursorStyle::Bar);
        assert!(!appearance.cursor.blink);
        assert_eq!(appearance.theme.background, Rgba::rgb(1, 2, 3));
        assert_eq!(appearance.theme.foreground, Rgba::rgb(4, 5, 6));
        assert_eq!(appearance.theme.selection_background, Rgba::rgb(7, 8, 9));
        assert_eq!(appearance.theme.selection_foreground, Rgba::rgb(10, 11, 12));
        assert_eq!(appearance.theme.cursor, Rgba::rgb(13, 14, 15));
        assert_eq!(appearance.theme.cursor_text, Rgba::rgb(16, 17, 18));
        assert_eq!(appearance.theme.normal[1], Rgba::rgb(19, 20, 21));
        assert_eq!(appearance.theme.bright[4], Rgba::rgb(22, 23, 24));
        assert!(!appearance.hyperlink.hover_underline);
        assert_eq!(
            appearance.hyperlink.hover_foreground,
            Some(Rgba::rgb(137, 180, 250))
        );
        assert_eq!(appearance.search.selected_background, Rgba::rgb(4, 4, 4));
        assert_eq!(appearance.panes.inactive_opacity, 1.0);
        assert!(matches!(
            &appearance.background.kind,
            BackgroundKind::Gradient(_)
        ));
        if let BackgroundKind::Gradient(gradient) = &appearance.background.kind {
            assert_eq!(gradient.orientation, GradientOrientation::Horizontal);
            assert_eq!(gradient.colors.len(), 2);
        }
        assert!(appearance.tabs.enabled);
        assert_eq!(appearance.tabs.style, TabStyle::Separator);
        assert!(appearance.effects.retro_crt);
        assert!(appearance.effects.scanlines);
    }

    #[test]
    fn theme_pair_light_mode_selects_light_theme() {
        let config: AppConfig = toml::from_str(
            r#"
[theme]
light = "Catppuccin Latte"
dark = "Catppuccin Mocha"
mode = "light"
"#,
        )
        .expect("parse theme pair config");

        assert_eq!(
            config.renderer_appearance().theme.background,
            Rgba::rgb(239, 241, 245)
        );
    }

    #[test]
    fn unknown_theme_falls_back_to_default_theme() {
        let config: AppConfig =
            toml::from_str("theme = \"No Such Theme\"\n").expect("parse unknown theme config");

        assert_eq!(
            config.renderer_appearance().theme.background,
            Rgba::rgb(30, 30, 46)
        );
    }

    #[test]
    fn invalid_color_string_is_parse_error() {
        let error = toml::from_str::<AppConfig>("[colors]\nbackground = \"not-a-color\"\n")
            .expect_err("invalid color should fail parsing");

        assert!(error.to_string().contains("expected #RRGGBB"));
    }

    #[test]
    fn background_image_missing_file_falls_back_to_solid() {
        let config: AppConfig = toml::from_str(
            r#"
[background]
kind = "image"

[background.image]
path = "missing.png"
"#,
        )
        .expect("parse image config");

        assert!(matches!(
            config.renderer_appearance().background.kind,
            BackgroundKind::Solid
        ));
    }

    #[test]
    fn background_image_path_resolves_relative_to_config_file() {
        let dir = temp_path("image-config");
        let wallpaper_dir = dir.join("wallpapers");
        fs::create_dir_all(&wallpaper_dir).expect("create wallpaper dir");
        let image_path = wallpaper_dir.join("knightty.png");
        fs::write(&image_path, b"not decoded yet").expect("write placeholder png");
        let config_path = dir.join(CONFIG_FILE_NAME);
        fs::write(
            &config_path,
            r##"
[background]
kind = "image"

[background.image]
path = "wallpapers/knightty.png"
opacity = 0.3
fit = "contain"
tint = "#010203"
tint_opacity = 0.4
"##,
        )
        .expect("write image config");

        let config = load_app_config_from_paths(std::slice::from_ref(&config_path))
            .expect("load image config");
        let appearance = config.renderer_appearance();

        let BackgroundKind::Image(image) = appearance.background.kind else {
            panic!("image background should be resolved");
        };
        assert_eq!(image.path, image_path);
        assert_eq!(image.opacity, 0.3);
        assert_eq!(image.fit, ImageFit::Contain);
        assert_eq!(image.tint, Rgba::rgb(1, 2, 3));
        assert_eq!(image.tint_opacity, 0.4);
    }

    #[test]
    fn terminal_settings_are_loaded() {
        let config: AppConfig = toml::from_str(
            r#"
[terminal]
scrollback_lines = 2000
scroll_multiplier = 5
"#,
        )
        .expect("parse config");

        assert_eq!(config.scrollback_lines(), 2000);
        assert_eq!(config.scroll_multiplier(), 5);
    }

    #[test]
    fn graphics_settings_are_loaded_with_secure_defaults() {
        let config: AppConfig = toml::from_str(
            r#"
[graphics]
enabled = false
max_encoded_bytes = 1024
max_decoded_bytes = 4096
max_width = 128
max_height = 64
max_pixels = 8192
max_images = 4
max_gpu_bytes = 16384
"#,
        )
        .expect("parse graphics config");

        assert!(!config.graphics.enabled);
        assert_eq!(config.graphics.max_encoded_bytes, 1024);
        assert_eq!(config.graphics.max_decoded_bytes, 4096);
        assert_eq!(config.graphics.max_width, 128);
        assert_eq!(config.graphics.max_height, 64);
        assert_eq!(config.graphics.max_pixels, 8192);
        assert_eq!(config.graphics.max_images, 4);
        assert_eq!(config.graphics.max_gpu_bytes, 16384);
        config.validate(None).expect("graphics limits validate");
    }

    #[test]
    fn zero_graphics_limit_is_rejected() {
        let config: AppConfig =
            toml::from_str("[graphics]\nmax_images = 0\n").expect("parse graphics config");

        let error = config
            .validate(None)
            .expect_err("zero graphics limit should fail");

        assert!(matches!(
            error,
            ConfigError::InvalidValue {
                field: "graphics.max_images",
                ..
            }
        ));
    }

    #[test]
    fn hyperlink_allowed_schemes_are_lowercase_normalized() {
        let config: AppConfig = toml::from_str(
            r#"
[hyperlink]
allowed_schemes = ["HTTPS", "Http"]
"#,
        )
        .expect("parse config");

        assert_eq!(
            config.hyperlink_allowed_schemes(),
            &["https".to_owned(), "http".to_owned()]
        );
    }

    #[test]
    fn empty_hyperlink_allowed_schemes_disable_open() {
        let config: AppConfig = toml::from_str(
            r#"
[hyperlink]
allowed_schemes = []
"#,
        )
        .expect("parse config");

        assert!(config.hyperlink_open_on_ctrl_click());
        assert!(config.hyperlink_allowed_schemes().is_empty());
        assert!(!config.hyperlink_open_enabled());
    }

    #[test]
    fn shell_settings_build_shell_command() {
        let config: AppConfig = toml::from_str(
            r#"
[shell]
program = "pwsh"
args = ["-NoLogo"]
"#,
        )
        .expect("parse config");

        let shell = config.shell_command().expect("shell should be configured");

        assert_eq!(shell.program, "pwsh");
        assert_eq!(shell.args, vec!["-NoLogo"]);
    }

    #[test]
    fn missing_shell_settings_use_default_shell() {
        let config = AppConfig::default();

        assert_eq!(config.shell_command(), None);
    }

    #[test]
    fn generated_default_config_loads_from_path() {
        let dir = temp_path("generated-default-config");
        fs::create_dir_all(&dir).expect("create dir");
        let path = dir.join(CONFIG_FILE_NAME);
        fs::write(&path, crate::config_spec::default_config_toml())
            .expect("write generated default config");

        let config = load_app_config_from_paths(std::slice::from_ref(&path))
            .expect("load generated default config");

        assert_eq!(config.initial_cols(), AppConfig::default().initial_cols());
        assert_eq!(config.initial_rows(), AppConfig::default().initial_rows());
        assert_eq!(
            config.scrollback_lines(),
            AppConfig::default().scrollback_lines()
        );
        assert_eq!(
            config.scroll_multiplier(),
            AppConfig::default().scroll_multiplier()
        );
    }

    #[test]
    fn terminal_scrollback_lines_are_range_validated() {
        let dir = temp_path("invalid-scrollback-lines");
        fs::create_dir_all(&dir).expect("create dir");
        let path = dir.join(CONFIG_FILE_NAME);
        fs::write(&path, "[terminal]\nscrollback_lines = 100001\n").expect("write invalid config");

        let error = load_app_config_from_paths(std::slice::from_ref(&path))
            .expect_err("invalid scrollback_lines errors");

        assert!(matches!(
            error,
            ConfigError::InvalidValue {
                field: "terminal.scrollback_lines",
                ..
            }
        ));
    }

    #[test]
    fn terminal_scroll_multiplier_is_range_validated() {
        let dir = temp_path("invalid-scroll-multiplier");
        fs::create_dir_all(&dir).expect("create dir");
        let path = dir.join(CONFIG_FILE_NAME);
        fs::write(&path, "[terminal]\nscroll_multiplier = 0\n").expect("write invalid config");

        let error = load_app_config_from_paths(std::slice::from_ref(&path))
            .expect_err("invalid scroll_multiplier errors");

        assert!(matches!(
            error,
            ConfigError::InvalidValue {
                field: "terminal.scroll_multiplier",
                ..
            }
        ));
    }

    #[test]
    fn empty_shell_program_is_invalid() {
        let dir = temp_path("invalid-shell-program");
        fs::create_dir_all(&dir).expect("create dir");
        let path = dir.join(CONFIG_FILE_NAME);
        fs::write(&path, "[shell]\nprogram = \"\"\n").expect("write invalid config");

        let error = load_app_config_from_paths(std::slice::from_ref(&path))
            .expect_err("invalid shell program errors");

        assert!(matches!(
            error,
            ConfigError::InvalidValue {
                field: "shell.program",
                ..
            }
        ));
    }

    #[cfg(windows)]
    #[test]
    fn windows_user_config_path_uses_appdata() {
        let path = user_config_path_from_env(env_map([("APPDATA", "C:\\Users\\me\\Roaming")]))
            .expect("appdata path");

        assert_eq!(
            path,
            PathBuf::from("C:\\Users\\me\\Roaming")
                .join("knightty")
                .join(CONFIG_FILE_NAME)
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn unix_user_config_path_prefers_xdg_config_home() {
        let path = user_config_path_from_env(env_map([
            ("XDG_CONFIG_HOME", "/tmp/config"),
            ("HOME", "/home/me"),
        ]))
        .expect("xdg path");

        assert_eq!(path, PathBuf::from("/tmp/config/knightty/knightty.config"));
    }

    fn env_map<const N: usize>(
        values: [(&'static str, &'static str); N],
    ) -> impl FnMut(&str) -> Option<OsString> {
        let values = values
            .into_iter()
            .map(|(key, value)| (key, OsString::from(value)))
            .collect::<HashMap<_, _>>();

        move |key| values.get(key).cloned()
    }

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "knightty-config-test-{}-{}-{name}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ))
    }
}
