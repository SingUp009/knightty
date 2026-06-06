use std::borrow::Cow;
use std::mem;
use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use glyphon::cosmic_text::{FeatureTag, FontFeatures};
use glyphon::{
    Attrs, Buffer, Cache, Family, FontSystem, Metrics, Resolution, Shaping, Style as GlyphStyle,
    SwashCache, TextArea, TextAtlas, TextBounds, TextRenderer, Viewport, Weight, Wrap,
};
use knightty_core::{Cell, Color, Damage, GridSnapshot};
use thiserror::Error;
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

#[derive(Clone, Debug, PartialEq)]
pub struct RendererConfig {
    pub cell_metrics: CellMetrics,
    pub font_family: Option<String>,
}

impl Default for RendererConfig {
    fn default() -> Self {
        Self {
            cell_metrics: CellMetrics::default(),
            font_family: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RenderPlan {
    pub text: Vec<TextSegmentPlan>,
    pub rects: Vec<RectPlan>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TextSegmentPlan {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub text: String,
    pub style: TextStyle,
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
}

pub struct Renderer {
    _instance: Instance,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface_config: SurfaceConfiguration,
    cell_metrics: CellMetrics,
    font_family: Option<String>,
    font_system: FontSystem,
    swash_cache: SwashCache,
    viewport: Viewport,
    atlas: TextAtlas,
    text_renderer: TextRenderer,
    text_buffers: Vec<Buffer>,
    prepared_text: Vec<PreparedTextArea>,
    rect_pipeline: RectPipeline,
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

        let surface_config = SurfaceConfiguration {
            usage: TextureUsages::RENDER_ATTACHMENT,
            format,
            width: width.max(1),
            height: height.max(1),
            present_mode: PresentMode::Fifo,
            alpha_mode: CompositeAlphaMode::Opaque,
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

        Ok(Self {
            _instance: instance,
            surface,
            device,
            queue,
            surface_config,
            cell_metrics,
            font_family: config.font_family,
            font_system,
            swash_cache,
            viewport,
            atlas,
            text_renderer,
            text_buffers: vec![text_buffer],
            prepared_text: Vec::new(),
            rect_pipeline,
        })
    }

    pub fn cell_metrics(&self) -> CellMetrics {
        self.cell_metrics
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

    pub fn render(&mut self, snapshot: &GridSnapshot, damage: &Damage) -> Result<(), RenderError> {
        let plan = build_render_plan(snapshot, damage, self.cell_metrics);
        self.update_text(&plan);

        let background_rects = plan
            .rects
            .iter()
            .copied()
            .filter(|rect| rect.layer == RectLayer::Background)
            .collect::<Vec<_>>();
        let overlay_rects = plan
            .rects
            .iter()
            .copied()
            .filter(|rect| rect.layer == RectLayer::Overlay)
            .collect::<Vec<_>>();
        self.rect_pipeline.prepare(
            &self.device,
            &self.queue,
            self.surface_config.width,
            self.surface_config.height,
            &background_rects,
            &overlay_rects,
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
            scale: 1.0,
            bounds: TextBounds {
                left: 0,
                top: 0,
                right: self.surface_config.width as i32,
                bottom: self.surface_config.height as i32,
            },
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
                        load: LoadOp::Clear(wgpu::Color::BLACK),
                        store: StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });

            self.rect_pipeline
                .draw(&mut pass, PreparedRectLayer::Background);
            self.text_renderer
                .render(&self.atlas, &self.viewport, &mut pass)?;
            self.rect_pipeline
                .draw(&mut pass, PreparedRectLayer::Overlay);
        }

        self.queue.submit(Some(encoder.finish()));
        frame.present();
        self.atlas.trim();

        Ok(())
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
            self.prepared_text.push(PreparedTextArea {
                buffer_index: index,
                left: segment.x as f32,
                top: segment.y as f32,
            });
        }
    }
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
}

pub fn build_render_plan(
    snapshot: &GridSnapshot,
    _damage: &Damage,
    metrics: CellMetrics,
) -> RenderPlan {
    let mut text = Vec::new();
    let mut rects = Vec::new();

    for (row_index, row) in snapshot.lines().enumerate() {
        push_background_rects(row, row_index, metrics, &mut rects);
        push_underline_rects(row, row_index, metrics, &mut rects);
        push_text_segments(row, row_index, metrics, &mut text);
    }

    if snapshot.cursor.visible {
        rects.push(RectPlan {
            x: snapshot.cursor.x as u32 * metrics.width,
            y: snapshot.cursor.y as u32 * metrics.height,
            width: 2,
            height: metrics.height,
            color: DEFAULT_FG,
            layer: RectLayer::Overlay,
        });
    }

    RenderPlan { text, rects }
}

fn push_text_segments(
    row: &[Cell],
    row_index: usize,
    metrics: CellMetrics,
    text: &mut Vec<TextSegmentPlan>,
) {
    for (column, cell) in row.iter().enumerate() {
        if cell.flags.wide_spacer || cell.ch == ' ' {
            continue;
        }

        text.push(TextSegmentPlan {
            x: column as u32 * metrics.width,
            y: row_index as u32 * metrics.height,
            width: if cell.flags.wide {
                metrics.width * 2
            } else {
                metrics.width
            },
            height: metrics.height,
            text: cell.ch.to_string(),
            style: style_for_cell(cell),
        });
    }
}

fn push_background_rects(
    row: &[Cell],
    row_index: usize,
    metrics: CellMetrics,
    rects: &mut Vec<RectPlan>,
) {
    let mut start = None::<(usize, Rgba)>;

    for (column, cell) in row.iter().enumerate() {
        let bg = effective_bg(cell);
        if bg == DEFAULT_BG {
            flush_rect(
                start.take(),
                column,
                row_index,
                metrics,
                RectLayer::Background,
                rects,
            );
            continue;
        }

        match start {
            Some((_, color)) if color == bg => {}
            Some(_) => {
                flush_rect(
                    start.take(),
                    column,
                    row_index,
                    metrics,
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
        metrics,
        RectLayer::Background,
        rects,
    );
}

fn push_underline_rects(
    row: &[Cell],
    row_index: usize,
    metrics: CellMetrics,
    rects: &mut Vec<RectPlan>,
) {
    let mut start = None::<(usize, Rgba)>;

    for (column, cell) in row.iter().enumerate() {
        if cell.flags.wide_spacer || !cell.flags.underline {
            flush_underline_rect(start.take(), column, row_index, metrics, rects);
            continue;
        }

        let fg = effective_fg(cell);
        match start {
            Some((_, color)) if color == fg => {}
            Some(_) => {
                flush_underline_rect(start.take(), column, row_index, metrics, rects);
                start = Some((column, fg));
            }
            None => start = Some((column, fg)),
        }
    }

    flush_underline_rect(start, row.len(), row_index, metrics, rects);
}

fn flush_rect(
    start: Option<(usize, Rgba)>,
    end_column: usize,
    row_index: usize,
    metrics: CellMetrics,
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
        x: start_column as u32 * metrics.width,
        y: row_index as u32 * metrics.height,
        width: (end_column - start_column) as u32 * metrics.width,
        height: metrics.height,
        color,
        layer,
    });
}

fn flush_underline_rect(
    start: Option<(usize, Rgba)>,
    end_column: usize,
    row_index: usize,
    metrics: CellMetrics,
    rects: &mut Vec<RectPlan>,
) {
    let Some((start_column, color)) = start else {
        return;
    };
    if end_column <= start_column {
        return;
    }

    rects.push(RectPlan {
        x: start_column as u32 * metrics.width,
        y: row_index as u32 * metrics.height + metrics.height.saturating_sub(2),
        width: (end_column - start_column) as u32 * metrics.width,
        height: 1,
        color,
        layer: RectLayer::Overlay,
    });
}

fn style_for_cell(cell: &Cell) -> TextStyle {
    TextStyle {
        fg: effective_fg(cell),
        bold: cell.flags.bold,
        italic: cell.flags.italic,
        underline: cell.flags.underline,
        inverse: cell.flags.inverse,
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

fn effective_fg(cell: &Cell) -> Rgba {
    let fg = resolve_color(cell.fg, true);
    let bg = resolve_color(cell.bg, false);
    if cell.flags.inverse { bg } else { fg }
}

fn effective_bg(cell: &Cell) -> Rgba {
    let fg = resolve_color(cell.fg, true);
    let bg = resolve_color(cell.bg, false);
    if cell.flags.inverse { fg } else { bg }
}

fn resolve_color(color: Color, foreground: bool) -> Rgba {
    match color {
        Color::DefaultFg => DEFAULT_FG,
        Color::DefaultBg => DEFAULT_BG,
        Color::Rgb(r, g, b) => Rgba::rgb(r, g, b),
        Color::Indexed(index) => indexed_color(index),
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

fn indexed_color(index: u8) -> Rgba {
    const ANSI: [Rgba; 16] = [
        Rgba::rgb(0, 0, 0),
        Rgba::rgb(205, 49, 49),
        Rgba::rgb(13, 188, 121),
        Rgba::rgb(229, 229, 16),
        Rgba::rgb(36, 114, 200),
        Rgba::rgb(188, 63, 188),
        Rgba::rgb(17, 168, 205),
        Rgba::rgb(229, 229, 229),
        Rgba::rgb(102, 102, 102),
        Rgba::rgb(241, 76, 76),
        Rgba::rgb(35, 209, 139),
        Rgba::rgb(245, 245, 67),
        Rgba::rgb(59, 142, 234),
        Rgba::rgb(214, 112, 214),
        Rgba::rgb(41, 184, 219),
        Rgba::rgb(255, 255, 255),
    ];

    match index {
        0..=15 => ANSI[index as usize],
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

struct RectPipeline {
    pipeline: RenderPipeline,
    background: RectVertexBuffer,
    overlay: RectVertexBuffer,
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
            overlay: RectVertexBuffer::new(device, "knightty overlay rect vertices"),
        }
    }

    fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        surface_width: u32,
        surface_height: u32,
        background_rects: &[RectPlan],
        overlay_rects: &[RectPlan],
    ) {
        self.background.prepare(
            device,
            queue,
            surface_width,
            surface_height,
            background_rects,
        );
        self.overlay
            .prepare(device, queue, surface_width, surface_height, overlay_rects);
    }

    fn draw(&self, pass: &mut RenderPass<'_>, layer: PreparedRectLayer) {
        let buffer = match layer {
            PreparedRectLayer::Background => &self.background,
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
    ) {
        let vertices = rect_vertices(rects, surface_width, surface_height);
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

fn rect_vertices(rects: &[RectPlan], surface_width: u32, surface_height: u32) -> Vec<RectVertex> {
    let mut vertices = Vec::with_capacity(rects.len() * 6);
    for rect in rects {
        let left = to_ndc_x(rect.x, surface_width);
        let right = to_ndc_x(rect.x.saturating_add(rect.width), surface_width);
        let top = to_ndc_y(rect.y, surface_height);
        let bottom = to_ndc_y(rect.y.saturating_add(rect.height), surface_height);
        let color = rect.color.to_f32();

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
        CellMetrics, DEFAULT_BG, DEFAULT_FG, RectLayer, Rgba, TextStyle, attrs_for_style,
        build_render_plan,
    };
    use glyphon::Family;
    use glyphon::cosmic_text::FeatureTag;
    use knightty_core::{Damage, Terminal};

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
    fn cell_metrics_scale_from_font_size_and_line_height() {
        let metrics = CellMetrics::from_font_size(18.0, 22.0);

        assert_eq!(metrics.font_size, 18.0);
        assert_eq!(metrics.line_height, 22.0);
        assert_eq!(metrics.width, 10);
        assert_eq!(metrics.height, 22);
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
                        fg: DEFAULT_BG,
                        bold: false,
                        italic: false,
                        underline: false,
                        inverse: true,
                    }
        }));
        assert!(
            plan.rects
                .iter()
                .any(|rect| rect.layer == RectLayer::Background && rect.color == DEFAULT_FG)
        );
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
                && rect.color == DEFAULT_FG
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
    fn render_plan_adds_cursor_overlay_when_visible() {
        let terminal = Terminal::new(4, 1);

        let metrics = CellMetrics::default();
        let plan = build_render_plan(&terminal.snapshot(), &Damage::Full, metrics);

        assert!(plan.rects.iter().any(|rect| {
            rect.layer == RectLayer::Overlay
                && rect.x == 0
                && rect.y == 0
                && rect.width == 2
                && rect.height == metrics.height
        }));
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
}
