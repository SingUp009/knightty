use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::mem;
use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use glyphon::cosmic_text::{FeatureTag, FontFeatures};
use glyphon::{
    Attrs, Buffer, Cache, Family, FontSystem, Metrics, Resolution, Shaping, Style as GlyphStyle,
    SwashCache, TextArea, TextAtlas, TextBounds, TextRenderer, Viewport, Weight, Wrap,
};
use image::ImageError;
use knightty_core::{
    Cell, CellSpanPlacement, Color, Cursor, Damage, GridSnapshot, ImageId, ImagePlacement,
};
use thiserror::Error;
use unicode_width::UnicodeWidthChar;
use wgpu::{
    Adapter, Backends, BlendState, BufferDescriptor, BufferUsages, ColorTargetState, ColorWrites,
    CommandEncoderDescriptor, CompositeAlphaMode, DeviceDescriptor, DeviceType, FragmentState,
    Instance, LoadOp, MultisampleState, Operations, PipelineCompilationOptions,
    PipelineLayoutDescriptor, PresentMode, PrimitiveState, RenderPass, RenderPassColorAttachment,
    RenderPassDescriptor, RenderPipeline, RenderPipelineDescriptor, ShaderModuleDescriptor,
    ShaderSource, StoreOp, SurfaceConfiguration, SurfaceTarget, TextureFormat, TextureUsages,
    TextureViewDescriptor, VertexAttribute, VertexBufferLayout, VertexFormat, VertexState,
    VertexStepMode,
};

pub const DEFAULT_THEME_NAME: &str = "Catppuccin Mocha";
pub const MAX_BLUR_RADIUS: u32 = 100;

const DEFAULT_FG: Rgba = Rgba::rgb(230, 230, 230);
const DEFAULT_BG: Rgba = Rgba::rgb(0, 0, 0);

/// Fixed terminal cell metrics used by the renderer and PTY resize path.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CellMetrics {
    pub width: u32,
    pub height: u32,
    pub font_size: f32,
    pub line_height: f32,
}

impl Default for CellMetrics {
    fn default() -> Self {
        Self {
            width: 9,
            height: 18,
            font_size: 16.0,
            line_height: 18.0,
        }
    }
}

impl CellMetrics {
    pub fn from_font_size(font_size: f32, line_height: f32) -> Self {
        let default = Self::default();
        let width = (font_size * default.width as f32 / default.font_size)
            .round()
            .max(1.0) as u32;
        let height = line_height.round().max(1.0) as u32;

        Self {
            width,
            height,
            font_size,
            line_height,
        }
    }

    pub fn cols_for_width(self, width: u32) -> usize {
        (width / self.width.max(1)).max(1) as usize
    }

    pub fn rows_for_height(self, height: u32) -> usize {
        (height / self.height.max(1)).max(1) as usize
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RendererConfig {
    pub cell_metrics: CellMetrics,
    pub font_family: Option<String>,
    pub appearance: RendererAppearance,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FontFamilyInfo {
    pub name: String,
    pub monospaced: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RenderPlan {
    pub text: Vec<TextSegmentPlan>,
    pub rects: Vec<RectPlan>,
    pub selection_rects: Vec<RectPlan>,
    pub images: Vec<ImageQuadPlan>,
    pub hyperlink_spans: Vec<HyperlinkSpan>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImePreedit {
    pub text: String,
    pub cursor_range: Option<Range<usize>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RenderOptions {
    pub hovered_hyperlink_id: Option<usize>,
    pub search_matches: Vec<SearchMatch>,
    pub current_search_match: Option<SearchMatch>,
    pub search_query: Option<String>,
    pub cursor_blink_visible: bool,
    pub focused: bool,
    pub ime_preedit: Option<ImePreedit>,
    pub inline_images: Vec<InlineImageData>,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            hovered_hyperlink_id: None,
            search_matches: Vec::new(),
            current_search_match: None,
            search_query: None,
            cursor_blink_visible: true,
            focused: true,
            ime_preedit: None,
            inline_images: Vec::new(),
        }
    }
}

/// Decoded image pixels supplied by the application for GPU upload.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InlineImageData {
    pub id: ImageId,
    pub width: u32,
    pub height: u32,
    pub rgba: Arc<[u8]>,
}

/// Headless image draw plan in surface pixel coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImageQuadPlan {
    pub image_id: ImageId,
    pub destination: PixelRect,
    pub uv: UvRect,
    pub z_index: i32,
    pub client_image_id: u32,
    pub insertion_order: u64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PixelRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UvRect {
    pub u0: f32,
    pub v0: f32,
    pub u1: f32,
    pub v1: f32,
}

const BELOW_CELL_BACKGROUND: i32 = i32::MIN / 2;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InlineImageLayer {
    BelowCellBackground,
    BelowText,
    Zero,
    AboveText,
}

impl InlineImageLayer {
    const fn for_z_index(z_index: i32) -> Self {
        if z_index < BELOW_CELL_BACKGROUND {
            Self::BelowCellBackground
        } else if z_index < 0 {
            Self::BelowText
        } else if z_index == 0 {
            Self::Zero
        } else {
            Self::AboveText
        }
    }
}

/// Contiguous visible cells associated with one hyperlink.
///
/// `end_col` is exclusive.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HyperlinkSpan {
    pub hyperlink_id: usize,
    pub row: usize,
    pub start_col: usize,
    pub end_col: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TextSegmentPlan {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub text: String,
    pub style: TextStyle,
    pub cell_span: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TextStyle {
    pub fg: Rgba,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub inverse: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RectPlan {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub color: Rgba,
    pub layer: RectLayer,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RectLayer {
    Background,
    Overlay,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    pub const fn with_channels(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    pub fn with_alpha(self, opacity: f32) -> Self {
        let opacity = clamp_opacity(opacity);
        Self {
            a: (opacity * 255.0).round() as u8,
            ..self
        }
    }

    pub fn from_hex_rgb(value: &str) -> Result<Self, ColorParseError> {
        let Some(hex) = value.strip_prefix('#') else {
            return Err(ColorParseError::MissingHash);
        };
        if hex.len() != 6 {
            return Err(ColorParseError::InvalidLength(hex.len()));
        }

        let r = parse_hex_channel(&hex[0..2])?;
        let g = parse_hex_channel(&hex[2..4])?;
        let b = parse_hex_channel(&hex[4..6])?;
        Ok(Self::rgb(r, g, b))
    }

    fn to_glyphon(self) -> glyphon::Color {
        glyphon::Color::rgba(self.r, self.g, self.b, self.a)
    }

    fn to_f32(self) -> [f32; 4] {
        [
            f32::from(self.r) / 255.0,
            f32::from(self.g) / 255.0,
            f32::from(self.b) / 255.0,
            f32::from(self.a) / 255.0,
        ]
    }

    fn to_target_f32(self, target_is_srgb: bool) -> [f32; 4] {
        if target_is_srgb {
            [
                srgb_channel_to_linear(self.r),
                srgb_channel_to_linear(self.g),
                srgb_channel_to_linear(self.b),
                f32::from(self.a) / 255.0,
            ]
        } else {
            self.to_f32()
        }
    }
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum ColorParseError {
    #[error("expected #RRGGBB color")]
    MissingHash,
    #[error("expected 6 hex digits after #, got {0}")]
    InvalidLength(usize),
    #[error("invalid hex color channel `{0}`")]
    InvalidChannel(String),
}

fn parse_hex_channel(value: &str) -> Result<u8, ColorParseError> {
    u8::from_str_radix(value, 16).map_err(|_| ColorParseError::InvalidChannel(value.to_owned()))
}

fn srgb_channel_to_linear(value: u8) -> f32 {
    let value = f32::from(value) / 255.0;
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

pub fn clamp_opacity(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        1.0
    }
}

pub fn clamp_blur_radius(value: u32) -> u32 {
    value.min(MAX_BLUR_RADIUS)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResolvedTheme {
    pub background: Rgba,
    pub foreground: Rgba,
    pub selection_background: Rgba,
    pub selection_foreground: Rgba,
    pub cursor: Rgba,
    pub cursor_text: Rgba,
    pub normal: [Rgba; 8],
    pub bright: [Rgba; 8],
}

impl Default for ResolvedTheme {
    fn default() -> Self {
        builtin_theme(DEFAULT_THEME_NAME).expect("default built-in theme should exist")
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CatppuccinFlavor {
    Latte,
    Frappe,
    Macchiato,
    Mocha,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BuiltinTheme {
    pub name: &'static str,
    pub flavor: Option<CatppuccinFlavor>,
    pub theme: ResolvedTheme,
}

pub const BUILTIN_THEME_NAMES: &[&str] = &[
    "Catppuccin Latte",
    "Catppuccin Frappe",
    "Catppuccin Macchiato",
    "Catppuccin Mocha",
];

pub fn builtin_theme_names() -> &'static [&'static str] {
    BUILTIN_THEME_NAMES
}

pub fn builtin_theme(name: &str) -> Option<ResolvedTheme> {
    Some(match name {
        "Catppuccin Latte" => catppuccin_theme(CatppuccinFlavor::Latte),
        "Catppuccin Frappe" => catppuccin_theme(CatppuccinFlavor::Frappe),
        "Catppuccin Macchiato" => catppuccin_theme(CatppuccinFlavor::Macchiato),
        "Catppuccin Mocha" => catppuccin_theme(CatppuccinFlavor::Mocha),
        _ => return None,
    })
}

pub fn builtin_themes() -> [BuiltinTheme; 4] {
    [
        BuiltinTheme {
            name: "Catppuccin Latte",
            flavor: Some(CatppuccinFlavor::Latte),
            theme: catppuccin_theme(CatppuccinFlavor::Latte),
        },
        BuiltinTheme {
            name: "Catppuccin Frappe",
            flavor: Some(CatppuccinFlavor::Frappe),
            theme: catppuccin_theme(CatppuccinFlavor::Frappe),
        },
        BuiltinTheme {
            name: "Catppuccin Macchiato",
            flavor: Some(CatppuccinFlavor::Macchiato),
            theme: catppuccin_theme(CatppuccinFlavor::Macchiato),
        },
        BuiltinTheme {
            name: "Catppuccin Mocha",
            flavor: Some(CatppuccinFlavor::Mocha),
            theme: catppuccin_theme(CatppuccinFlavor::Mocha),
        },
    ]
}

pub fn resolve_theme_name(name: Option<&str>) -> (ResolvedTheme, Option<String>) {
    let Some(name) = name else {
        return (ResolvedTheme::default(), None);
    };
    match builtin_theme(name) {
        Some(theme) => (theme, None),
        None => (
            ResolvedTheme::default(),
            Some(format!(
                "unknown theme `{name}`; falling back to {DEFAULT_THEME_NAME}. Available themes: {}",
                builtin_theme_names().join(", ")
            )),
        ),
    }
}

fn catppuccin_theme(flavor: CatppuccinFlavor) -> ResolvedTheme {
    match flavor {
        CatppuccinFlavor::Latte => ResolvedTheme {
            background: Rgba::rgb(239, 241, 245),
            foreground: Rgba::rgb(76, 79, 105),
            selection_background: Rgba::rgb(188, 192, 204),
            selection_foreground: Rgba::rgb(76, 79, 105),
            cursor: Rgba::rgb(220, 138, 120),
            cursor_text: Rgba::rgb(239, 241, 245),
            normal: [
                Rgba::rgb(188, 192, 204),
                Rgba::rgb(210, 15, 57),
                Rgba::rgb(64, 160, 43),
                Rgba::rgb(223, 142, 29),
                Rgba::rgb(30, 102, 245),
                Rgba::rgb(234, 118, 203),
                Rgba::rgb(23, 146, 153),
                Rgba::rgb(92, 95, 119),
            ],
            bright: [
                Rgba::rgb(172, 176, 190),
                Rgba::rgb(210, 15, 57),
                Rgba::rgb(64, 160, 43),
                Rgba::rgb(223, 142, 29),
                Rgba::rgb(30, 102, 245),
                Rgba::rgb(234, 118, 203),
                Rgba::rgb(23, 146, 153),
                Rgba::rgb(76, 79, 105),
            ],
        },
        CatppuccinFlavor::Frappe => ResolvedTheme {
            background: Rgba::rgb(48, 52, 70),
            foreground: Rgba::rgb(198, 208, 245),
            selection_background: Rgba::rgb(81, 87, 109),
            selection_foreground: Rgba::rgb(198, 208, 245),
            cursor: Rgba::rgb(242, 213, 207),
            cursor_text: Rgba::rgb(48, 52, 70),
            normal: [
                Rgba::rgb(81, 87, 109),
                Rgba::rgb(231, 130, 132),
                Rgba::rgb(166, 209, 137),
                Rgba::rgb(229, 200, 144),
                Rgba::rgb(140, 170, 238),
                Rgba::rgb(244, 184, 228),
                Rgba::rgb(129, 200, 190),
                Rgba::rgb(181, 191, 226),
            ],
            bright: [
                Rgba::rgb(98, 104, 128),
                Rgba::rgb(231, 130, 132),
                Rgba::rgb(166, 209, 137),
                Rgba::rgb(229, 200, 144),
                Rgba::rgb(140, 170, 238),
                Rgba::rgb(244, 184, 228),
                Rgba::rgb(129, 200, 190),
                Rgba::rgb(198, 208, 245),
            ],
        },
        CatppuccinFlavor::Macchiato => ResolvedTheme {
            background: Rgba::rgb(36, 39, 58),
            foreground: Rgba::rgb(202, 211, 245),
            selection_background: Rgba::rgb(73, 77, 100),
            selection_foreground: Rgba::rgb(202, 211, 245),
            cursor: Rgba::rgb(244, 219, 214),
            cursor_text: Rgba::rgb(36, 39, 58),
            normal: [
                Rgba::rgb(73, 77, 100),
                Rgba::rgb(237, 135, 150),
                Rgba::rgb(166, 218, 149),
                Rgba::rgb(238, 212, 159),
                Rgba::rgb(138, 173, 244),
                Rgba::rgb(245, 189, 230),
                Rgba::rgb(139, 213, 202),
                Rgba::rgb(184, 192, 224),
            ],
            bright: [
                Rgba::rgb(91, 96, 120),
                Rgba::rgb(237, 135, 150),
                Rgba::rgb(166, 218, 149),
                Rgba::rgb(238, 212, 159),
                Rgba::rgb(138, 173, 244),
                Rgba::rgb(245, 189, 230),
                Rgba::rgb(139, 213, 202),
                Rgba::rgb(202, 211, 245),
            ],
        },
        CatppuccinFlavor::Mocha => ResolvedTheme {
            background: Rgba::rgb(30, 30, 46),
            foreground: Rgba::rgb(205, 214, 244),
            selection_background: Rgba::rgb(69, 71, 90),
            selection_foreground: Rgba::rgb(205, 214, 244),
            cursor: Rgba::rgb(245, 224, 220),
            cursor_text: Rgba::rgb(30, 30, 46),
            normal: [
                Rgba::rgb(69, 71, 90),
                Rgba::rgb(243, 139, 168),
                Rgba::rgb(166, 227, 161),
                Rgba::rgb(249, 226, 175),
                Rgba::rgb(137, 180, 250),
                Rgba::rgb(245, 194, 231),
                Rgba::rgb(148, 226, 213),
                Rgba::rgb(186, 194, 222),
            ],
            bright: [
                Rgba::rgb(88, 91, 112),
                Rgba::rgb(243, 139, 168),
                Rgba::rgb(166, 227, 161),
                Rgba::rgb(249, 226, 175),
                Rgba::rgb(137, 180, 250),
                Rgba::rgb(245, 194, 231),
                Rgba::rgb(148, 226, 213),
                Rgba::rgb(166, 173, 200),
            ],
        },
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RendererAppearance {
    pub theme: ResolvedTheme,
    pub window: WindowAppearance,
    pub cursor: CursorAppearance,
    pub hyperlink: HyperlinkAppearance,
    pub search: SearchAppearance,
    pub panes: PaneAppearance,
    pub background: BackgroundAppearance,
    pub tabs: TabAppearance,
    pub effects: EffectAppearance,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindowAppearance {
    pub opacity: f32,
    pub padding_x: u32,
    pub padding_y: u32,
    pub unfocused_opacity: f32,
    pub blur: bool,
    pub blur_radius: u32,
    pub backdrop: WindowBackdrop,
}

impl Default for WindowAppearance {
    fn default() -> Self {
        Self {
            opacity: 1.0,
            padding_x: 0,
            padding_y: 0,
            unfocused_opacity: 1.0,
            blur: false,
            blur_radius: 20,
            backdrop: WindowBackdrop::None,
        }
    }
}

impl WindowAppearance {
    pub fn normalized(self) -> Self {
        Self {
            opacity: clamp_opacity(self.opacity),
            unfocused_opacity: clamp_opacity(self.unfocused_opacity),
            blur_radius: clamp_blur_radius(self.blur_radius),
            ..self
        }
    }

    pub fn usable_width(self, width: u32) -> u32 {
        width.saturating_sub(self.padding_x.saturating_mul(2))
    }

    pub fn usable_height(self, height: u32) -> u32 {
        height.saturating_sub(self.padding_y.saturating_mul(2))
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum WindowBackdrop {
    #[default]
    None,
    Acrylic,
    Mica,
    Tabbed,
}

impl fmt::Display for WindowBackdrop {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::None => "none",
            Self::Acrylic => "acrylic",
            Self::Mica => "mica",
            Self::Tabbed => "tabbed",
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CursorAppearance {
    pub style: CursorStyle,
    pub blink: bool,
}

impl Default for CursorAppearance {
    fn default() -> Self {
        Self {
            style: CursorStyle::Block,
            blink: true,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CursorStyle {
    Block,
    Bar,
    Underline,
    HollowBlock,
}

impl CursorStyle {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "block" => Some(Self::Block),
            "bar" => Some(Self::Bar),
            "underline" => Some(Self::Underline),
            "hollow_block" => Some(Self::HollowBlock),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Block => "block",
            Self::Bar => "bar",
            Self::Underline => "underline",
            Self::HollowBlock => "hollow_block",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HyperlinkAppearance {
    pub hover_underline: bool,
    pub hover_foreground: Option<Rgba>,
    pub hover_background: Option<Rgba>,
}

impl Default for HyperlinkAppearance {
    fn default() -> Self {
        Self {
            hover_underline: true,
            hover_foreground: None,
            hover_background: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SearchAppearance {
    pub foreground: Rgba,
    pub background: Rgba,
    pub selected_foreground: Rgba,
    pub selected_background: Rgba,
}

impl Default for SearchAppearance {
    fn default() -> Self {
        Self {
            foreground: Rgba::rgb(30, 30, 46),
            background: Rgba::rgb(249, 226, 175),
            selected_foreground: Rgba::rgb(30, 30, 46),
            selected_background: Rgba::rgb(250, 179, 135),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PaneAppearance {
    pub inactive_opacity: f32,
    pub inactive_tint: Rgba,
}

impl Default for PaneAppearance {
    fn default() -> Self {
        Self {
            inactive_opacity: 1.0,
            inactive_tint: Rgba::rgb(24, 24, 37),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct BackgroundAppearance {
    pub kind: BackgroundKind,
}

impl Default for BackgroundAppearance {
    fn default() -> Self {
        Self {
            kind: BackgroundKind::Solid,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum BackgroundKind {
    Solid,
    Gradient(GradientBackground),
    Image(ImageBackground),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GradientBackground {
    pub orientation: GradientOrientation,
    pub colors: Vec<Rgba>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GradientOrientation {
    Vertical,
    Horizontal,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ImageBackground {
    pub path: PathBuf,
    pub opacity: f32,
    pub fit: ImageFit,
    pub tint: Rgba,
    pub tint_opacity: f32,
    pub load_state: ImageLoadState,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImageFit {
    Contain,
    Cover,
    Stretch,
    Tile,
    Center,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImageLoadState {
    Ready,
    Missing,
    UnsupportedFormat,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TabAppearance {
    pub enabled: bool,
    pub show_when_single: bool,
    pub style: TabStyle,
    pub active_background: Rgba,
    pub active_foreground: Rgba,
    pub inactive_background: Rgba,
    pub inactive_foreground: Rgba,
}

impl Default for TabAppearance {
    fn default() -> Self {
        Self {
            enabled: false,
            show_when_single: false,
            style: TabStyle::Minimal,
            active_background: Rgba::rgb(49, 50, 68),
            active_foreground: Rgba::rgb(205, 214, 244),
            inactive_background: Rgba::rgb(24, 24, 37),
            inactive_foreground: Rgba::rgb(127, 132, 156),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TabStyle {
    Minimal,
    Separator,
    Powerline,
    Slant,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct EffectAppearance {
    pub retro_crt: bool,
    pub scanlines: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SearchMatch {
    pub row: usize,
    pub start_col: usize,
    pub end_col: usize,
}

impl SearchMatch {
    pub fn contains(&self, col: usize, row: usize) -> bool {
        self.row == row && col >= self.start_col && col < self.end_col
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CellVisualState {
    pub selected: bool,
    pub search_match: bool,
    pub current_search_match: bool,
    pub hyperlink_hover: bool,
    pub block_cursor: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CellVisual {
    pub fg: Rgba,
    pub bg: Option<Rgba>,
    pub underline: bool,
    pub inverse: bool,
}

pub struct Renderer {
    _instance: Instance,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface_config: SurfaceConfiguration,
    cell_metrics: CellMetrics,
    font_family: Option<String>,
    appearance: RendererAppearance,
    font_system: FontSystem,
    swash_cache: SwashCache,
    viewport: Viewport,
    atlas: TextAtlas,
    text_renderer: TextRenderer,
    text_buffers: Vec<Buffer>,
    prepared_text: Vec<PreparedTextArea>,
    rect_pipeline: RectPipeline,
    image_pipeline: ImagePipeline,
    background_image: Option<BackgroundImageTexture>,
    inline_images: BTreeMap<ImageId, InlineImageTexture>,
}

impl Renderer {
    pub async fn new(
        instance: Instance,
        surface: wgpu::Surface<'static>,
        width: u32,
        height: u32,
    ) -> Result<Self, RenderError> {
        Self::with_config(instance, surface, width, height, RendererConfig::default()).await
    }

    pub async fn with_config(
        instance: Instance,
        surface: wgpu::Surface<'static>,
        width: u32,
        height: u32,
        config: RendererConfig,
    ) -> Result<Self, RenderError> {
        let adapter = select_adapter(&instance, &surface).await?;
        let adapter_info = adapter.get_info();
        eprintln!(
            "knightty renderer: adapter=\"{}\" backend={:?} device_type={:?} vendor=0x{:04x} device=0x{:04x} driver=\"{}\" driver_info=\"{}\"",
            adapter_info.name,
            adapter_info.backend,
            adapter_info.device_type,
            adapter_info.vendor,
            adapter_info.device,
            adapter_info.driver,
            adapter_info.driver_info,
        );
        if adapter_info.device_type == DeviceType::Cpu {
            eprintln!(
                "knightty renderer: warning: selected CPU/software adapter; rendering is expected to be slow and should only be used as a fallback"
            );
        }
        let (device, queue) = adapter.request_device(&DeviceDescriptor::default()).await?;
        device.on_uncaptured_error(Arc::new(|error| match error {
            wgpu::Error::OutOfMemory { .. } => {
                eprintln!("knightty renderer: GPU out of memory");
            }
            wgpu::Error::Validation { description, .. } => {
                eprintln!("knightty renderer: wgpu validation error: {description}");
            }
            wgpu::Error::Internal { description, .. } => {
                eprintln!("knightty renderer: wgpu internal error: {description}");
            }
        }));
        let capabilities = surface.get_capabilities(&adapter);
        let format = capabilities
            .formats
            .iter()
            .copied()
            .find(TextureFormat::is_srgb)
            .or_else(|| capabilities.formats.first().copied())
            .ok_or(RenderError::NoSurfaceFormat)?;

        let appearance = RendererAppearance {
            window: config.appearance.window.normalized(),
            ..config.appearance
        };
        let surface_config = SurfaceConfiguration {
            usage: TextureUsages::RENDER_ATTACHMENT,
            format,
            width: width.max(1),
            height: height.max(1),
            present_mode: PresentMode::Fifo,
            alpha_mode: surface_alpha_mode(&capabilities.alpha_modes, appearance.window.opacity),
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &surface_config);

        let cell_metrics = config.cell_metrics;
        let mut font_system = FontSystem::new();
        let swash_cache = SwashCache::new();
        let cache = Cache::new(&device);
        let viewport = Viewport::new(&device, &cache);
        let mut atlas = TextAtlas::new(&device, &queue, &cache, format);
        let text_renderer =
            TextRenderer::new(&mut atlas, &device, MultisampleState::default(), None);
        let text_buffer = new_text_buffer(&mut font_system, cell_metrics);

        let rect_pipeline = RectPipeline::new(&device, format);
        let image_pipeline = ImagePipeline::new(&device, format);
        let background_image = load_background_image(&device, &queue, &image_pipeline, &appearance);

        Ok(Self {
            _instance: instance,
            surface,
            device,
            queue,
            surface_config,
            cell_metrics,
            font_family: config.font_family,
            appearance,
            font_system,
            swash_cache,
            viewport,
            atlas,
            text_renderer,
            text_buffers: vec![text_buffer],
            prepared_text: Vec::new(),
            rect_pipeline,
            image_pipeline,
            background_image,
            inline_images: BTreeMap::new(),
        })
    }

    pub fn cell_metrics(&self) -> CellMetrics {
        self.cell_metrics
    }

    pub fn update_config(&mut self, config: RendererConfig) {
        let appearance = RendererAppearance {
            window: config.appearance.window.normalized(),
            ..config.appearance
        };
        let background_changed = self.appearance.background.kind != appearance.background.kind;
        if self.cell_metrics != config.cell_metrics {
            self.cell_metrics = config.cell_metrics;
            self.text_buffers.clear();
            self.text_buffers
                .push(new_text_buffer(&mut self.font_system, self.cell_metrics));
            self.prepared_text.clear();
        }
        self.font_family = config.font_family;
        self.appearance = appearance;
        if background_changed {
            self.background_image = load_background_image(
                &self.device,
                &self.queue,
                &self.image_pipeline,
                &self.appearance,
            );
        }
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        let width = width.max(1);
        let height = height.max(1);
        if self.surface_config.width == width && self.surface_config.height == height {
            return;
        }

        self.surface_config.width = width;
        self.surface_config.height = height;
        self.surface.configure(&self.device, &self.surface_config);
        for buffer in &mut self.text_buffers {
            buffer.set_size(
                &mut self.font_system,
                Some(self.cell_metrics.width as f32 * 2.0),
                Some(self.cell_metrics.height as f32),
            );
        }
    }

    pub fn recreate_surface<'window>(
        &mut self,
        target: impl Into<SurfaceTarget<'window>>,
    ) -> Result<(), RenderError>
    where
        'window: 'static,
    {
        self.surface = self._instance.create_surface(target)?;
        self.surface.configure(&self.device, &self.surface_config);
        Ok(())
    }

    pub fn render(
        &mut self,
        snapshot: &GridSnapshot,
        damage: &Damage,
        options: RenderOptions,
    ) -> Result<(), RenderError> {
        let plan = build_render_plan_with_appearance(
            snapshot,
            damage,
            self.cell_metrics,
            &self.appearance,
            &options,
        );
        self.update_inline_image_cache(&options.inline_images);
        self.update_text(&plan);

        let mut background_rects = background_pass_rects(
            &self.appearance,
            self.surface_config.width,
            self.surface_config.height,
        );
        background_rects.extend(
            plan.rects
                .iter()
                .copied()
                .filter(|rect| rect.layer == RectLayer::Background)
                .collect::<Vec<_>>(),
        );
        let mut overlay_rects = plan
            .rects
            .iter()
            .copied()
            .filter(|rect| rect.layer == RectLayer::Overlay)
            .collect::<Vec<_>>();
        if !options.focused {
            overlay_rects.push(unfocused_overlay_rect(
                &self.appearance,
                self.surface_config.width,
                self.surface_config.height,
            ));
        }
        overlay_rects.extend(effect_overlay_rects(
            &self.appearance,
            self.surface_config.width,
            self.surface_config.height,
        ));
        self.rect_pipeline.prepare(
            &self.device,
            &self.queue,
            RectPrepareParams {
                surface_width: self.surface_config.width,
                surface_height: self.surface_config.height,
                background_rects: &background_rects,
                selection_rects: &plan.selection_rects,
                overlay_rects: &overlay_rects,
            },
        );
        let active_background_image =
            active_background_image(&self.appearance, self.background_image.as_ref());
        self.image_pipeline.prepare_background(
            &self.device,
            &self.queue,
            self.surface_config.width,
            self.surface_config.height,
            active_background_image,
        );
        self.image_pipeline.prepare_inline(
            &self.device,
            &self.queue,
            self.surface_config.width,
            self.surface_config.height,
            &plan.images,
        );

        self.viewport.update(
            &self.queue,
            Resolution {
                width: self.surface_config.width,
                height: self.surface_config.height,
            },
        );

        let text_areas = self.prepared_text.iter().map(|area| TextArea {
            buffer: &self.text_buffers[area.buffer_index],
            left: area.left,
            top: area.top,
            scale: area.scale,
            bounds: area.bounds,
            default_color: DEFAULT_FG.to_glyphon(),
            custom_glyphs: &[],
        });

        self.text_renderer.prepare(
            &self.device,
            &self.queue,
            &mut self.font_system,
            &mut self.atlas,
            &self.viewport,
            text_areas,
            &mut self.swash_cache,
        )?;

        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame) => frame,
            wgpu::CurrentSurfaceTexture::Timeout => {
                eprintln!("knightty renderer: surface frame acquisition timed out");
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Occluded => {
                eprintln!("knightty renderer: surface is occluded");
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Suboptimal(_) => {
                eprintln!("knightty renderer: surface outdated; reconfiguring");
                self.surface.configure(&self.device, &self.surface_config);
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                eprintln!("knightty renderer: surface lost; recreation required");
                return Err(RenderError::SurfaceLost);
            }
            wgpu::CurrentSurfaceTexture::Validation => return Err(RenderError::SurfaceValidation),
        };

        let view = frame.texture.create_view(&TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&CommandEncoderDescriptor { label: None });
        {
            let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: Operations {
                        load: LoadOp::Clear(clear_color(
                            &self.appearance,
                            self.rect_pipeline.target_is_srgb,
                        )),
                        store: StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });

            self.image_pipeline.draw(
                &mut pass,
                active_background_image.map(|(_, texture)| texture),
            );
            self.image_pipeline.draw_inline(
                &mut pass,
                &self.inline_images,
                InlineImageLayer::BelowCellBackground,
            );
            self.rect_pipeline
                .draw(&mut pass, PreparedRectLayer::Background);
            self.image_pipeline.draw_inline(
                &mut pass,
                &self.inline_images,
                InlineImageLayer::BelowText,
            );
            self.image_pipeline
                .draw_inline(&mut pass, &self.inline_images, InlineImageLayer::Zero);
            self.rect_pipeline
                .draw(&mut pass, PreparedRectLayer::Selection);
            self.text_renderer
                .render(&self.atlas, &self.viewport, &mut pass)?;
            self.image_pipeline.draw_inline(
                &mut pass,
                &self.inline_images,
                InlineImageLayer::AboveText,
            );
            self.rect_pipeline
                .draw(&mut pass, PreparedRectLayer::Overlay);
        }

        self.queue.submit(Some(encoder.finish()));
        frame.present();
        self.atlas.trim();

        Ok(())
    }

    fn update_inline_image_cache(&mut self, images: &[InlineImageData]) {
        let (remove, upload) =
            inline_image_cache_changes(self.inline_images.keys().copied(), images);
        for id in remove {
            self.inline_images.remove(&id);
        }

        for image in images.iter().filter(|image| upload.contains(&image.id)) {
            match InlineImageTexture::upload(&self.device, &self.queue, &self.image_pipeline, image)
            {
                Ok(texture) => {
                    self.inline_images.insert(image.id, texture);
                }
                Err(error) => {
                    eprintln!(
                        "knightty renderer: failed to upload inline image {:?}: {error}",
                        image.id
                    );
                }
            }
        }
    }

    fn update_text(&mut self, plan: &RenderPlan) {
        while self.text_buffers.len() < plan.text.len() {
            let buffer = new_text_buffer(&mut self.font_system, self.cell_metrics);
            self.text_buffers.push(buffer);
        }

        self.prepared_text.clear();
        for (index, segment) in plan.text.iter().enumerate() {
            let attrs = attrs_for_style(segment.style, self.font_family.as_deref());
            let buffer = &mut self.text_buffers[index];
            buffer.set_monospace_width(
                &mut self.font_system,
                (!segment.cell_span).then_some(self.cell_metrics.width as f32),
            );
            buffer.set_size(
                &mut self.font_system,
                Some(segment.width as f32),
                Some(segment.height as f32),
            );
            buffer.set_text(
                &mut self.font_system,
                &segment.text,
                &attrs,
                Shaping::Advanced,
                None,
            );
            let (left, top, scale, bounds) = if segment.cell_span {
                let glyph_advance = buffer.layout_runs().next().map_or(0.0, |run| run.line_w);
                if let Some(scale) = cell_span_scale(
                    glyph_advance,
                    segment.width as f32,
                    segment.height as f32,
                    self.cell_metrics.line_height,
                ) {
                    let scaled_width = glyph_advance * scale;
                    let scaled_height = self.cell_metrics.line_height * scale;
                    (
                        segment.x as f32 + (segment.width as f32 - scaled_width) * 0.5,
                        segment.y as f32 + (segment.height as f32 - scaled_height) * 0.5,
                        scale,
                        TextBounds {
                            left: segment.x.min(i32::MAX as u32) as i32,
                            top: segment.y.min(i32::MAX as u32) as i32,
                            right: segment.x.saturating_add(segment.width).min(i32::MAX as u32)
                                as i32,
                            bottom: segment
                                .y
                                .saturating_add(segment.height)
                                .min(i32::MAX as u32) as i32,
                        },
                    )
                } else {
                    (
                        segment.x as f32,
                        segment.y as f32,
                        1.0,
                        TextBounds {
                            left: segment.x.min(i32::MAX as u32) as i32,
                            top: segment.y.min(i32::MAX as u32) as i32,
                            right: segment
                                .x
                                .saturating_add(self.cell_metrics.width)
                                .min(i32::MAX as u32) as i32,
                            bottom: segment
                                .y
                                .saturating_add(self.cell_metrics.height)
                                .min(i32::MAX as u32) as i32,
                        },
                    )
                }
            } else {
                (
                    segment.x as f32,
                    segment.y as f32,
                    1.0,
                    TextBounds {
                        left: 0,
                        top: 0,
                        right: self.surface_config.width as i32,
                        bottom: self.surface_config.height as i32,
                    },
                )
            };
            self.prepared_text.push(PreparedTextArea {
                buffer_index: index,
                left,
                top,
                scale,
                bounds,
            });
        }
    }
}

fn cell_span_scale(
    glyph_advance: f32,
    span_width: f32,
    span_height: f32,
    line_height: f32,
) -> Option<f32> {
    if !glyph_advance.is_finite()
        || !line_height.is_finite()
        || glyph_advance <= 0.0
        || line_height <= 0.0
    {
        return None;
    }
    let scale = (span_width / glyph_advance).min(span_height / line_height);
    scale.is_finite().then_some(scale.max(0.0))
}

pub fn available_font_families() -> Vec<FontFamilyInfo> {
    let font_system = FontSystem::new();
    font_families_from_system(&font_system)
}

fn font_families_from_system(font_system: &FontSystem) -> Vec<FontFamilyInfo> {
    unique_font_family_infos(font_system.db().faces().flat_map(|face| {
        face.families
            .iter()
            .map(move |(family, _language)| (family.as_str(), face.monospaced))
    }))
}

fn unique_font_family_infos<'a>(
    families: impl IntoIterator<Item = (&'a str, bool)>,
) -> Vec<FontFamilyInfo> {
    let mut by_name = BTreeMap::new();
    for (family, monospaced) in families {
        let family = family.trim();
        if family.is_empty() {
            continue;
        }

        by_name
            .entry(family.to_owned())
            .and_modify(|existing| *existing |= monospaced)
            .or_insert(monospaced);
    }

    by_name
        .into_iter()
        .map(|(name, monospaced)| FontFamilyInfo { name, monospaced })
        .collect()
}

fn inline_image_cache_changes(
    existing: impl IntoIterator<Item = ImageId>,
    requested: &[InlineImageData],
) -> (Vec<ImageId>, BTreeSet<ImageId>) {
    let existing = existing.into_iter().collect::<BTreeSet<_>>();
    let requested = requested
        .iter()
        .map(|image| image.id)
        .collect::<BTreeSet<_>>();
    let remove = existing.difference(&requested).copied().collect();
    let upload = requested.difference(&existing).copied().collect();
    (remove, upload)
}

fn new_text_buffer(font_system: &mut FontSystem, cell_metrics: CellMetrics) -> Buffer {
    let mut buffer = Buffer::new(
        font_system,
        Metrics::new(cell_metrics.font_size, cell_metrics.line_height),
    );
    buffer.set_wrap(font_system, Wrap::None);
    buffer.set_monospace_width(font_system, Some(cell_metrics.width as f32));
    buffer.set_size(
        font_system,
        Some(cell_metrics.width as f32 * 2.0),
        Some(cell_metrics.height as f32),
    );
    buffer
}

struct PreparedTextArea {
    buffer_index: usize,
    left: f32,
    top: f32,
    scale: f32,
    bounds: TextBounds,
}

pub fn build_render_plan(
    snapshot: &GridSnapshot,
    _damage: &Damage,
    metrics: CellMetrics,
) -> RenderPlan {
    build_render_plan_with_options(snapshot, _damage, metrics, RenderOptions::default())
}

pub fn build_render_plan_with_options(
    snapshot: &GridSnapshot,
    _damage: &Damage,
    metrics: CellMetrics,
    options: RenderOptions,
) -> RenderPlan {
    build_render_plan_with_appearance(
        snapshot,
        _damage,
        metrics,
        &RendererAppearance::default(),
        &options,
    )
}

pub fn build_render_plan_with_appearance(
    snapshot: &GridSnapshot,
    _damage: &Damage,
    metrics: CellMetrics,
    appearance: &RendererAppearance,
    options: &RenderOptions,
) -> RenderPlan {
    let mut text = Vec::new();
    let mut rects = Vec::new();
    let mut selection_rects = Vec::new();
    let mut images = Vec::new();
    let mut hyperlink_spans = Vec::new();
    let layout = RenderLayout::new(metrics, appearance.window);
    let context = RenderPlanContext {
        layout,
        appearance,
        options,
        cursor: CursorRenderState {
            cursor: snapshot.cursor,
            visible: cursor_visible(snapshot, appearance, options),
        },
    };

    for (row_index, row) in snapshot.lines().enumerate() {
        push_background_rects(row, row_index, context, &mut rects);
        push_underline_rects(row, row_index, layout, appearance, &mut rects);
        push_text_segments(
            row,
            row_index,
            context,
            &snapshot.selection_rects,
            &snapshot.cell_spans,
            &mut text,
        );
        push_hyperlink_spans(row, row_index, &mut hyperlink_spans);
    }
    push_cell_span_text(snapshot, context, &mut text);
    push_cell_span_underline_rects(snapshot, layout, appearance, &mut rects);
    push_inline_image_quads(snapshot, layout, &mut images);
    push_search_rects(snapshot, layout, appearance, options, &mut rects);
    push_selection_rects(snapshot, layout, appearance, &mut selection_rects);
    if let Some(hyperlink_id) = options.hovered_hyperlink_id
        && appearance.hyperlink.hover_underline
    {
        push_hover_hyperlink_underline_rects(
            snapshot,
            hyperlink_id,
            layout,
            appearance,
            &mut rects,
        );
    }

    if context.cursor.visible {
        push_cursor_rects(snapshot, layout, appearance, &mut rects);
    }
    let tab_bar_visible = tab_bar_visible(appearance);
    if tab_bar_visible {
        push_tab_bar(layout, appearance, &mut rects, &mut text);
    }
    if let Some(query) = options.search_query.as_deref() {
        push_search_bar(
            query,
            layout,
            appearance,
            tab_bar_visible,
            &mut rects,
            &mut text,
        );
    }
    push_ime_preedit(snapshot, layout, appearance, options, &mut rects, &mut text);

    RenderPlan {
        text,
        rects,
        selection_rects,
        images,
        hyperlink_spans,
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct RenderLayout {
    metrics: CellMetrics,
    origin_x: u32,
    origin_y: u32,
}

impl RenderLayout {
    fn new(metrics: CellMetrics, window: WindowAppearance) -> Self {
        Self {
            metrics,
            origin_x: window.padding_x,
            origin_y: window.padding_y,
        }
    }

    fn cell_x(self, col: usize) -> u32 {
        self.origin_x
            .saturating_add(col as u32 * self.metrics.width)
    }

    fn cell_y(self, row: usize) -> u32 {
        self.origin_y
            .saturating_add(row as u32 * self.metrics.height)
    }
}

fn push_inline_image_quads(
    snapshot: &GridSnapshot,
    layout: RenderLayout,
    images: &mut Vec<ImageQuadPlan>,
) {
    for (insertion_order, placement) in snapshot.image_placements.iter().enumerate() {
        if let Some(quad) = inline_image_quad(placement, snapshot, layout, insertion_order as u64) {
            images.push(quad);
        }
    }
    images.sort_by_key(|image| (image.z_index, image.client_image_id, image.insertion_order));
}

fn inline_image_quad(
    placement: &ImagePlacement,
    snapshot: &GridSnapshot,
    layout: RenderLayout,
    insertion_order: u64,
) -> Option<ImageQuadPlan> {
    let left = f64::from(layout.origin_x)
        + placement.anchor.col as f64 * f64::from(layout.metrics.width)
        + f64::from(placement.pixel_offset.x);
    let top = f64::from(layout.origin_y)
        + placement.anchor.row as f64 * f64::from(layout.metrics.height)
        + f64::from(placement.pixel_offset.y);
    let width = f64::from(placement.columns) * f64::from(layout.metrics.width);
    let height = f64::from(placement.rows) * f64::from(layout.metrics.height);
    if width <= 0.0 || height <= 0.0 {
        return None;
    }

    let grid_left = f64::from(layout.origin_x);
    let grid_top = f64::from(layout.origin_y);
    let grid_right = grid_left + snapshot.cols as f64 * f64::from(layout.metrics.width);
    let grid_bottom = grid_top + snapshot.rows as f64 * f64::from(layout.metrics.height);
    let clipped_left = left.max(grid_left);
    let clipped_top = top.max(grid_top);
    let clipped_right = (left + width).min(grid_right);
    let clipped_bottom = (top + height).min(grid_bottom);
    if clipped_right <= clipped_left || clipped_bottom <= clipped_top {
        return None;
    }

    let source_right = placement
        .source_rect
        .x
        .checked_add(placement.source_rect.width)?;
    let source_bottom = placement
        .source_rect
        .y
        .checked_add(placement.source_rect.height)?;
    let source_width = f64::from(placement.source_width);
    let source_height = f64::from(placement.source_height);
    if source_width <= 0.0 || source_height <= 0.0 {
        return None;
    }
    let base_u0 = f64::from(placement.source_rect.x) / source_width;
    let base_v0 = f64::from(placement.source_rect.y) / source_height;
    let base_u1 = f64::from(source_right) / source_width;
    let base_v1 = f64::from(source_bottom) / source_height;
    let left_clip = (clipped_left - left) / width;
    let top_clip = (clipped_top - top) / height;
    let right_clip = (clipped_right - left) / width;
    let bottom_clip = (clipped_bottom - top) / height;

    Some(ImageQuadPlan {
        image_id: placement.image_id,
        destination: PixelRect {
            x: clipped_left as f32,
            y: clipped_top as f32,
            width: (clipped_right - clipped_left) as f32,
            height: (clipped_bottom - clipped_top) as f32,
        },
        uv: UvRect {
            u0: (base_u0 + (base_u1 - base_u0) * left_clip) as f32,
            v0: (base_v0 + (base_v1 - base_v0) * top_clip) as f32,
            u1: (base_u0 + (base_u1 - base_u0) * right_clip) as f32,
            v1: (base_v0 + (base_v1 - base_v0) * bottom_clip) as f32,
        },
        z_index: placement.z_index.0,
        client_image_id: placement.kitty_image_id.unwrap_or(0),
        insertion_order,
    })
}

#[derive(Clone, Copy)]
struct RenderPlanContext<'a> {
    layout: RenderLayout,
    appearance: &'a RendererAppearance,
    options: &'a RenderOptions,
    cursor: CursorRenderState,
}

#[derive(Clone, Copy)]
struct CursorRenderState {
    cursor: Cursor,
    visible: bool,
}

impl CursorRenderState {
    fn block_at(self, column: usize, row: usize, style: CursorStyle) -> bool {
        self.visible
            && style == CursorStyle::Block
            && self.cursor.x == column
            && self.cursor.y == row
    }
}

fn cursor_visible(
    snapshot: &GridSnapshot,
    appearance: &RendererAppearance,
    options: &RenderOptions,
) -> bool {
    snapshot.cursor.visible && (!appearance.cursor.blink || options.cursor_blink_visible)
}

fn push_hover_hyperlink_underline_rects(
    snapshot: &GridSnapshot,
    hyperlink_id: usize,
    layout: RenderLayout,
    appearance: &RendererAppearance,
    rects: &mut Vec<RectPlan>,
) {
    for (row_index, row) in snapshot.lines().enumerate() {
        push_hyperlink_underline_rects(row, row_index, hyperlink_id, layout, appearance, rects);
    }
}

fn push_hyperlink_underline_rects(
    row: &[Cell],
    row_index: usize,
    hyperlink_id: usize,
    layout: RenderLayout,
    appearance: &RendererAppearance,
    rects: &mut Vec<RectPlan>,
) {
    let mut start = None::<(usize, Rgba)>;

    for (column, cell) in row.iter().enumerate() {
        if cell.flags.wide_spacer || cell.hyperlink != Some(hyperlink_id) {
            flush_underline_rect(start.take(), column, row_index, layout, rects);
            continue;
        }

        let fg = appearance
            .hyperlink
            .hover_foreground
            .unwrap_or_else(|| effective_fg(cell, &appearance.theme));
        match start {
            Some((_, color)) if color == fg => {}
            Some(_) => {
                flush_underline_rect(start.take(), column, row_index, layout, rects);
                start = Some((column, fg));
            }
            None => start = Some((column, fg)),
        }
    }

    flush_underline_rect(start, row.len(), row_index, layout, rects);
}

fn push_selection_rects(
    snapshot: &GridSnapshot,
    layout: RenderLayout,
    appearance: &RendererAppearance,
    rects: &mut Vec<RectPlan>,
) {
    for rect in &snapshot.selection_rects {
        rects.push(RectPlan {
            x: layout.cell_x(rect.col),
            y: layout.cell_y(rect.row),
            width: rect.width as u32 * layout.metrics.width,
            height: rect.height as u32 * layout.metrics.height,
            color: appearance.theme.selection_background,
            layer: RectLayer::Background,
        });
    }
}

fn push_text_segments(
    row: &[Cell],
    row_index: usize,
    context: RenderPlanContext<'_>,
    selection_rects: &[knightty_core::SelectionRect],
    cell_spans: &[CellSpanPlacement],
    text: &mut Vec<TextSegmentPlan>,
) {
    for (column, cell) in row.iter().enumerate() {
        if cell.flags.wide_spacer
            || cell.ch == ' '
            || cell_spans
                .iter()
                .any(|span| span.anchor.row == row_index as isize && span.anchor.col == column)
        {
            continue;
        }

        let state = visual_state_for_cell(cell, column, row_index, selection_rects, context);
        text.push(TextSegmentPlan {
            x: context.layout.cell_x(column),
            y: context.layout.cell_y(row_index),
            width: if cell.flags.wide {
                context.layout.metrics.width * 2
            } else {
                context.layout.metrics.width
            },
            height: context.layout.metrics.height,
            text: cell.ch.to_string(),
            style: style_for_cell(cell, context.appearance, state),
            cell_span: false,
        });
    }
}

fn push_cell_span_text(
    snapshot: &GridSnapshot,
    context: RenderPlanContext<'_>,
    text: &mut Vec<TextSegmentPlan>,
) {
    for span in &snapshot.cell_spans {
        let Ok(row) = usize::try_from(span.anchor.row) else {
            continue;
        };
        if row >= snapshot.rows || span.anchor.col >= snapshot.cols {
            continue;
        }
        text.push(TextSegmentPlan {
            x: context.layout.cell_x(span.anchor.col),
            y: context.layout.cell_y(row),
            width: u32::from(span.columns).saturating_mul(context.layout.metrics.width),
            height: u32::from(span.rows).saturating_mul(context.layout.metrics.height),
            text: span.text.clone(),
            style: style_for_cell(
                &span.cell,
                context.appearance,
                visual_state_for_cell(
                    &span.cell,
                    span.anchor.col,
                    row,
                    &snapshot.selection_rects,
                    context,
                ),
            ),
            cell_span: true,
        });
    }
}

fn push_cell_span_underline_rects(
    snapshot: &GridSnapshot,
    layout: RenderLayout,
    appearance: &RendererAppearance,
    rects: &mut Vec<RectPlan>,
) {
    for span in &snapshot.cell_spans {
        if !span.cell.flags.underline {
            continue;
        }
        let Ok(row) = usize::try_from(span.anchor.row) else {
            continue;
        };
        if row >= snapshot.rows || span.anchor.col >= snapshot.cols {
            continue;
        }
        let bottom_row = row
            .saturating_add(usize::from(span.rows))
            .min(snapshot.rows)
            .saturating_sub(1);
        rects.push(RectPlan {
            x: layout.cell_x(span.anchor.col),
            y: layout
                .cell_y(bottom_row)
                .saturating_add(layout.metrics.height.saturating_sub(2)),
            width: u32::from(span.columns).saturating_mul(layout.metrics.width),
            height: 1,
            color: effective_fg(&span.cell, &appearance.theme),
            layer: RectLayer::Overlay,
        });
    }
}

fn push_hyperlink_spans(row: &[Cell], row_index: usize, spans: &mut Vec<HyperlinkSpan>) {
    let mut start = None::<(usize, usize)>;

    for (column, cell) in row.iter().enumerate() {
        match (start, cell.hyperlink) {
            (Some((_, current_id)), Some(hyperlink_id)) if current_id == hyperlink_id => {}
            (Some((start_col, hyperlink_id)), Some(next_id)) => {
                spans.push(HyperlinkSpan {
                    hyperlink_id,
                    row: row_index,
                    start_col,
                    end_col: column,
                });
                start = Some((column, next_id));
            }
            (None, Some(hyperlink_id)) => {
                start = Some((column, hyperlink_id));
            }
            (Some((start_col, hyperlink_id)), None) => {
                spans.push(HyperlinkSpan {
                    hyperlink_id,
                    row: row_index,
                    start_col,
                    end_col: column,
                });
                start = None;
            }
            (None, None) => {}
        }
    }

    if let Some((start_col, hyperlink_id)) = start {
        spans.push(HyperlinkSpan {
            hyperlink_id,
            row: row_index,
            start_col,
            end_col: row.len(),
        });
    }
}

fn push_background_rects(
    row: &[Cell],
    row_index: usize,
    context: RenderPlanContext<'_>,
    rects: &mut Vec<RectPlan>,
) {
    let mut start = None::<(usize, Rgba)>;

    for (column, cell) in row.iter().enumerate() {
        let bg = cell_background_for_plan(cell, column, row_index, context);
        let Some(bg) = bg else {
            flush_rect(
                start.take(),
                column,
                row_index,
                context.layout,
                RectLayer::Background,
                rects,
            );
            continue;
        };

        match start {
            Some((_, color)) if color == bg => {}
            Some(_) => {
                flush_rect(
                    start.take(),
                    column,
                    row_index,
                    context.layout,
                    RectLayer::Background,
                    rects,
                );
                start = Some((column, bg));
            }
            None => start = Some((column, bg)),
        }
    }

    flush_rect(
        start,
        row.len(),
        row_index,
        context.layout,
        RectLayer::Background,
        rects,
    );
}

fn push_underline_rects(
    row: &[Cell],
    row_index: usize,
    layout: RenderLayout,
    appearance: &RendererAppearance,
    rects: &mut Vec<RectPlan>,
) {
    let mut start = None::<(usize, Rgba)>;

    for (column, cell) in row.iter().enumerate() {
        if cell.flags.wide_spacer || !cell.flags.underline {
            flush_underline_rect(start.take(), column, row_index, layout, rects);
            continue;
        }

        let fg = effective_fg(cell, &appearance.theme);
        match start {
            Some((_, color)) if color == fg => {}
            Some(_) => {
                flush_underline_rect(start.take(), column, row_index, layout, rects);
                start = Some((column, fg));
            }
            None => start = Some((column, fg)),
        }
    }

    flush_underline_rect(start, row.len(), row_index, layout, rects);
}

fn flush_rect(
    start: Option<(usize, Rgba)>,
    end_column: usize,
    row_index: usize,
    layout: RenderLayout,
    layer: RectLayer,
    rects: &mut Vec<RectPlan>,
) {
    let Some((start_column, color)) = start else {
        return;
    };
    if end_column <= start_column {
        return;
    }

    rects.push(RectPlan {
        x: layout.cell_x(start_column),
        y: layout.cell_y(row_index),
        width: (end_column - start_column) as u32 * layout.metrics.width,
        height: layout.metrics.height,
        color,
        layer,
    });
}

fn flush_underline_rect(
    start: Option<(usize, Rgba)>,
    end_column: usize,
    row_index: usize,
    layout: RenderLayout,
    rects: &mut Vec<RectPlan>,
) {
    let Some((start_column, color)) = start else {
        return;
    };
    if end_column <= start_column {
        return;
    }

    rects.push(RectPlan {
        x: layout.cell_x(start_column),
        y: layout
            .cell_y(row_index)
            .saturating_add(layout.metrics.height.saturating_sub(2)),
        width: (end_column - start_column) as u32 * layout.metrics.width,
        height: 1,
        color,
        layer: RectLayer::Overlay,
    });
}

fn visual_state_for_cell(
    cell: &Cell,
    column: usize,
    row_index: usize,
    selection_rects: &[knightty_core::SelectionRect],
    context: RenderPlanContext<'_>,
) -> CellVisualState {
    CellVisualState {
        selected: selection_rects.iter().any(|rect| {
            row_index >= rect.row
                && row_index < rect.row + rect.height
                && column >= rect.col
                && column < rect.col + rect.width
        }),
        search_match: context
            .options
            .search_matches
            .iter()
            .any(|range| range.contains(column, row_index)),
        current_search_match: context
            .options
            .current_search_match
            .as_ref()
            .is_some_and(|range| range.contains(column, row_index)),
        hyperlink_hover: context
            .options
            .hovered_hyperlink_id
            .is_some_and(|hyperlink_id| cell.hyperlink == Some(hyperlink_id)),
        block_cursor: context
            .cursor
            .block_at(column, row_index, context.appearance.cursor.style),
    }
}

fn cell_background_for_plan(
    cell: &Cell,
    column: usize,
    row_index: usize,
    context: RenderPlanContext<'_>,
) -> Option<Rgba> {
    let state = CellVisualState {
        selected: false,
        search_match: context
            .options
            .search_matches
            .iter()
            .any(|range| range.contains(column, row_index)),
        current_search_match: context
            .options
            .current_search_match
            .as_ref()
            .is_some_and(|range| range.contains(column, row_index)),
        hyperlink_hover: context
            .options
            .hovered_hyperlink_id
            .is_some_and(|hyperlink_id| cell.hyperlink == Some(hyperlink_id)),
        block_cursor: context
            .cursor
            .block_at(column, row_index, context.appearance.cursor.style),
    };
    cell_visual(cell, context.appearance, state).bg
}

fn default_background_rect(cell: &Cell, theme: &ResolvedTheme) -> Option<Rgba> {
    if !cell.flags.inverse && matches!(cell.bg, Color::DefaultBg) {
        return None;
    }

    let bg = effective_bg(cell, theme);
    if bg == theme.background {
        None
    } else {
        Some(bg)
    }
}

fn push_search_rects(
    snapshot: &GridSnapshot,
    layout: RenderLayout,
    appearance: &RendererAppearance,
    options: &RenderOptions,
    rects: &mut Vec<RectPlan>,
) {
    for search_match in &options.search_matches {
        if Some(search_match) == options.current_search_match.as_ref() {
            continue;
        }
        push_search_rect(
            snapshot,
            search_match,
            layout,
            appearance.search.background,
            rects,
        );
    }
    if let Some(search_match) = &options.current_search_match {
        push_search_rect(
            snapshot,
            search_match,
            layout,
            appearance.search.selected_background,
            rects,
        );
    }
}

fn push_search_rect(
    snapshot: &GridSnapshot,
    search_match: &SearchMatch,
    layout: RenderLayout,
    color: Rgba,
    rects: &mut Vec<RectPlan>,
) {
    if search_match.end_col <= search_match.start_col {
        return;
    }
    rects.push(RectPlan {
        x: layout.cell_x(search_match.start_col),
        y: layout.cell_y(search_match.row),
        width: (search_match.end_col - search_match.start_col) as u32 * layout.metrics.width,
        height: layout.metrics.height,
        color,
        layer: RectLayer::Background,
    });

    for span in &snapshot.cell_spans {
        let Ok(row) = usize::try_from(span.anchor.row) else {
            continue;
        };
        if !search_match.contains(span.anchor.col, row) {
            continue;
        }
        rects.push(RectPlan {
            x: layout.cell_x(span.anchor.col),
            y: layout.cell_y(row),
            width: u32::from(span.columns).saturating_mul(layout.metrics.width),
            height: u32::from(span.rows).saturating_mul(layout.metrics.height),
            color,
            layer: RectLayer::Background,
        });
    }
}

fn push_search_bar(
    query: &str,
    layout: RenderLayout,
    appearance: &RendererAppearance,
    below_tab_bar: bool,
    rects: &mut Vec<RectPlan>,
    text: &mut Vec<TextSegmentPlan>,
) {
    let label = format!("/{query}");
    let label_width = label.chars().count().max(1) as u32 * layout.metrics.width;
    let padding = layout.metrics.width;
    let width = label_width.saturating_add(padding.saturating_mul(2));
    let y = layout.origin_y
        + if below_tab_bar {
            layout.metrics.height
        } else {
            0
        };
    rects.push(RectPlan {
        x: layout.origin_x,
        y,
        width,
        height: layout.metrics.height,
        color: appearance.search.selected_background.with_alpha(0.92),
        layer: RectLayer::Background,
    });
    text.push(TextSegmentPlan {
        x: layout.origin_x.saturating_add(padding),
        y,
        width: label_width,
        height: layout.metrics.height,
        text: label,
        style: TextStyle {
            fg: appearance.search.selected_foreground,
            bold: false,
            italic: false,
            underline: false,
            inverse: false,
        },
        cell_span: false,
    });
}

fn tab_bar_visible(appearance: &RendererAppearance) -> bool {
    appearance.tabs.enabled && appearance.tabs.show_when_single
}

fn push_tab_bar(
    layout: RenderLayout,
    appearance: &RendererAppearance,
    rects: &mut Vec<RectPlan>,
    text: &mut Vec<TextSegmentPlan>,
) {
    let label = tab_label(appearance.tabs.style);
    let label_width = label.chars().count().max(1) as u32 * layout.metrics.width;
    let padding = layout.metrics.width;
    let width = label_width.saturating_add(padding.saturating_mul(2));
    rects.push(RectPlan {
        x: layout.origin_x,
        y: layout.origin_y,
        width,
        height: layout.metrics.height,
        color: appearance.tabs.active_background,
        layer: RectLayer::Background,
    });
    rects.push(RectPlan {
        x: layout.origin_x,
        y: layout
            .origin_y
            .saturating_add(layout.metrics.height.saturating_sub(1)),
        width,
        height: 1,
        color: appearance.tabs.inactive_background,
        layer: RectLayer::Overlay,
    });
    text.push(TextSegmentPlan {
        x: layout.origin_x.saturating_add(padding),
        y: layout.origin_y,
        width: label_width,
        height: layout.metrics.height,
        text: label.to_owned(),
        style: TextStyle {
            fg: appearance.tabs.active_foreground,
            bold: false,
            italic: false,
            underline: false,
            inverse: false,
        },
        cell_span: false,
    });
}

fn tab_label(style: TabStyle) -> &'static str {
    match style {
        TabStyle::Minimal => "knightty",
        TabStyle::Separator => "knightty |",
        TabStyle::Powerline => "knightty >",
        TabStyle::Slant => "/ knightty /",
    }
}

fn push_cursor_rects(
    snapshot: &GridSnapshot,
    layout: RenderLayout,
    appearance: &RendererAppearance,
    rects: &mut Vec<RectPlan>,
) {
    let x = layout.cell_x(snapshot.cursor.x);
    let y = layout.cell_y(snapshot.cursor.y);
    let width = layout.metrics.width;
    let height = layout.metrics.height;
    let color = appearance.theme.cursor;

    match appearance.cursor.style {
        CursorStyle::Block => {}
        CursorStyle::Bar => rects.push(RectPlan {
            x,
            y,
            width: 2,
            height,
            color,
            layer: RectLayer::Overlay,
        }),
        CursorStyle::Underline => rects.push(RectPlan {
            x,
            y: y.saturating_add(height.saturating_sub(2)),
            width,
            height: 2,
            color,
            layer: RectLayer::Overlay,
        }),
        CursorStyle::HollowBlock => {
            rects.extend([
                RectPlan {
                    x,
                    y,
                    width,
                    height: 1,
                    color,
                    layer: RectLayer::Overlay,
                },
                RectPlan {
                    x,
                    y: y.saturating_add(height.saturating_sub(1)),
                    width,
                    height: 1,
                    color,
                    layer: RectLayer::Overlay,
                },
                RectPlan {
                    x,
                    y,
                    width: 1,
                    height,
                    color,
                    layer: RectLayer::Overlay,
                },
                RectPlan {
                    x: x.saturating_add(width.saturating_sub(1)),
                    y,
                    width: 1,
                    height,
                    color,
                    layer: RectLayer::Overlay,
                },
            ]);
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PreeditGlyph {
    row: usize,
    col: usize,
    cells: usize,
    byte_start: usize,
    byte_end: usize,
    text: String,
}

fn push_ime_preedit(
    snapshot: &GridSnapshot,
    layout: RenderLayout,
    appearance: &RendererAppearance,
    options: &RenderOptions,
    rects: &mut Vec<RectPlan>,
    text: &mut Vec<TextSegmentPlan>,
) {
    let Some(preedit) = options.ime_preedit.as_ref() else {
        return;
    };
    if preedit.text.is_empty() || !snapshot.cursor.visible {
        return;
    }

    let glyphs = positioned_preedit_glyphs(
        &preedit.text,
        snapshot.cursor.x,
        snapshot.cursor.y,
        snapshot.cols,
        snapshot.rows,
    );
    if glyphs.is_empty() {
        return;
    }

    let cursor_range = preedit
        .cursor_range
        .as_ref()
        .and_then(|range| normalize_preedit_cursor_range(&preedit.text, range));
    push_preedit_highlight_rects(&glyphs, cursor_range.as_ref(), layout, appearance, rects);
    push_preedit_underline_rects(&glyphs, layout, appearance, rects);

    for glyph in glyphs {
        let highlighted = cursor_range
            .as_ref()
            .is_some_and(|range| byte_ranges_intersect(glyph.byte_start..glyph.byte_end, range));
        text.push(TextSegmentPlan {
            x: layout.cell_x(glyph.col),
            y: layout.cell_y(glyph.row),
            width: glyph.cells as u32 * layout.metrics.width,
            height: layout.metrics.height,
            text: glyph.text,
            style: TextStyle {
                fg: if highlighted {
                    appearance.theme.selection_foreground
                } else {
                    appearance.theme.foreground
                },
                bold: false,
                italic: false,
                underline: false,
                inverse: false,
            },
            cell_span: false,
        });
    }
}

fn positioned_preedit_glyphs(
    text: &str,
    start_col: usize,
    start_row: usize,
    cols: usize,
    rows: usize,
) -> Vec<PreeditGlyph> {
    if cols == 0 || rows == 0 || start_row >= rows {
        return Vec::new();
    }

    let mut glyphs: Vec<PreeditGlyph> = Vec::new();
    let mut col = start_col.min(cols.saturating_sub(1));
    let mut row = start_row;
    for (byte_start, ch) in text.char_indices() {
        let byte_end = byte_start + ch.len_utf8();
        let width = ch.width().unwrap_or(0);
        if width == 0 {
            if let Some(glyph) = glyphs.last_mut() {
                glyph.text.push(ch);
                glyph.byte_end = byte_end;
            } else if row < rows {
                glyphs.push(PreeditGlyph {
                    row,
                    col,
                    cells: 1,
                    byte_start,
                    byte_end,
                    text: ch.to_string(),
                });
            }
            continue;
        }

        let cells = width.min(cols).max(1);
        if col + cells > cols {
            row = row.saturating_add(1);
            col = 0;
        }
        if row >= rows {
            break;
        }

        glyphs.push(PreeditGlyph {
            row,
            col,
            cells,
            byte_start,
            byte_end,
            text: ch.to_string(),
        });
        col = col.saturating_add(cells);
    }

    glyphs
}

fn push_preedit_highlight_rects(
    glyphs: &[PreeditGlyph],
    cursor_range: Option<&Range<usize>>,
    layout: RenderLayout,
    appearance: &RendererAppearance,
    rects: &mut Vec<RectPlan>,
) {
    let Some(cursor_range) = cursor_range else {
        return;
    };
    for glyph in glyphs {
        if !byte_ranges_intersect(glyph.byte_start..glyph.byte_end, cursor_range) {
            continue;
        }
        rects.push(RectPlan {
            x: layout.cell_x(glyph.col),
            y: layout.cell_y(glyph.row),
            width: glyph.cells as u32 * layout.metrics.width,
            height: layout.metrics.height,
            color: appearance.theme.selection_background,
            layer: RectLayer::Background,
        });
    }
}

fn push_preedit_underline_rects(
    glyphs: &[PreeditGlyph],
    layout: RenderLayout,
    appearance: &RendererAppearance,
    rects: &mut Vec<RectPlan>,
) {
    for glyph in glyphs {
        rects.push(RectPlan {
            x: layout.cell_x(glyph.col),
            y: layout
                .cell_y(glyph.row)
                .saturating_add(layout.metrics.height.saturating_sub(2)),
            width: glyph.cells as u32 * layout.metrics.width,
            height: 1,
            color: appearance.theme.foreground,
            layer: RectLayer::Overlay,
        });
    }
}

fn normalize_preedit_cursor_range(text: &str, range: &Range<usize>) -> Option<Range<usize>> {
    let start = floor_char_boundary(text, range.start.min(text.len()));
    let end = ceil_char_boundary(text, range.end.min(text.len()));
    if end <= start { None } else { Some(start..end) }
}

fn floor_char_boundary(text: &str, mut index: usize) -> usize {
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn ceil_char_boundary(text: &str, mut index: usize) -> usize {
    while index < text.len() && !text.is_char_boundary(index) {
        index += 1;
    }
    index
}

fn byte_ranges_intersect(lhs: Range<usize>, rhs: &Range<usize>) -> bool {
    lhs.start < rhs.end && rhs.start < lhs.end
}

fn style_for_cell(
    cell: &Cell,
    appearance: &RendererAppearance,
    state: CellVisualState,
) -> TextStyle {
    let visual = cell_visual(cell, appearance, state);
    TextStyle {
        fg: visual.fg,
        bold: cell.flags.bold,
        italic: cell.flags.italic,
        underline: visual.underline,
        inverse: visual.inverse,
    }
}

fn attrs_for_style(style: TextStyle, font_family: Option<&str>) -> Attrs<'_> {
    let family = font_family.map(Family::Name).unwrap_or(Family::Monospace);
    let mut attrs = Attrs::new()
        .family(family)
        .color(style.fg.to_glyphon())
        .font_features(terminal_font_features());
    if style.bold {
        attrs = attrs.weight(Weight::BOLD);
    }
    if style.italic {
        attrs = attrs.style(GlyphStyle::Italic);
    }
    attrs
}

fn terminal_font_features() -> FontFeatures {
    let mut features = FontFeatures::new();
    features.disable(FeatureTag::STANDARD_LIGATURES);
    features.disable(FeatureTag::CONTEXTUAL_LIGATURES);
    features.disable(FeatureTag::DISCRETIONARY_LIGATURES);
    features.disable(FeatureTag::CONTEXTUAL_ALTERNATES);
    features
}

pub fn cell_visual(
    cell: &Cell,
    appearance: &RendererAppearance,
    state: CellVisualState,
) -> CellVisual {
    let fg = effective_fg(cell, &appearance.theme);
    let bg = effective_bg(cell, &appearance.theme);
    let mut visual = CellVisual {
        fg,
        bg: default_background_rect(cell, &appearance.theme),
        underline: cell.flags.underline,
        inverse: cell.flags.inverse,
    };

    if state.hyperlink_hover {
        if let Some(fg) = appearance.hyperlink.hover_foreground {
            visual.fg = fg;
        }
        if let Some(bg) = appearance.hyperlink.hover_background {
            visual.bg = Some(bg);
        }
        visual.underline |= appearance.hyperlink.hover_underline;
    }

    if state.search_match {
        visual.fg = appearance.search.foreground;
        visual.bg = Some(appearance.search.background);
    }
    if state.current_search_match {
        visual.fg = appearance.search.selected_foreground;
        visual.bg = Some(appearance.search.selected_background);
    }
    if state.selected {
        visual.fg = appearance.theme.selection_foreground;
        visual.bg = Some(appearance.theme.selection_background);
    }

    if cell.flags.inverse && !state.selected && !state.search_match && !state.current_search_match {
        visual.bg = Some(bg);
    }
    if state.block_cursor {
        visual.fg = appearance.theme.cursor_text;
        visual.bg = Some(appearance.theme.cursor);
        visual.underline = false;
    }

    visual
}

fn effective_fg(cell: &Cell, theme: &ResolvedTheme) -> Rgba {
    let fg = resolve_color(cell.fg, true, theme);
    let bg = resolve_color(cell.bg, false, theme);
    if cell.flags.inverse { bg } else { fg }
}

fn effective_bg(cell: &Cell, theme: &ResolvedTheme) -> Rgba {
    let fg = resolve_color(cell.fg, true, theme);
    let bg = resolve_color(cell.bg, false, theme);
    if cell.flags.inverse { fg } else { bg }
}

fn resolve_color(color: Color, foreground: bool, theme: &ResolvedTheme) -> Rgba {
    match color {
        Color::DefaultFg => theme.foreground,
        Color::DefaultBg => theme.background,
        Color::Rgb(r, g, b) => Rgba::rgb(r, g, b),
        Color::Indexed(index) => indexed_color(index, theme),
    }
    .or_default_for(foreground)
}

trait DefaultColor {
    fn or_default_for(self, _foreground: bool) -> Self;
}

impl DefaultColor for Rgba {
    fn or_default_for(self, _foreground: bool) -> Self {
        self
    }
}

fn indexed_color(index: u8, theme: &ResolvedTheme) -> Rgba {
    match index {
        0..=7 => theme.normal[index as usize],
        8..=15 => theme.bright[(index - 8) as usize],
        16..=231 => {
            let index = index - 16;
            let r = index / 36;
            let g = (index % 36) / 6;
            let b = index % 6;
            Rgba::rgb(
                color_cube_channel(r),
                color_cube_channel(g),
                color_cube_channel(b),
            )
        }
        232..=255 => {
            let value = 8 + (index - 232) * 10;
            Rgba::rgb(value, value, value)
        }
    }
}

fn color_cube_channel(value: u8) -> u8 {
    if value == 0 { 0 } else { 55 + value * 40 }
}

pub fn background_pass_rects(
    appearance: &RendererAppearance,
    surface_width: u32,
    surface_height: u32,
) -> Vec<RectPlan> {
    match &appearance.background.kind {
        BackgroundKind::Solid => Vec::new(),
        BackgroundKind::Gradient(gradient) if gradient.colors.len() >= 2 => {
            gradient_rects(gradient, surface_width, surface_height)
        }
        BackgroundKind::Image(image) if image.load_state == ImageLoadState::Ready => {
            if image.tint_opacity <= 0.0 {
                Vec::new()
            } else {
                vec![RectPlan {
                    x: 0,
                    y: 0,
                    width: surface_width.max(1),
                    height: surface_height.max(1),
                    color: image.tint.with_alpha(image.tint_opacity),
                    layer: RectLayer::Background,
                }]
            }
        }
        BackgroundKind::Image(_) | BackgroundKind::Gradient(_) => Vec::new(),
    }
}

fn gradient_rects(
    gradient: &GradientBackground,
    surface_width: u32,
    surface_height: u32,
) -> Vec<RectPlan> {
    let steps = match gradient.orientation {
        GradientOrientation::Vertical => (surface_height / 4).clamp(2, 128),
        GradientOrientation::Horizontal => (surface_width / 4).clamp(2, 128),
    };
    let mut rects = Vec::with_capacity(steps as usize);
    for step in 0..steps {
        let t = if steps <= 1 {
            0.0
        } else {
            step as f32 / (steps - 1) as f32
        };
        let color = gradient_color_at(&gradient.colors, t);
        match gradient.orientation {
            GradientOrientation::Vertical => {
                let y = step * surface_height.max(1) / steps;
                let next_y = (step + 1) * surface_height.max(1) / steps;
                rects.push(RectPlan {
                    x: 0,
                    y,
                    width: surface_width.max(1),
                    height: next_y.saturating_sub(y).max(1),
                    color,
                    layer: RectLayer::Background,
                });
            }
            GradientOrientation::Horizontal => {
                let x = step * surface_width.max(1) / steps;
                let next_x = (step + 1) * surface_width.max(1) / steps;
                rects.push(RectPlan {
                    x,
                    y: 0,
                    width: next_x.saturating_sub(x).max(1),
                    height: surface_height.max(1),
                    color,
                    layer: RectLayer::Background,
                });
            }
        }
    }
    rects
}

fn gradient_color_at(colors: &[Rgba], t: f32) -> Rgba {
    if colors.is_empty() {
        return DEFAULT_BG;
    }
    if colors.len() == 1 {
        return colors[0];
    }

    let scaled = t.clamp(0.0, 1.0) * (colors.len() - 1) as f32;
    let start = scaled.floor() as usize;
    let end = (start + 1).min(colors.len() - 1);
    let local_t = scaled - start as f32;
    lerp_rgba(colors[start], colors[end], local_t)
}

fn lerp_rgba(start: Rgba, end: Rgba, t: f32) -> Rgba {
    fn lerp_channel(start: u8, end: u8, t: f32) -> u8 {
        (start as f32 + (end as f32 - start as f32) * t).round() as u8
    }

    Rgba::with_channels(
        lerp_channel(start.r, end.r, t),
        lerp_channel(start.g, end.g, t),
        lerp_channel(start.b, end.b, t),
        lerp_channel(start.a, end.a, t),
    )
}

fn clear_color(appearance: &RendererAppearance, target_is_srgb: bool) -> wgpu::Color {
    let rgba = appearance
        .theme
        .background
        .with_alpha(appearance.window.opacity)
        .to_target_f32(target_is_srgb);
    wgpu::Color {
        r: rgba[0] as f64,
        g: rgba[1] as f64,
        b: rgba[2] as f64,
        a: rgba[3] as f64,
    }
}

fn surface_alpha_mode(supported: &[CompositeAlphaMode], opacity: f32) -> CompositeAlphaMode {
    if clamp_opacity(opacity) >= 1.0 {
        return CompositeAlphaMode::Opaque;
    }

    [
        CompositeAlphaMode::PreMultiplied,
        CompositeAlphaMode::PostMultiplied,
        CompositeAlphaMode::Inherit,
        CompositeAlphaMode::Auto,
    ]
    .into_iter()
    .find(|mode| supported.contains(mode))
    .unwrap_or(CompositeAlphaMode::Auto)
}

fn unfocused_overlay_rect(
    appearance: &RendererAppearance,
    surface_width: u32,
    surface_height: u32,
) -> RectPlan {
    let opacity = clamp_opacity(appearance.window.unfocused_opacity);
    let alpha = 1.0 - opacity;
    RectPlan {
        x: 0,
        y: 0,
        width: surface_width.max(1),
        height: surface_height.max(1),
        color: appearance
            .panes
            .inactive_tint
            .with_alpha(alpha.clamp(0.0, 1.0)),
        layer: RectLayer::Overlay,
    }
}

pub fn effect_overlay_rects(
    appearance: &RendererAppearance,
    surface_width: u32,
    surface_height: u32,
) -> Vec<RectPlan> {
    let mut rects = Vec::new();
    if appearance.effects.scanlines {
        let line_color = Rgba::with_channels(0, 0, 0, 28);
        let height = surface_height.max(1);
        let width = surface_width.max(1);
        for y in (1..height).step_by(2) {
            rects.push(RectPlan {
                x: 0,
                y,
                width,
                height: 1,
                color: line_color,
                layer: RectLayer::Overlay,
            });
        }
    }

    if appearance.effects.retro_crt {
        let width = surface_width.max(1);
        let height = surface_height.max(1);
        let horizontal = (height / 16).max(1);
        let vertical = (width / 20).max(1);
        let edge = Rgba::with_channels(0, 0, 0, 38);
        rects.extend([
            RectPlan {
                x: 0,
                y: 0,
                width,
                height: horizontal,
                color: edge,
                layer: RectLayer::Overlay,
            },
            RectPlan {
                x: 0,
                y: height.saturating_sub(horizontal),
                width,
                height: horizontal,
                color: edge,
                layer: RectLayer::Overlay,
            },
            RectPlan {
                x: 0,
                y: 0,
                width: vertical,
                height,
                color: edge,
                layer: RectLayer::Overlay,
            },
            RectPlan {
                x: width.saturating_sub(vertical),
                y: 0,
                width: vertical,
                height,
                color: edge,
                layer: RectLayer::Overlay,
            },
        ]);
    }

    rects
}

fn load_background_image(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pipeline: &ImagePipeline,
    appearance: &RendererAppearance,
) -> Option<BackgroundImageTexture> {
    let BackgroundKind::Image(image) = &appearance.background.kind else {
        return None;
    };
    if image.load_state != ImageLoadState::Ready {
        return None;
    }

    match BackgroundImageTexture::load(device, queue, pipeline, image) {
        Ok(texture) => Some(texture),
        Err(error) => {
            eprintln!(
                "knightty renderer: failed to load background image \"{}\": {error}",
                image.path.display()
            );
            None
        }
    }
}

fn active_background_image<'a>(
    appearance: &'a RendererAppearance,
    texture: Option<&'a BackgroundImageTexture>,
) -> Option<(&'a ImageBackground, &'a BackgroundImageTexture)> {
    let BackgroundKind::Image(image) = &appearance.background.kind else {
        return None;
    };
    if image.load_state != ImageLoadState::Ready {
        return None;
    }
    texture
        .filter(|texture| texture.path == image.path)
        .map(|texture| (image, texture))
}

struct BackgroundImageTexture {
    path: PathBuf,
    width: u32,
    height: u32,
    _texture: wgpu::Texture,
    _view: wgpu::TextureView,
    bind_group: wgpu::BindGroup,
}

impl BackgroundImageTexture {
    fn load(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        pipeline: &ImagePipeline,
        image: &ImageBackground,
    ) -> Result<Self, ImageError> {
        let rgba = image::open(&image.path)?.into_rgba8();
        let (width, height) = rgba.dimensions();
        let size = wgpu::Extent3d {
            width: width.max(1),
            height: height.max(1),
            depth_or_array_layers: 1,
        };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("knightty background image texture"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        queue.write_texture(
            texture.as_image_copy(),
            &rgba.into_raw(),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * width.max(1)),
                rows_per_image: Some(height.max(1)),
            },
            size,
        );
        let view = texture.create_view(&TextureViewDescriptor::default());
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("knightty background image bind group"),
            layout: &pipeline.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&pipeline.background_sampler),
                },
            ],
        });

        Ok(Self {
            path: image.path.clone(),
            width,
            height,
            _texture: texture,
            _view: view,
            bind_group,
        })
    }
}

struct InlineImageTexture {
    _texture: wgpu::Texture,
    _view: wgpu::TextureView,
    bind_group: wgpu::BindGroup,
}

impl InlineImageTexture {
    fn upload(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        pipeline: &ImagePipeline,
        image: &InlineImageData,
    ) -> Result<Self, InlineImageUploadError> {
        if image.width == 0 || image.height == 0 {
            return Err(InlineImageUploadError::InvalidDimensions);
        }
        let expected_bytes = u64::from(image.width)
            .checked_mul(u64::from(image.height))
            .and_then(|value| value.checked_mul(4))
            .ok_or(InlineImageUploadError::SizeOverflow)?;
        if image.rgba.len() as u64 != expected_bytes {
            return Err(InlineImageUploadError::InvalidByteLength);
        }

        let size = wgpu::Extent3d {
            width: image.width,
            height: image.height,
            depth_or_array_layers: 1,
        };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("knightty inline image texture"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        queue.write_texture(
            texture.as_image_copy(),
            &image.rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * image.width),
                rows_per_image: Some(image.height),
            },
            size,
        );
        let view = texture.create_view(&TextureViewDescriptor::default());
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("knightty inline image bind group"),
            layout: &pipeline.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&pipeline.inline_sampler),
                },
            ],
        });

        Ok(Self {
            _texture: texture,
            _view: view,
            bind_group,
        })
    }
}

#[derive(Debug, Error)]
enum InlineImageUploadError {
    #[error("image dimensions must be non-zero")]
    InvalidDimensions,
    #[error("image RGBA byte length does not match its dimensions")]
    InvalidByteLength,
    #[error("image byte length overflowed")]
    SizeOverflow,
}

struct ImagePipeline {
    pipeline: RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    background_sampler: wgpu::Sampler,
    inline_sampler: wgpu::Sampler,
    background_vertices: ImageVertexBuffer,
    inline_vertices: ImageVertexBuffer,
    inline_draws: Vec<PreparedInlineImageDraw>,
}

struct PreparedInlineImageDraw {
    image_id: ImageId,
    vertices: Range<u32>,
    layer: InlineImageLayer,
}

impl ImagePipeline {
    fn new(device: &wgpu::Device, format: TextureFormat) -> Self {
        let shader = device.create_shader_module(ShaderModuleDescriptor {
            label: Some("knightty background image shader"),
            source: ShaderSource::Wgsl(Cow::Borrowed(IMAGE_SHADER)),
        });
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("knightty background image bind group layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let background_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("knightty background image sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let inline_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("knightty inline image sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: Some("knightty background image pipeline layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });
        let attributes = [
            VertexAttribute {
                format: VertexFormat::Float32x2,
                offset: 0,
                shader_location: 0,
            },
            VertexAttribute {
                format: VertexFormat::Float32x2,
                offset: mem::size_of::<[f32; 2]>() as u64,
                shader_location: 1,
            },
            VertexAttribute {
                format: VertexFormat::Float32,
                offset: mem::size_of::<[f32; 4]>() as u64,
                shader_location: 2,
            },
        ];
        let buffers = [VertexBufferLayout {
            array_stride: mem::size_of::<ImageVertex>() as u64,
            step_mode: VertexStepMode::Vertex,
            attributes: &attributes,
        }];
        let targets = [Some(ColorTargetState {
            format,
            blend: Some(BlendState::ALPHA_BLENDING),
            write_mask: ColorWrites::ALL,
        })];
        let pipeline = device.create_render_pipeline(&RenderPipelineDescriptor {
            label: Some("knightty background image pipeline"),
            layout: Some(&layout),
            vertex: VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: PipelineCompilationOptions::default(),
                buffers: &buffers,
            },
            primitive: PrimitiveState::default(),
            depth_stencil: None,
            multisample: MultisampleState::default(),
            fragment: Some(FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: PipelineCompilationOptions::default(),
                targets: &targets,
            }),
            multiview_mask: None,
            cache: None,
        });

        Self {
            pipeline,
            bind_group_layout,
            background_sampler,
            inline_sampler,
            background_vertices: ImageVertexBuffer::new(device),
            inline_vertices: ImageVertexBuffer::new(device),
            inline_draws: Vec::new(),
        }
    }

    fn prepare_background(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        surface_width: u32,
        surface_height: u32,
        background: Option<(&ImageBackground, &BackgroundImageTexture)>,
    ) {
        let Some((image, texture)) = background else {
            self.background_vertices.vertex_count = 0;
            return;
        };
        self.background_vertices.prepare(
            device,
            queue,
            ImageVertexParams {
                fit: image.fit,
                opacity: clamp_opacity(image.opacity),
                image_width: texture.width,
                image_height: texture.height,
                surface_width,
                surface_height,
            },
        );
    }

    fn draw(&self, pass: &mut RenderPass<'_>, texture: Option<&BackgroundImageTexture>) {
        let Some(texture) = texture else {
            return;
        };
        if self.background_vertices.vertex_count == 0 {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &texture.bind_group, &[]);
        pass.set_vertex_buffer(0, self.background_vertices.buffer.slice(..));
        pass.draw(0..self.background_vertices.vertex_count, 0..1);
    }

    fn prepare_inline(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        surface_width: u32,
        surface_height: u32,
        plans: &[ImageQuadPlan],
    ) {
        self.inline_draws.clear();
        let mut vertices = Vec::with_capacity(plans.len().saturating_mul(6));
        for plan in plans {
            let start = vertices.len() as u32;
            vertices.extend(inline_image_vertices(*plan, surface_width, surface_height));
            let end = vertices.len() as u32;
            self.inline_draws.push(PreparedInlineImageDraw {
                image_id: plan.image_id,
                vertices: start..end,
                layer: InlineImageLayer::for_z_index(plan.z_index),
            });
        }
        self.inline_vertices
            .prepare_vertices(device, queue, &vertices);
    }

    fn draw_inline<'a>(
        &'a self,
        pass: &mut RenderPass<'a>,
        textures: &'a BTreeMap<ImageId, InlineImageTexture>,
        layer: InlineImageLayer,
    ) {
        if self.inline_vertices.vertex_count == 0 {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_vertex_buffer(0, self.inline_vertices.buffer.slice(..));
        for draw in self.inline_draws.iter().filter(|draw| draw.layer == layer) {
            let Some(texture) = textures.get(&draw.image_id) else {
                continue;
            };
            pass.set_bind_group(0, &texture.bind_group, &[]);
            pass.draw(draw.vertices.clone(), 0..1);
        }
    }
}

struct ImageVertexBuffer {
    buffer: wgpu::Buffer,
    size: u64,
    vertex_count: u32,
}

#[derive(Clone, Copy)]
struct ImageVertexParams {
    fit: ImageFit,
    opacity: f32,
    image_width: u32,
    image_height: u32,
    surface_width: u32,
    surface_height: u32,
}

impl ImageVertexBuffer {
    fn new(device: &wgpu::Device) -> Self {
        let size = mem::size_of::<ImageVertex>() as u64;
        let buffer = device.create_buffer(&BufferDescriptor {
            label: Some("knightty background image vertices"),
            size,
            usage: BufferUsages::VERTEX | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            buffer,
            size,
            vertex_count: 0,
        }
    }

    fn prepare(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, params: ImageVertexParams) {
        let vertices = image_vertices(
            params.fit,
            params.image_width,
            params.image_height,
            params.opacity,
            params.surface_width,
            params.surface_height,
        );
        self.prepare_vertices(device, queue, &vertices);
    }

    fn prepare_vertices(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        vertices: &[ImageVertex],
    ) {
        self.vertex_count = vertices.len() as u32;
        if vertices.is_empty() {
            return;
        }

        let bytes = bytemuck::cast_slice(vertices);
        if bytes.len() as u64 > self.size {
            self.buffer.destroy();
            self.size = next_image_buffer_size(bytes.len() as u64);
            self.buffer = device.create_buffer(&BufferDescriptor {
                label: Some("knightty background image vertices"),
                size: self.size,
                usage: BufferUsages::VERTEX | BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        queue.write_buffer(&self.buffer, 0, bytes);
    }
}

fn inline_image_vertices(
    quad: ImageQuadPlan,
    surface_width: u32,
    surface_height: u32,
) -> [ImageVertex; 6] {
    let left = to_ndc_x_f32(quad.destination.x, surface_width);
    let right = to_ndc_x_f32(quad.destination.x + quad.destination.width, surface_width);
    let top = to_ndc_y_f32(quad.destination.y, surface_height);
    let bottom = to_ndc_y_f32(quad.destination.y + quad.destination.height, surface_height);

    [
        ImageVertex::new(left, top, quad.uv.u0, quad.uv.v0, 1.0),
        ImageVertex::new(right, top, quad.uv.u1, quad.uv.v0, 1.0),
        ImageVertex::new(right, bottom, quad.uv.u1, quad.uv.v1, 1.0),
        ImageVertex::new(left, top, quad.uv.u0, quad.uv.v0, 1.0),
        ImageVertex::new(right, bottom, quad.uv.u1, quad.uv.v1, 1.0),
        ImageVertex::new(left, bottom, quad.uv.u0, quad.uv.v1, 1.0),
    ]
}

fn image_vertices(
    fit: ImageFit,
    image_width: u32,
    image_height: u32,
    opacity: f32,
    surface_width: u32,
    surface_height: u32,
) -> Vec<ImageVertex> {
    let quad = image_quad(
        fit,
        image_width,
        image_height,
        surface_width,
        surface_height,
    );
    let left = to_ndc_x_f32(quad.x, surface_width);
    let right = to_ndc_x_f32(quad.x + quad.width, surface_width);
    let top = to_ndc_y_f32(quad.y, surface_height);
    let bottom = to_ndc_y_f32(quad.y + quad.height, surface_height);
    let opacity = clamp_opacity(opacity);

    vec![
        ImageVertex::new(left, top, quad.u0, quad.v0, opacity),
        ImageVertex::new(right, top, quad.u1, quad.v0, opacity),
        ImageVertex::new(right, bottom, quad.u1, quad.v1, opacity),
        ImageVertex::new(left, top, quad.u0, quad.v0, opacity),
        ImageVertex::new(right, bottom, quad.u1, quad.v1, opacity),
        ImageVertex::new(left, bottom, quad.u0, quad.v1, opacity),
    ]
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct ImageQuad {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    u0: f32,
    v0: f32,
    u1: f32,
    v1: f32,
}

fn image_quad(
    fit: ImageFit,
    image_width: u32,
    image_height: u32,
    surface_width: u32,
    surface_height: u32,
) -> ImageQuad {
    let image_width = image_width.max(1) as f32;
    let image_height = image_height.max(1) as f32;
    let surface_width = surface_width.max(1) as f32;
    let surface_height = surface_height.max(1) as f32;

    match fit {
        ImageFit::Stretch => ImageQuad {
            x: 0.0,
            y: 0.0,
            width: surface_width,
            height: surface_height,
            u0: 0.0,
            v0: 0.0,
            u1: 1.0,
            v1: 1.0,
        },
        ImageFit::Contain => {
            let scale = (surface_width / image_width).min(surface_height / image_height);
            let width = image_width * scale;
            let height = image_height * scale;
            centered_image_quad(surface_width, surface_height, width, height)
        }
        ImageFit::Cover => {
            cover_image_quad(surface_width, surface_height, image_width, image_height)
        }
        ImageFit::Tile => ImageQuad {
            x: 0.0,
            y: 0.0,
            width: surface_width,
            height: surface_height,
            u0: 0.0,
            v0: 0.0,
            u1: surface_width / image_width,
            v1: surface_height / image_height,
        },
        ImageFit::Center => {
            centered_image_quad(surface_width, surface_height, image_width, image_height)
        }
    }
}

fn centered_image_quad(
    surface_width: f32,
    surface_height: f32,
    width: f32,
    height: f32,
) -> ImageQuad {
    ImageQuad {
        x: (surface_width - width) * 0.5,
        y: (surface_height - height) * 0.5,
        width,
        height,
        u0: 0.0,
        v0: 0.0,
        u1: 1.0,
        v1: 1.0,
    }
}

fn cover_image_quad(
    surface_width: f32,
    surface_height: f32,
    image_width: f32,
    image_height: f32,
) -> ImageQuad {
    let image_aspect = image_width / image_height;
    let surface_aspect = surface_width / surface_height;
    let (u0, v0, u1, v1) = if image_aspect > surface_aspect {
        let visible_width = surface_aspect / image_aspect;
        let inset = (1.0 - visible_width) * 0.5;
        (inset, 0.0, 1.0 - inset, 1.0)
    } else {
        let visible_height = image_aspect / surface_aspect;
        let inset = (1.0 - visible_height) * 0.5;
        (0.0, inset, 1.0, 1.0 - inset)
    };

    ImageQuad {
        x: 0.0,
        y: 0.0,
        width: surface_width,
        height: surface_height,
        u0,
        v0,
        u1,
        v1,
    }
}

fn to_ndc_x_f32(x: f32, width: u32) -> f32 {
    (x / width.max(1) as f32) * 2.0 - 1.0
}

fn to_ndc_y_f32(y: f32, height: u32) -> f32 {
    1.0 - (y / height.max(1) as f32) * 2.0
}

fn next_image_buffer_size(size: u64) -> u64 {
    size.next_power_of_two()
        .max(mem::size_of::<ImageVertex>() as u64)
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct ImageVertex {
    position: [f32; 2],
    uv: [f32; 2],
    opacity: f32,
}

impl ImageVertex {
    fn new(x: f32, y: f32, u: f32, v: f32, opacity: f32) -> Self {
        Self {
            position: [x, y],
            uv: [u, v],
            opacity,
        }
    }
}

const IMAGE_SHADER: &str = r#"
@group(0) @binding(0) var background_texture: texture_2d<f32>;
@group(0) @binding(1) var background_sampler: sampler;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) opacity: f32,
};

@vertex
fn vs_main(
    @location(0) position: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) opacity: f32,
) -> VertexOutput {
    var output: VertexOutput;
    output.position = vec4<f32>(position, 0.0, 1.0);
    output.uv = uv;
    output.opacity = opacity;
    return output;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    let sampled = textureSample(background_texture, background_sampler, input.uv);
    return vec4<f32>(sampled.rgb, sampled.a * input.opacity);
}
"#;

struct RectPipeline {
    pipeline: RenderPipeline,
    background: RectVertexBuffer,
    selection: RectVertexBuffer,
    overlay: RectVertexBuffer,
    target_is_srgb: bool,
}

struct RectPrepareParams<'a> {
    surface_width: u32,
    surface_height: u32,
    background_rects: &'a [RectPlan],
    selection_rects: &'a [RectPlan],
    overlay_rects: &'a [RectPlan],
}

impl RectPipeline {
    fn new(device: &wgpu::Device, format: TextureFormat) -> Self {
        let shader = device.create_shader_module(ShaderModuleDescriptor {
            label: Some("knightty rect shader"),
            source: ShaderSource::Wgsl(Cow::Borrowed(RECT_SHADER)),
        });
        let layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: Some("knightty rect pipeline layout"),
            bind_group_layouts: &[],
            immediate_size: 0,
        });
        let attributes = [
            VertexAttribute {
                format: VertexFormat::Float32x2,
                offset: 0,
                shader_location: 0,
            },
            VertexAttribute {
                format: VertexFormat::Float32x4,
                offset: mem::size_of::<[f32; 2]>() as u64,
                shader_location: 1,
            },
        ];
        let buffers = [VertexBufferLayout {
            array_stride: mem::size_of::<RectVertex>() as u64,
            step_mode: VertexStepMode::Vertex,
            attributes: &attributes,
        }];
        let targets = [Some(ColorTargetState {
            format,
            blend: Some(BlendState::ALPHA_BLENDING),
            write_mask: ColorWrites::ALL,
        })];
        let pipeline = device.create_render_pipeline(&RenderPipelineDescriptor {
            label: Some("knightty rect pipeline"),
            layout: Some(&layout),
            vertex: VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: PipelineCompilationOptions::default(),
                buffers: &buffers,
            },
            primitive: PrimitiveState::default(),
            depth_stencil: None,
            multisample: MultisampleState::default(),
            fragment: Some(FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: PipelineCompilationOptions::default(),
                targets: &targets,
            }),
            multiview_mask: None,
            cache: None,
        });

        Self {
            pipeline,
            background: RectVertexBuffer::new(device, "knightty background rect vertices"),
            selection: RectVertexBuffer::new(device, "knightty selection rect vertices"),
            overlay: RectVertexBuffer::new(device, "knightty overlay rect vertices"),
            target_is_srgb: format.is_srgb(),
        }
    }

    fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        params: RectPrepareParams<'_>,
    ) {
        self.background.prepare(
            device,
            queue,
            params.surface_width,
            params.surface_height,
            params.background_rects,
            self.target_is_srgb,
        );
        self.selection.prepare(
            device,
            queue,
            params.surface_width,
            params.surface_height,
            params.selection_rects,
            self.target_is_srgb,
        );
        self.overlay.prepare(
            device,
            queue,
            params.surface_width,
            params.surface_height,
            params.overlay_rects,
            self.target_is_srgb,
        );
    }

    fn draw(&self, pass: &mut RenderPass<'_>, layer: PreparedRectLayer) {
        let buffer = match layer {
            PreparedRectLayer::Background => &self.background,
            PreparedRectLayer::Selection => &self.selection,
            PreparedRectLayer::Overlay => &self.overlay,
        };
        if buffer.vertex_count == 0 {
            return;
        }

        pass.set_pipeline(&self.pipeline);
        pass.set_vertex_buffer(0, buffer.buffer.slice(..));
        pass.draw(0..buffer.vertex_count, 0..1);
    }
}

enum PreparedRectLayer {
    Background,
    Selection,
    Overlay,
}

struct RectVertexBuffer {
    buffer: wgpu::Buffer,
    size: u64,
    vertex_count: u32,
}

impl RectVertexBuffer {
    fn new(device: &wgpu::Device, label: &'static str) -> Self {
        let size = mem::size_of::<RectVertex>() as u64;
        let buffer = device.create_buffer(&BufferDescriptor {
            label: Some(label),
            size,
            usage: BufferUsages::VERTEX | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            buffer,
            size,
            vertex_count: 0,
        }
    }

    fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        surface_width: u32,
        surface_height: u32,
        rects: &[RectPlan],
        target_is_srgb: bool,
    ) {
        let vertices = rect_vertices(rects, surface_width, surface_height, target_is_srgb);
        self.vertex_count = vertices.len() as u32;
        if vertices.is_empty() {
            return;
        }

        let bytes = bytemuck::cast_slice(&vertices);
        if bytes.len() as u64 > self.size {
            self.buffer.destroy();
            self.size = next_buffer_size(bytes.len() as u64);
            self.buffer = device.create_buffer(&BufferDescriptor {
                label: Some("knightty rect vertices"),
                size: self.size,
                usage: BufferUsages::VERTEX | BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        queue.write_buffer(&self.buffer, 0, bytes);
    }
}

fn rect_vertices(
    rects: &[RectPlan],
    surface_width: u32,
    surface_height: u32,
    target_is_srgb: bool,
) -> Vec<RectVertex> {
    let mut vertices = Vec::with_capacity(rects.len() * 6);
    for rect in rects {
        let left = to_ndc_x(rect.x, surface_width);
        let right = to_ndc_x(rect.x.saturating_add(rect.width), surface_width);
        let top = to_ndc_y(rect.y, surface_height);
        let bottom = to_ndc_y(rect.y.saturating_add(rect.height), surface_height);
        let color = rect.color.to_target_f32(target_is_srgb);

        vertices.extend_from_slice(&[
            RectVertex::new(left, top, color),
            RectVertex::new(right, top, color),
            RectVertex::new(right, bottom, color),
            RectVertex::new(left, top, color),
            RectVertex::new(right, bottom, color),
            RectVertex::new(left, bottom, color),
        ]);
    }
    vertices
}

fn to_ndc_x(x: u32, width: u32) -> f32 {
    (x as f32 / width.max(1) as f32) * 2.0 - 1.0
}

fn to_ndc_y(y: u32, height: u32) -> f32 {
    1.0 - (y as f32 / height.max(1) as f32) * 2.0
}

fn next_buffer_size(size: u64) -> u64 {
    size.next_power_of_two()
        .max(mem::size_of::<RectVertex>() as u64)
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct RectVertex {
    position: [f32; 2],
    color: [f32; 4],
}

impl RectVertex {
    fn new(x: f32, y: f32, color: [f32; 4]) -> Self {
        Self {
            position: [x, y],
            color,
        }
    }
}

const RECT_SHADER: &str = r#"
struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_main(@location(0) position: vec2<f32>, @location(1) color: vec4<f32>) -> VertexOutput {
    var output: VertexOutput;
    output.position = vec4<f32>(position, 0.0, 1.0);
    output.color = color;
    return output;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    return input.color;
}
"#;

async fn select_adapter(
    instance: &Instance,
    surface: &wgpu::Surface<'static>,
) -> Result<Adapter, RenderError> {
    let adapters = instance.enumerate_adapters(Backends::all()).await;
    for adapter in &adapters {
        let info = adapter.get_info();
        let supported = adapter.is_surface_supported(surface);
        eprintln!(
            "knightty renderer: candidate adapter=\"{}\" backend={:?} device_type={:?} supported_surface={}",
            info.name, info.backend, info.device_type, supported,
        );
    }

    adapters
        .iter()
        .filter(|adapter| adapter.is_surface_supported(surface))
        .find(|adapter| adapter.get_info().device_type != DeviceType::Cpu)
        .cloned()
        .or_else(|| {
            adapters
                .into_iter()
                .find(|adapter| adapter.is_surface_supported(surface))
        })
        .ok_or(RenderError::NoAdapter)
}

#[derive(Debug, Error)]
pub enum RenderError {
    #[error("failed to create wgpu surface: {0}")]
    CreateSurface(#[from] wgpu::CreateSurfaceError),
    #[error("no compatible wgpu adapter found")]
    NoAdapter,
    #[error("failed to request wgpu device: {0}")]
    RequestDevice(#[from] wgpu::RequestDeviceError),
    #[error("surface has no supported texture formats")]
    NoSurfaceFormat,
    #[error("text prepare failed: {0}")]
    TextPrepare(#[from] glyphon::PrepareError),
    #[error("text render failed: {0}")]
    TextRender(#[from] glyphon::RenderError),
    #[error("surface was lost")]
    SurfaceLost,
    #[error("GPU out of memory")]
    OutOfMemory,
    #[error("surface validation failed")]
    SurfaceValidation,
}

#[cfg(test)]
mod tests {
    use super::{
        BELOW_CELL_BACKGROUND, BackgroundAppearance, BackgroundKind, CellMetrics, CursorStyle,
        DEFAULT_FG, GradientBackground, GradientOrientation, ImageBackground, ImageFit,
        ImageLoadState, ImageQuadPlan, ImePreedit, InlineImageData, InlineImageLayer, PixelRect,
        RectLayer, RectPlan, RenderOptions, RendererAppearance, Rgba, SearchMatch, TextStyle,
        UvRect, attrs_for_style, background_pass_rects, build_render_plan,
        build_render_plan_with_appearance, build_render_plan_with_options, builtin_theme,
        builtin_theme_names, cell_span_scale, clamp_blur_radius, clamp_opacity,
        effect_overlay_rects, image_quad, image_vertices, inline_image_cache_changes,
        resolve_theme_name, unique_font_family_infos,
    };
    use glyphon::Family;
    use glyphon::cosmic_text::FeatureTag;
    use knightty_core::{
        Damage, GridPoint, ImageId, ImagePixelOffset, ImagePlacement, ImagePlacementId,
        ImageSourceRect, ImageZIndex, SelectionMode, SelectionPoint, Terminal,
    };
    use std::path::PathBuf;
    use std::sync::Arc;

    fn ime_options(text: &str, cursor_range: Option<std::ops::Range<usize>>) -> RenderOptions {
        RenderOptions {
            ime_preedit: Some(ImePreedit {
                text: text.to_owned(),
                cursor_range,
            }),
            ..RenderOptions::default()
        }
    }

    #[test]
    fn font_family_infos_are_sorted_trimmed_and_deduplicated() {
        let infos = unique_font_family_infos([
            (" Inter ", false),
            ("CaskaydiaCove Nerd Font", true),
            ("", true),
            ("Inter", true),
        ]);

        assert_eq!(infos.len(), 2);
        assert_eq!(infos[0].name, "CaskaydiaCove Nerd Font");
        assert!(infos[0].monospaced);
        assert_eq!(infos[1].name, "Inter");
        assert!(infos[1].monospaced);
    }

    #[test]
    fn hex_rgb_parser_accepts_rrggbb_and_rejects_invalid_values() {
        assert_eq!(Rgba::from_hex_rgb("#1e1e2e"), Ok(Rgba::rgb(30, 30, 46)));
        assert!(Rgba::from_hex_rgb("1e1e2e").is_err());
        assert!(Rgba::from_hex_rgb("#abcd").is_err());
        assert!(Rgba::from_hex_rgb("#zzzzzz").is_err());
    }

    #[test]
    fn opacity_and_blur_radius_are_clamped() {
        assert_eq!(clamp_opacity(-0.5), 0.0);
        assert_eq!(clamp_opacity(1.5), 1.0);
        assert_eq!(clamp_opacity(f32::NAN), 1.0);
        assert_eq!(clamp_blur_radius(999), 100);
    }

    #[test]
    fn catppuccin_theme_registry_resolves_all_flavors() {
        assert_eq!(
            builtin_theme_names(),
            &[
                "Catppuccin Latte",
                "Catppuccin Frappe",
                "Catppuccin Macchiato",
                "Catppuccin Mocha",
            ]
        );

        for name in builtin_theme_names() {
            assert!(builtin_theme(name).is_some(), "{name} should resolve");
        }

        let mocha = builtin_theme("Catppuccin Mocha").expect("mocha theme");
        assert_eq!(mocha.background, Rgba::rgb(30, 30, 46));
        assert_eq!(mocha.foreground, Rgba::rgb(205, 214, 244));
        assert_eq!(mocha.cursor, Rgba::rgb(245, 224, 220));
    }

    #[test]
    fn unknown_theme_falls_back_to_default_with_warning() {
        let (theme, warning) = resolve_theme_name(Some("No Such Theme"));

        assert_eq!(theme, builtin_theme("Catppuccin Mocha").unwrap());
        assert!(
            warning
                .expect("warning")
                .contains("Available themes: Catppuccin Latte")
        );
    }

    #[test]
    fn cursor_style_parser_accepts_config_values() {
        assert_eq!(CursorStyle::parse("block"), Some(CursorStyle::Block));
        assert_eq!(CursorStyle::parse("bar"), Some(CursorStyle::Bar));
        assert_eq!(
            CursorStyle::parse("underline"),
            Some(CursorStyle::Underline)
        );
        assert_eq!(
            CursorStyle::parse("hollow_block"),
            Some(CursorStyle::HollowBlock)
        );
        assert_eq!(CursorStyle::parse("beam"), None);
    }

    #[test]
    fn render_plan_preserves_truecolor_foreground_text_style() {
        let mut terminal = Terminal::new(4, 1);
        terminal.feed(b"\x1b[38;2;255;0;0mX");

        let plan = build_render_plan(&terminal.snapshot(), &Damage::Full, CellMetrics::default());
        assert!(plan.text.iter().any(|segment| {
            segment.text == "X"
                && segment.style.fg == Rgba::rgb(255, 0, 0)
                && !segment.style.bold
                && !segment.style.italic
        }));
    }

    #[test]
    fn render_plan_emits_one_scaled_text_segment_for_cell_span() {
        let mut terminal = Terminal::new(8, 4);
        terminal.feed(b"\x1b[2;3H");
        terminal.place_cell_span("界", 3, 2).unwrap();
        let metrics = CellMetrics {
            width: 10,
            height: 20,
            font_size: 16.0,
            line_height: 20.0,
        };

        let plan = build_render_plan(&terminal.snapshot(), &Damage::Full, metrics);
        let spans = plan
            .text
            .iter()
            .filter(|segment| segment.cell_span)
            .collect::<Vec<_>>();

        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].text, "界");
        assert_eq!(spans[0].x, 20);
        assert_eq!(spans[0].y, 20);
        assert_eq!(spans[0].width, 30);
        assert_eq!(spans[0].height, 40);
        assert!(
            !plan
                .text
                .iter()
                .any(|segment| !segment.cell_span && segment.text == "界")
        );
    }

    #[test]
    fn cell_span_scale_uses_glyph_advance_instead_of_cell_occupancy() {
        assert_eq!(cell_span_scale(10.0, 30.0, 100.0, 20.0), Some(3.0));
        assert_eq!(cell_span_scale(15.0, 30.0, 100.0, 20.0), Some(2.0));
        assert_eq!(cell_span_scale(10.0, 30.0, 40.0, 20.0), Some(2.0));
        assert_eq!(cell_span_scale(0.0, 30.0, 40.0, 20.0), None);
    }

    #[test]
    fn render_plan_expands_search_match_to_entire_cell_span() {
        let mut terminal = Terminal::new(8, 4);
        terminal.feed(b"\x1b[2;3H");
        terminal.place_cell_span("A", 3, 2).unwrap();
        let metrics = CellMetrics::default();
        let appearance = RendererAppearance::default();

        let plan = build_render_plan_with_options(
            &terminal.snapshot(),
            &Damage::Full,
            metrics,
            RenderOptions {
                search_matches: vec![SearchMatch {
                    row: 1,
                    start_col: 2,
                    end_col: 3,
                }],
                ..RenderOptions::default()
            },
        );

        assert!(plan.rects.iter().any(|rect| {
            rect.x == metrics.width * 2
                && rect.y == metrics.height
                && rect.width == metrics.width * 3
                && rect.height == metrics.height * 2
                && rect.color == appearance.search.background
        }));
    }

    #[test]
    fn render_plan_draws_cell_span_underline_once_at_rectangle_bottom() {
        let mut terminal = Terminal::new(8, 4);
        terminal.feed(b"\x1b[4m");
        terminal.place_cell_span("A", 3, 2).unwrap();
        let metrics = CellMetrics::default();

        let plan = build_render_plan(&terminal.snapshot(), &Damage::Full, metrics);
        let matching = plan
            .rects
            .iter()
            .filter(|rect| {
                rect.layer == RectLayer::Overlay
                    && rect.width == metrics.width * 3
                    && rect.y == metrics.height * 2 - 2
            })
            .count();

        assert_eq!(matching, 1);
    }

    #[test]
    fn render_plan_preserves_truecolor_background_rect_color() {
        let mut terminal = Terminal::new(4, 1);
        terminal.feed(b"\x1b[48;2;0;128;255mX");

        let plan = build_render_plan(&terminal.snapshot(), &Damage::Full, CellMetrics::default());

        assert!(plan.rects.iter().any(|rect| {
            rect.layer == RectLayer::Background
                && rect.x == 0
                && rect.y == 0
                && rect.color == Rgba::rgb(0, 128, 255)
                && rect.color.a == 255
        }));
    }

    #[test]
    fn render_plan_ansi_palette_uses_saturated_base_colors() {
        let mut terminal = Terminal::new(4, 1);
        terminal.feed(b"\x1b[31mX");

        let plan = build_render_plan(&terminal.snapshot(), &Damage::Full, CellMetrics::default());

        assert!(plan.text.iter().any(|segment| {
            segment.text == "X" && segment.style.fg == Rgba::rgb(243, 139, 168)
        }));
    }

    #[test]
    fn render_plan_256_color_cube_uses_expected_rgb_steps() {
        let mut terminal = Terminal::new(4, 1);
        terminal.feed(b"\x1b[38;5;196mX");

        let plan = build_render_plan(&terminal.snapshot(), &Damage::Full, CellMetrics::default());

        assert!(
            plan.text
                .iter()
                .any(|segment| { segment.text == "X" && segment.style.fg == Rgba::rgb(255, 0, 0) })
        );
    }

    #[test]
    fn render_plan_256_grayscale_uses_expected_ramp() {
        let mut terminal = Terminal::new(4, 1);
        terminal.feed(b"\x1b[38;5;244mX");

        let plan = build_render_plan(&terminal.snapshot(), &Damage::Full, CellMetrics::default());

        assert!(plan.text.iter().any(|segment| {
            segment.text == "X" && segment.style.fg == Rgba::rgb(128, 128, 128)
        }));
    }

    #[test]
    fn render_plan_bold_keeps_truecolor_rgb() {
        let mut terminal = Terminal::new(4, 1);
        terminal.feed(b"\x1b[1;38;2;64;128;192mX");

        let plan = build_render_plan(&terminal.snapshot(), &Damage::Full, CellMetrics::default());

        assert!(plan.text.iter().any(|segment| {
            segment.text == "X" && segment.style.bold && segment.style.fg == Rgba::rgb(64, 128, 192)
        }));
    }

    #[test]
    fn render_plan_dim_sgr_does_not_modify_truecolor_rgb() {
        let mut terminal = Terminal::new(4, 1);
        terminal.feed(b"\x1b[2;38;2;64;128;192mX");

        let plan = build_render_plan(&terminal.snapshot(), &Damage::Full, CellMetrics::default());

        assert!(
            plan.text.iter().any(|segment| {
                segment.text == "X" && segment.style.fg == Rgba::rgb(64, 128, 192)
            })
        );
    }

    #[test]
    fn rect_vertices_keep_srgb_values_for_linear_targets() {
        let rect = RectPlan {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
            color: Rgba::rgb(128, 64, 255),
            layer: RectLayer::Background,
        };

        let vertices = super::rect_vertices(&[rect], 100, 100, false);
        let color = vertices[0].color;

        assert_close(color[0], 128.0 / 255.0);
        assert_close(color[1], 64.0 / 255.0);
        assert_close(color[2], 1.0);
        assert_close(color[3], 1.0);
    }

    #[test]
    fn rect_vertices_convert_srgb_values_for_srgb_targets() {
        let rect = RectPlan {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
            color: Rgba::rgb(128, 64, 255),
            layer: RectLayer::Background,
        };

        let vertices = super::rect_vertices(&[rect], 100, 100, true);
        let color = vertices[0].color;

        assert_close(color[0], super::srgb_channel_to_linear(128));
        assert_close(color[1], super::srgb_channel_to_linear(64));
        assert_close(color[2], 1.0);
        assert_close(color[3], 1.0);
        assert!(color[0] < 128.0 / 255.0);
        assert!(color[1] < 64.0 / 255.0);
    }

    #[test]
    fn cell_metrics_scale_from_font_size_and_line_height() {
        let metrics = CellMetrics::from_font_size(18.0, 22.0);

        assert_eq!(metrics.font_size, 18.0);
        assert_eq!(metrics.line_height, 22.0);
        assert_eq!(metrics.width, 10);
        assert_eq!(metrics.height, 22);
    }

    #[test]
    fn render_plan_applies_padding_to_grid_origin() {
        let mut terminal = Terminal::new(4, 1);
        terminal.feed(b"X");
        let metrics = CellMetrics::default();
        let mut appearance = RendererAppearance::default();
        appearance.window.padding_x = 12;
        appearance.window.padding_y = 10;

        let plan = build_render_plan_with_appearance(
            &terminal.snapshot(),
            &Damage::Full,
            metrics,
            &appearance,
            &RenderOptions::default(),
        );

        let segment = plan
            .text
            .iter()
            .find(|segment| segment.text == "X")
            .expect("text segment");
        assert_eq!(segment.x, 12);
        assert_eq!(segment.y, 10);
        assert!(plan.rects.iter().any(|rect| {
            rect.layer == RectLayer::Background
                && rect.x == 12 + metrics.width
                && rect.y == 10
                && rect.width == metrics.width
        }));
    }

    #[test]
    fn render_plan_applies_current_search_colors() {
        let mut terminal = Terminal::new(4, 1);
        terminal.feed(b"X");

        let plan = build_render_plan_with_options(
            &terminal.snapshot(),
            &Damage::Full,
            CellMetrics::default(),
            RenderOptions {
                current_search_match: Some(SearchMatch {
                    row: 0,
                    start_col: 0,
                    end_col: 1,
                }),
                ..RenderOptions::default()
            },
        );

        assert!(
            plan.text.iter().any(|segment| {
                segment.text == "X" && segment.style.fg == Rgba::rgb(30, 30, 46)
            })
        );
        assert!(plan.rects.iter().any(|rect| {
            rect.layer == RectLayer::Background
                && rect.color == Rgba::rgb(250, 179, 135)
                && rect.width == CellMetrics::default().width
        }));
    }

    #[test]
    fn render_plan_adds_search_query_bar_when_search_is_active() {
        let terminal = Terminal::new(8, 1);

        let plan = build_render_plan_with_options(
            &terminal.snapshot(),
            &Damage::Full,
            CellMetrics::default(),
            RenderOptions {
                search_query: Some("needle".to_owned()),
                ..RenderOptions::default()
            },
        );

        assert!(plan.text.iter().any(|segment| {
            segment.text == "/needle" && segment.style.fg == Rgba::rgb(30, 30, 46)
        }));
        assert!(plan.rects.iter().any(|rect| {
            rect.layer == RectLayer::Background
                && rect.color == Rgba::with_channels(250, 179, 135, 235)
        }));
    }

    #[test]
    fn render_plan_adds_single_tab_bar_when_enabled() {
        let terminal = Terminal::new(8, 1);
        let mut appearance = RendererAppearance::default();
        appearance.tabs.enabled = true;
        appearance.tabs.show_when_single = true;

        let plan = build_render_plan_with_appearance(
            &terminal.snapshot(),
            &Damage::Full,
            CellMetrics::default(),
            &appearance,
            &RenderOptions::default(),
        );

        assert!(plan.text.iter().any(|segment| {
            segment.text == "knightty" && segment.style.fg == appearance.tabs.active_foreground
        }));
        assert!(plan.rects.iter().any(|rect| {
            rect.layer == RectLayer::Background && rect.color == appearance.tabs.active_background
        }));
    }

    #[test]
    fn search_query_bar_is_placed_below_visible_tab_bar() {
        let terminal = Terminal::new(8, 1);
        let mut appearance = RendererAppearance::default();
        appearance.tabs.enabled = true;
        appearance.tabs.show_when_single = true;
        let metrics = CellMetrics::default();

        let plan = build_render_plan_with_appearance(
            &terminal.snapshot(),
            &Damage::Full,
            metrics,
            &appearance,
            &RenderOptions {
                search_query: Some("x".to_owned()),
                ..RenderOptions::default()
            },
        );

        let search_segment = plan
            .text
            .iter()
            .find(|segment| segment.text == "/x")
            .expect("search segment");
        assert_eq!(search_segment.y, metrics.height);
    }

    #[test]
    fn selection_foreground_has_priority_over_search_match() {
        let mut terminal = Terminal::new(4, 1);
        terminal.feed(b"XY");
        terminal.begin_selection(SelectionPoint { col: 0, row: 0 }, SelectionMode::Simple);
        terminal.update_selection(SelectionPoint { col: 1, row: 0 });

        let plan = build_render_plan_with_options(
            &terminal.snapshot(),
            &Damage::Full,
            CellMetrics::default(),
            RenderOptions {
                current_search_match: Some(SearchMatch {
                    row: 0,
                    start_col: 0,
                    end_col: 1,
                }),
                ..RenderOptions::default()
            },
        );

        assert!(plan.text.iter().any(|segment| {
            segment.text == "X" && segment.style.fg == Rgba::rgb(205, 214, 244)
        }));
    }

    #[test]
    fn background_pass_builds_gradient_rects() {
        let appearance = RendererAppearance {
            background: BackgroundAppearance {
                kind: BackgroundKind::Gradient(GradientBackground {
                    orientation: GradientOrientation::Vertical,
                    colors: vec![Rgba::rgb(30, 30, 46), Rgba::rgb(17, 17, 27)],
                }),
            },
            ..RendererAppearance::default()
        };

        let rects = background_pass_rects(&appearance, 100, 80);

        assert!(rects.len() >= 2);
        assert_eq!(rects.first().unwrap().color, Rgba::rgb(30, 30, 46));
        assert_eq!(rects.last().unwrap().color, Rgba::rgb(17, 17, 27));
    }

    #[test]
    fn background_image_ready_adds_tint_overlay_rect() {
        let appearance = RendererAppearance {
            background: BackgroundAppearance {
                kind: BackgroundKind::Image(ImageBackground {
                    path: PathBuf::from("wallpapers/knightty.png"),
                    opacity: 0.25,
                    fit: ImageFit::Cover,
                    tint: Rgba::rgb(30, 30, 46),
                    tint_opacity: 0.60,
                    load_state: ImageLoadState::Ready,
                }),
            },
            ..RendererAppearance::default()
        };

        let rects = background_pass_rects(&appearance, 100, 80);

        assert_eq!(rects.len(), 1);
        assert_eq!(rects[0].color, Rgba::with_channels(30, 30, 46, 153));
    }

    #[test]
    fn image_quad_contain_centers_preserved_aspect_ratio() {
        let quad = image_quad(ImageFit::Contain, 200, 100, 100, 100);

        assert_close(quad.x, 0.0);
        assert_close(quad.y, 25.0);
        assert_close(quad.width, 100.0);
        assert_close(quad.height, 50.0);
        assert_close(quad.u0, 0.0);
        assert_close(quad.v0, 0.0);
        assert_close(quad.u1, 1.0);
        assert_close(quad.v1, 1.0);
    }

    #[test]
    fn image_quad_cover_crops_wide_images_to_surface() {
        let quad = image_quad(ImageFit::Cover, 200, 100, 100, 100);

        assert_close(quad.x, 0.0);
        assert_close(quad.y, 0.0);
        assert_close(quad.width, 100.0);
        assert_close(quad.height, 100.0);
        assert_close(quad.u0, 0.25);
        assert_close(quad.u1, 0.75);
        assert_close(quad.v0, 0.0);
        assert_close(quad.v1, 1.0);
    }

    #[test]
    fn image_quad_tile_repeats_uvs_over_surface() {
        let quad = image_quad(ImageFit::Tile, 32, 16, 96, 64);

        assert_close(quad.width, 96.0);
        assert_close(quad.height, 64.0);
        assert_close(quad.u1, 3.0);
        assert_close(quad.v1, 4.0);
    }

    #[test]
    fn image_vertices_apply_opacity_to_each_vertex() {
        let vertices = image_vertices(ImageFit::Stretch, 32, 32, 0.4, 100, 80);

        assert_eq!(vertices.len(), 6);
        assert!(vertices.iter().all(|vertex| vertex.opacity == 0.4));
        assert_close(vertices[0].position[0], -1.0);
        assert_close(vertices[0].position[1], 1.0);
        assert_close(vertices[2].position[0], 1.0);
        assert_close(vertices[2].position[1], -1.0);
    }

    #[test]
    fn inline_image_quad_uses_cell_destination_and_keeps_text_plan() {
        let mut terminal = Terminal::new(10, 4);
        terminal.add_image_placement(ImagePlacement {
            placement_id: ImagePlacementId::new(7),
            image_id: ImageId::new(7),
            kitty_image_id: None,
            anchor: GridPoint { col: 2, row: 1 },
            columns: 3,
            rows: 2,
            source_width: 30,
            source_height: 40,
            source_rect: ImageSourceRect {
                x: 0,
                y: 0,
                width: 30,
                height: 40,
            },
            pixel_offset: ImagePixelOffset::default(),
            z_index: ImageZIndex::default(),
        });
        terminal.feed(b"X");
        let metrics = CellMetrics {
            width: 10,
            height: 20,
            font_size: 16.0,
            line_height: 20.0,
        };

        let plan = build_render_plan(&terminal.snapshot(), &Damage::Full, metrics);

        assert_eq!(
            plan.images,
            vec![ImageQuadPlan {
                image_id: ImageId::new(7),
                destination: PixelRect {
                    x: 20.0,
                    y: 20.0,
                    width: 30.0,
                    height: 40.0,
                },
                uv: UvRect {
                    u0: 0.0,
                    v0: 0.0,
                    u1: 1.0,
                    v1: 1.0,
                },
                z_index: 0,
                client_image_id: 0,
                insertion_order: 0,
            }]
        );
        assert_eq!(plan.text.len(), 1);
    }

    #[test]
    fn inline_image_quad_clips_to_viewport_and_adjusts_uvs() {
        let mut terminal = Terminal::new(10, 4);
        terminal.add_image_placement(ImagePlacement {
            placement_id: ImagePlacementId::new(8),
            image_id: ImageId::new(8),
            kitty_image_id: None,
            anchor: GridPoint { col: 0, row: -1 },
            columns: 2,
            rows: 2,
            source_width: 20,
            source_height: 40,
            source_rect: ImageSourceRect {
                x: 0,
                y: 10,
                width: 20,
                height: 20,
            },
            pixel_offset: ImagePixelOffset::default(),
            z_index: ImageZIndex::default(),
        });
        let metrics = CellMetrics {
            width: 10,
            height: 20,
            font_size: 16.0,
            line_height: 20.0,
        };

        let plan = build_render_plan(&terminal.snapshot(), &Damage::Full, metrics);

        assert_eq!(plan.images[0].destination.y, 0.0);
        assert_eq!(plan.images[0].destination.height, 20.0);
        assert_eq!(plan.images[0].uv.v0, 0.5);
        assert_eq!(plan.images[0].uv.v1, 0.75);
    }

    #[test]
    fn inline_image_crop_offset_and_resize_recalculate_destination_and_uvs() {
        let mut terminal = Terminal::new(10, 4);
        terminal.add_image_placement(ImagePlacement {
            placement_id: ImagePlacementId::new(9),
            image_id: ImageId::new(9),
            kitty_image_id: Some(42),
            anchor: GridPoint { col: 0, row: 0 },
            columns: 2,
            rows: 2,
            source_width: 100,
            source_height: 80,
            source_rect: ImageSourceRect {
                x: 20,
                y: 10,
                width: 40,
                height: 20,
            },
            pixel_offset: ImagePixelOffset { x: 5, y: 10 },
            z_index: ImageZIndex(-1),
        });
        let snapshot = terminal.snapshot();

        let original = build_render_plan(
            &snapshot,
            &Damage::Full,
            CellMetrics {
                width: 10,
                height: 20,
                font_size: 16.0,
                line_height: 20.0,
            },
        );
        assert_eq!(
            original.images[0].destination,
            PixelRect {
                x: 5.0,
                y: 10.0,
                width: 20.0,
                height: 40.0,
            }
        );
        assert_eq!(
            original.images[0].uv,
            UvRect {
                u0: 0.2,
                v0: 0.125,
                u1: 0.6,
                v1: 0.375,
            }
        );

        let resized = build_render_plan(
            &snapshot,
            &Damage::Full,
            CellMetrics {
                width: 4,
                height: 8,
                font_size: 8.0,
                line_height: 8.0,
            },
        );
        assert_eq!(resized.images[0].destination.x, 5.0);
        assert_eq!(resized.images[0].destination.y, 10.0);
        assert_eq!(resized.images[0].destination.width, 8.0);
        assert_eq!(resized.images[0].destination.height, 16.0);
    }

    #[test]
    fn kitty_images_sort_by_z_client_id_and_stable_insertion_order() {
        let mut terminal = Terminal::new(4, 2);
        let placements = [
            (1, 42, 0),
            (2, 7, 0),
            (3, 1, BELOW_CELL_BACKGROUND - 1),
            (4, 1, BELOW_CELL_BACKGROUND),
            (5, 1, -1),
            (6, 1, 1),
            (7, 7, 0),
        ];
        for (id, client_image_id, z_index) in placements {
            terminal.add_image_placement(ImagePlacement {
                placement_id: ImagePlacementId::new(id),
                image_id: ImageId::new(id),
                kitty_image_id: Some(client_image_id),
                anchor: GridPoint { col: 0, row: 0 },
                columns: 1,
                rows: 1,
                source_width: 1,
                source_height: 1,
                source_rect: ImageSourceRect {
                    x: 0,
                    y: 0,
                    width: 1,
                    height: 1,
                },
                pixel_offset: ImagePixelOffset::default(),
                z_index: ImageZIndex(z_index),
            });
        }

        let plan = build_render_plan(
            &terminal.snapshot(),
            &Damage::Full,
            CellMetrics {
                width: 10,
                height: 20,
                font_size: 16.0,
                line_height: 20.0,
            },
        );
        assert_eq!(
            plan.images
                .iter()
                .map(|image| image.image_id)
                .collect::<Vec<_>>(),
            vec![
                ImageId::new(3),
                ImageId::new(4),
                ImageId::new(5),
                ImageId::new(2),
                ImageId::new(7),
                ImageId::new(1),
                ImageId::new(6),
            ]
        );
        assert_eq!(plan.images[3].insertion_order, 1);
        assert_eq!(plan.images[4].insertion_order, 6);
    }

    #[test]
    fn kitty_z_threshold_maps_to_four_render_layers() {
        assert_eq!(
            InlineImageLayer::for_z_index(BELOW_CELL_BACKGROUND - 1),
            InlineImageLayer::BelowCellBackground
        );
        assert_eq!(
            InlineImageLayer::for_z_index(BELOW_CELL_BACKGROUND),
            InlineImageLayer::BelowText
        );
        assert_eq!(
            InlineImageLayer::for_z_index(-1),
            InlineImageLayer::BelowText
        );
        assert_eq!(InlineImageLayer::for_z_index(0), InlineImageLayer::Zero);
        assert_eq!(
            InlineImageLayer::for_z_index(1),
            InlineImageLayer::AboveText
        );
    }

    #[test]
    fn inline_image_cache_changes_reuse_existing_and_evict_removed_textures() {
        let requested = vec![
            InlineImageData {
                id: ImageId::new(2),
                width: 1,
                height: 1,
                rgba: Arc::from([0, 0, 0, 0]),
            },
            InlineImageData {
                id: ImageId::new(3),
                width: 1,
                height: 1,
                rgba: Arc::from([0, 0, 0, 0]),
            },
        ];

        let (remove, upload) =
            inline_image_cache_changes([ImageId::new(1), ImageId::new(2)], &requested);

        assert_eq!(remove, vec![ImageId::new(1)]);
        assert_eq!(
            upload.into_iter().collect::<Vec<_>>(),
            vec![ImageId::new(3)]
        );

        let (remove, upload) = inline_image_cache_changes([ImageId::new(2)], &[]);
        assert_eq!(remove, vec![ImageId::new(2)]);
        assert!(upload.is_empty());

        let (remove, upload) = inline_image_cache_changes([], &requested[..1]);
        assert!(remove.is_empty());
        assert_eq!(
            upload.into_iter().collect::<Vec<_>>(),
            vec![ImageId::new(2)]
        );
    }

    #[test]
    fn scanline_effect_builds_overlay_lines() {
        let mut appearance = RendererAppearance::default();
        appearance.effects.scanlines = true;

        let rects = effect_overlay_rects(&appearance, 10, 6);

        assert_eq!(rects.len(), 3);
        assert!(rects.iter().all(|rect| rect.layer == RectLayer::Overlay));
        assert_eq!(rects[0].y, 1);
        assert_eq!(rects[1].y, 3);
        assert_eq!(rects[2].y, 5);
    }

    #[test]
    fn retro_crt_effect_builds_edge_overlays() {
        let mut appearance = RendererAppearance::default();
        appearance.effects.retro_crt = true;

        let rects = effect_overlay_rects(&appearance, 100, 80);

        assert_eq!(rects.len(), 4);
        assert!(rects.iter().all(|rect| rect.layer == RectLayer::Overlay));
        assert!(rects.iter().any(|rect| rect.y == 0 && rect.height == 5));
        assert!(rects.iter().any(|rect| rect.x == 95 && rect.width == 5));
    }

    #[test]
    fn attrs_use_custom_font_family_and_disable_ligature_features() {
        let attrs = attrs_for_style(
            TextStyle {
                fg: DEFAULT_FG,
                bold: false,
                italic: false,
                underline: false,
                inverse: false,
            },
            Some("CaskaydiaCove Nerd Font"),
        );

        assert_eq!(attrs.family, Family::Name("CaskaydiaCove Nerd Font"));
        assert_disabled_feature(&attrs, FeatureTag::STANDARD_LIGATURES);
        assert_disabled_feature(&attrs, FeatureTag::CONTEXTUAL_LIGATURES);
        assert_disabled_feature(&attrs, FeatureTag::DISCRETIONARY_LIGATURES);
        assert_disabled_feature(&attrs, FeatureTag::CONTEXTUAL_ALTERNATES);
    }

    #[test]
    fn attrs_default_to_monospace_family() {
        let attrs = attrs_for_style(
            TextStyle {
                fg: DEFAULT_FG,
                bold: false,
                italic: false,
                underline: false,
                inverse: false,
            },
            None,
        );

        assert_eq!(attrs.family, Family::Monospace);
    }

    #[test]
    fn render_plan_creates_background_rect_for_truecolor_background() {
        let mut terminal = Terminal::new(4, 1);
        terminal.feed(b"\x1b[48;2;1;2;3mX ");

        let plan = build_render_plan(&terminal.snapshot(), &Damage::Full, CellMetrics::default());

        assert!(plan.rects.iter().any(|rect| {
            rect.layer == RectLayer::Background
                && rect.x == 0
                && rect.y == 0
                && rect.width >= CellMetrics::default().width
                && rect.color == Rgba::rgb(1, 2, 3)
        }));
    }

    #[test]
    fn render_plan_swaps_colors_for_inverse_cells() {
        let mut terminal = Terminal::new(4, 1);
        terminal.feed(b"\x1b[7mX");

        let plan = build_render_plan(&terminal.snapshot(), &Damage::Full, CellMetrics::default());

        assert!(plan.text.iter().any(|segment| {
            segment.text == "X"
                && segment.style
                    == TextStyle {
                        fg: Rgba::rgb(30, 30, 46),
                        bold: false,
                        italic: false,
                        underline: false,
                        inverse: true,
                    }
        }));
        assert!(plan.rects.iter().any(|rect| {
            rect.layer == RectLayer::Background && rect.color == Rgba::rgb(205, 214, 244)
        }));
    }

    #[test]
    fn render_plan_draws_underline_as_overlay_rect() {
        let mut terminal = Terminal::new(4, 1);
        terminal.feed(b"\x1b[4mX");

        let metrics = CellMetrics::default();
        let plan = build_render_plan(&terminal.snapshot(), &Damage::Full, metrics);

        assert!(plan.rects.iter().any(|rect| {
            rect.layer == RectLayer::Overlay
                && rect.x == 0
                && rect.y == metrics.height - 2
                && rect.height == 1
                && rect.color == Rgba::rgb(205, 214, 244)
        }));
    }

    #[test]
    fn render_plan_skips_wide_spacer_cells_in_text() {
        let mut terminal = Terminal::new(4, 1);
        terminal.feed("界A".as_bytes());

        let plan = build_render_plan(&terminal.snapshot(), &Damage::Full, CellMetrics::default());
        let rendered_text = plan
            .text
            .iter()
            .map(|segment| segment.text.as_str())
            .collect::<String>();

        assert_eq!(rendered_text, "界A");
    }

    #[test]
    fn render_plan_places_text_at_cell_coordinates_after_leading_spaces() {
        let mut terminal = Terminal::new(8, 1);
        terminal.feed(b"   X");

        let metrics = CellMetrics::default();
        let plan = build_render_plan(&terminal.snapshot(), &Damage::Full, metrics);
        let x = plan
            .text
            .iter()
            .find(|segment| segment.text == "X")
            .map(|segment| segment.x);

        assert_eq!(x, Some(metrics.width * 3));
    }

    #[test]
    fn render_plan_adds_block_cursor_background_when_visible() {
        let terminal = Terminal::new(4, 1);

        let metrics = CellMetrics::default();
        let plan = build_render_plan(&terminal.snapshot(), &Damage::Full, metrics);

        assert!(plan.rects.iter().any(|rect| {
            rect.layer == RectLayer::Background
                && rect.x == 0
                && rect.y == 0
                && rect.width == metrics.width
                && rect.height == metrics.height
                && rect.color == Rgba::rgb(245, 224, 220)
        }));
    }

    #[test]
    fn render_plan_applies_cursor_text_color_for_block_cursor_cell() {
        let mut terminal = Terminal::new(4, 1);
        terminal.feed(b"X\x1b[1D");

        let plan = build_render_plan(&terminal.snapshot(), &Damage::Full, CellMetrics::default());

        assert!(
            plan.text.iter().any(|segment| {
                segment.text == "X" && segment.style.fg == Rgba::rgb(30, 30, 46)
            })
        );
    }

    #[test]
    fn render_plan_hides_blinking_cursor_when_blink_phase_is_off() {
        let terminal = Terminal::new(4, 1);

        let metrics = CellMetrics::default();
        let plan = build_render_plan_with_options(
            &terminal.snapshot(),
            &Damage::Full,
            metrics,
            RenderOptions {
                cursor_blink_visible: false,
                ..RenderOptions::default()
            },
        );

        assert!(!plan.rects.iter().any(|rect| {
            rect.color == Rgba::rgb(245, 224, 220)
                && rect.x == 0
                && rect.y == 0
                && rect.width == metrics.width
                && rect.height == metrics.height
        }));
    }

    #[test]
    fn render_plan_places_ascii_ime_preedit_at_cursor_without_mutating_grid() {
        let mut terminal = Terminal::new(8, 2);
        terminal.feed(b"\x1b[2;3H");
        let snapshot = terminal.snapshot();

        let metrics = CellMetrics::default();
        let plan = build_render_plan_with_options(
            &snapshot,
            &Damage::Full,
            metrics,
            ime_options("abc", None),
        );

        assert_eq!(snapshot.cell(2, 1).ch, ' ');
        assert!(plan.text.iter().any(|segment| {
            segment.text == "a" && segment.x == metrics.width * 2 && segment.y == metrics.height
        }));
    }

    #[test]
    fn render_plan_places_wide_ime_preedit_at_cursor() {
        let mut terminal = Terminal::new(8, 2);
        terminal.feed(b"\x1b[1;2H");

        let metrics = CellMetrics::default();
        let plan = build_render_plan_with_options(
            &terminal.snapshot(),
            &Damage::Full,
            metrics,
            ime_options("日", None),
        );

        assert!(plan.text.iter().any(|segment| {
            segment.text == "日" && segment.x == metrics.width && segment.width == metrics.width * 2
        }));
    }

    #[test]
    fn render_plan_attaches_combining_ime_marks_without_advancing_cells() {
        let terminal = Terminal::new(8, 1);

        let metrics = CellMetrics::default();
        let plan = build_render_plan_with_options(
            &terminal.snapshot(),
            &Damage::Full,
            metrics,
            ime_options("a\u{0301}日b", None),
        );

        let accent = plan
            .text
            .iter()
            .find(|segment| segment.text == "a\u{0301}")
            .expect("combined accent segment");
        let wide = plan
            .text
            .iter()
            .find(|segment| segment.text == "日")
            .expect("wide segment");
        let trailing = plan
            .text
            .iter()
            .find(|segment| segment.text == "b")
            .expect("trailing segment");

        assert_eq!(accent.x, 0);
        assert_eq!(accent.width, metrics.width);
        assert_eq!(wide.x, metrics.width);
        assert_eq!(wide.width, metrics.width * 2);
        assert_eq!(trailing.x, metrics.width * 3);
    }

    #[test]
    fn render_plan_wraps_and_clips_ime_preedit_at_viewport_edge() {
        let mut terminal = Terminal::new(4, 2);
        terminal.feed(b"\x1b[1;4H");

        let metrics = CellMetrics::default();
        let plan = build_render_plan_with_options(
            &terminal.snapshot(),
            &Damage::Full,
            metrics,
            ime_options("abcdef", None),
        );

        assert!(plan.text.iter().any(|segment| {
            segment.text == "a" && segment.x == metrics.width * 3 && segment.y == 0
        }));
        assert!(plan.text.iter().any(|segment| {
            segment.text == "b" && segment.x == 0 && segment.y == metrics.height
        }));
        assert!(plan.text.iter().any(|segment| {
            segment.text == "e" && segment.x == metrics.width * 3 && segment.y == metrics.height
        }));
        assert!(!plan.text.iter().any(|segment| segment.text == "f"));
    }

    #[test]
    fn render_plan_underlines_full_ime_preedit_occupancy() {
        let mut terminal = Terminal::new(8, 1);
        terminal.feed(b"\x1b[1;2H");

        let metrics = CellMetrics::default();
        let appearance = RendererAppearance::default();
        let plan = build_render_plan_with_appearance(
            &terminal.snapshot(),
            &Damage::Full,
            metrics,
            &appearance,
            &ime_options("日a", None),
        );

        let underline_width: u32 = plan
            .rects
            .iter()
            .filter(|rect| {
                rect.layer == RectLayer::Overlay
                    && rect.y == metrics.height - 2
                    && rect.color == appearance.theme.foreground
            })
            .map(|rect| rect.width)
            .sum();
        assert_eq!(underline_width, metrics.width * 3);
    }

    #[test]
    fn render_plan_highlights_ime_cursor_range() {
        let terminal = Terminal::new(8, 1);

        let metrics = CellMetrics::default();
        let appearance = RendererAppearance::default();
        let plan = build_render_plan_with_appearance(
            &terminal.snapshot(),
            &Damage::Full,
            metrics,
            &appearance,
            &ime_options("abc", Some(1..2)),
        );

        assert!(plan.rects.iter().any(|rect| {
            rect.layer == RectLayer::Background
                && rect.x == metrics.width
                && rect.width == metrics.width
                && rect.color == appearance.theme.selection_background
        }));
        assert!(plan.text.iter().any(|segment| {
            segment.text == "b" && segment.style.fg == appearance.theme.selection_foreground
        }));
    }

    #[test]
    fn render_plan_clamps_ime_cursor_range_to_utf8_boundaries() {
        let terminal = Terminal::new(8, 1);

        let metrics = CellMetrics::default();
        let appearance = RendererAppearance::default();
        let plan = build_render_plan_with_appearance(
            &terminal.snapshot(),
            &Damage::Full,
            metrics,
            &appearance,
            &ime_options("日a", Some(1..2)),
        );

        assert!(plan.rects.iter().any(|rect| {
            rect.layer == RectLayer::Background
                && rect.x == 0
                && rect.width == metrics.width * 2
                && rect.color == appearance.theme.selection_background
        }));
    }

    #[test]
    fn render_plan_includes_selection_rects_from_snapshot() {
        let mut terminal = Terminal::new(8, 1);
        terminal.feed(b"hello");
        terminal.begin_selection(SelectionPoint { col: 1, row: 0 }, SelectionMode::Simple);
        terminal.update_selection(SelectionPoint { col: 3, row: 0 });

        let metrics = CellMetrics::default();
        let plan = build_render_plan(&terminal.snapshot(), &Damage::Full, metrics);

        assert_eq!(
            plan.selection_rects,
            vec![super::RectPlan {
                x: metrics.width,
                y: 0,
                width: metrics.width * 3,
                height: metrics.height,
                color: Rgba::rgb(69, 71, 90),
                layer: RectLayer::Background,
            }]
        );
    }

    #[test]
    fn render_plan_includes_hyperlink_spans_from_snapshot() {
        let mut terminal = Terminal::new(8, 1);
        terminal.feed(b"\x1b]8;id=link;https://example.com\x07abc\x1b]8;;\x07");

        let plan = build_render_plan(&terminal.snapshot(), &Damage::Full, CellMetrics::default());

        assert_eq!(
            plan.hyperlink_spans,
            vec![super::HyperlinkSpan {
                hyperlink_id: 0,
                row: 0,
                start_col: 0,
                end_col: 3,
            }]
        );
    }

    #[test]
    fn render_plan_underlines_hovered_hyperlink_cells() {
        let mut terminal = Terminal::new(8, 1);
        terminal.feed(b"\x1b]8;id=link;https://example.com\x07abc\x1b]8;;\x07");

        let metrics = CellMetrics::default();
        let plan = build_render_plan_with_options(
            &terminal.snapshot(),
            &Damage::Full,
            metrics,
            RenderOptions {
                hovered_hyperlink_id: Some(0),
                ..RenderOptions::default()
            },
        );

        assert!(plan.rects.iter().any(|rect| {
            rect.layer == RectLayer::Overlay
                && rect.x == 0
                && rect.y == metrics.height - 2
                && rect.width == metrics.width * 3
                && rect.height == 1
                && rect.color == Rgba::rgb(205, 214, 244)
        }));
    }

    #[test]
    fn render_plan_hover_underline_coexists_with_selection_rects() {
        let mut terminal = Terminal::new(8, 1);
        terminal.feed(b"\x1b]8;id=link;https://example.com\x07abc\x1b]8;;\x07");
        terminal.begin_selection(SelectionPoint { col: 0, row: 0 }, SelectionMode::Simple);
        terminal.update_selection(SelectionPoint { col: 2, row: 0 });

        let metrics = CellMetrics::default();
        let plan = build_render_plan_with_options(
            &terminal.snapshot(),
            &Damage::Full,
            metrics,
            RenderOptions {
                hovered_hyperlink_id: Some(0),
                ..RenderOptions::default()
            },
        );

        assert!(!plan.selection_rects.is_empty());
        assert!(plan.rects.iter().any(|rect| {
            rect.layer == RectLayer::Overlay
                && rect.y == metrics.height - 2
                && rect.width == metrics.width * 3
        }));
    }

    #[test]
    fn render_plan_ime_preedit_coexists_with_selection_hover_and_cursor() {
        let mut terminal = Terminal::new(8, 1);
        terminal.feed(b"\x1b]8;id=link;https://example.com\x07abc\x1b]8;;\x07");
        terminal.begin_selection(SelectionPoint { col: 0, row: 0 }, SelectionMode::Simple);
        terminal.update_selection(SelectionPoint { col: 2, row: 0 });

        let metrics = CellMetrics::default();
        let plan = build_render_plan_with_options(
            &terminal.snapshot(),
            &Damage::Full,
            metrics,
            RenderOptions {
                hovered_hyperlink_id: Some(0),
                ime_preedit: Some(ImePreedit {
                    text: "x".to_owned(),
                    cursor_range: Some(0..1),
                }),
                ..RenderOptions::default()
            },
        );

        assert!(!plan.selection_rects.is_empty());
        assert!(plan.rects.iter().any(|rect| {
            rect.layer == RectLayer::Overlay
                && rect.y == metrics.height - 2
                && rect.width == metrics.width * 3
        }));
        assert!(plan.rects.iter().any(|rect| {
            rect.layer == RectLayer::Background
                && rect.x == metrics.width * 3
                && rect.width == metrics.width
                && rect.color == RendererAppearance::default().theme.selection_background
        }));
        assert!(
            plan.text
                .iter()
                .any(|segment| { segment.text == "x" && segment.x == metrics.width * 3 })
        );
    }

    #[test]
    fn render_plan_underlines_hovered_hyperlink_in_scrollback_view() {
        let mut terminal = Terminal::with_scrollback(5, 2, 10);
        terminal.feed(b"\x1b]8;id=hist;https://example.com\x07A\x1b]8;;\x07\r\n");
        terminal.feed(b"B\r\nC\r\n");
        terminal.scroll_to_top();

        let metrics = CellMetrics::default();
        let plan = build_render_plan_with_options(
            &terminal.snapshot(),
            &Damage::Full,
            metrics,
            RenderOptions {
                hovered_hyperlink_id: Some(0),
                ..RenderOptions::default()
            },
        );

        assert!(plan.rects.iter().any(|rect| {
            rect.layer == RectLayer::Overlay
                && rect.x == 0
                && rect.y == metrics.height - 2
                && rect.width == metrics.width
                && rect.height == 1
        }));
    }

    #[test]
    fn render_plan_merges_adjacent_cells_and_splits_different_hyperlinks() {
        let mut terminal = Terminal::new(8, 1);
        terminal.feed(b"\x1b]8;id=a;https://a.example\x07AB\x1b]8;id=b;https://b.example\x07C");

        let plan = build_render_plan(&terminal.snapshot(), &Damage::Full, CellMetrics::default());

        assert_eq!(
            plan.hyperlink_spans,
            vec![
                super::HyperlinkSpan {
                    hyperlink_id: 0,
                    row: 0,
                    start_col: 0,
                    end_col: 2,
                },
                super::HyperlinkSpan {
                    hyperlink_id: 1,
                    row: 0,
                    start_col: 2,
                    end_col: 3,
                },
            ]
        );
    }

    #[test]
    fn render_plan_has_no_hyperlink_spans_without_cell_metadata() {
        let mut terminal = Terminal::new(8, 1);
        terminal.feed(b"abc");

        let plan = build_render_plan(&terminal.snapshot(), &Damage::Full, CellMetrics::default());

        assert!(plan.hyperlink_spans.is_empty());
    }

    #[test]
    fn render_plan_selection_rect_uses_cell_metrics() {
        let mut terminal = Terminal::new(8, 2);
        terminal.feed(b"hello\r\nworld");
        terminal.begin_selection(SelectionPoint { col: 2, row: 1 }, SelectionMode::Simple);
        terminal.update_selection(SelectionPoint { col: 4, row: 1 });

        let metrics = CellMetrics {
            width: 11,
            height: 23,
            font_size: 16.0,
            line_height: 23.0,
        };
        let plan = build_render_plan(&terminal.snapshot(), &Damage::Full, metrics);

        assert_eq!(plan.selection_rects[0].x, 22);
        assert_eq!(plan.selection_rects[0].y, 23);
        assert_eq!(plan.selection_rects[0].width, 33);
        assert_eq!(plan.selection_rects[0].height, 23);
    }

    fn assert_disabled_feature(attrs: &glyphon::Attrs<'_>, tag: FeatureTag) {
        assert!(
            attrs
                .font_features
                .features
                .iter()
                .any(|feature| feature.tag == tag && feature.value == 0),
            "feature {tag:?} should be disabled"
        );
    }

    fn assert_close(actual: f32, expected: f32) {
        assert!(
            (actual - expected).abs() < 0.000_001,
            "expected {actual} to be close to {expected}"
        );
    }
}
