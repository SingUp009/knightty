use std::fs::{self, File};
use std::io::{self, BufWriter};
use std::path::{Path, PathBuf};

use roxmltree::{Document, Node};
use thiserror::Error;
use tiny_skia::{FillRule, Paint, Path as SkiaPath, PathBuilder, Pixmap, Rect, Transform};

const SOURCE_WIDTH: u16 = 320;
const SOURCE_HEIGHT: u16 = 180;
const WIDTH: u16 = 160;
const HEIGHT: u16 = 90;
const KFA_MAGIC: &[u8; 4] = b"KFA2";
const FIXED_SCALE: f32 = 16.0;

const SOURCE_FRAMES: &[(&str, &str)] = &[
    ("idle_a", "knight_idle_a.svg"),
    ("idle_b", "knight_idle_b.svg"),
    ("enter_a", "knight_enter_a.svg"),
    ("enter_b", "knight_enter_b.svg"),
    ("enter_c", "knight_enter_c.svg"),
    ("reach", "knight_reach.svg"),
    ("draw_a", "knight_draw_a.svg"),
    ("draw_b", "knight_draw_b.svg"),
    ("anticipation", "knight_anticipation.svg"),
    ("slash_smear", "knight_slash_smear.svg"),
    ("follow_a", "knight_follow_a.svg"),
    ("follow_b", "knight_follow_b.svg"),
    ("logo", "knightty_logo.svg"),
];

const CAPE_POSES: &[(&str, &str)] = &[
    ("cape_idle_a", "cape_idle_a.svg"),
    ("cape_idle_b", "cape_idle_b.svg"),
    ("cape_pull_back", "cape_pull_back.svg"),
    ("cape_anticipation", "cape_anticipation.svg"),
    ("cape_slash_hold", "cape_slash_hold.svg"),
    ("cape_whip_forward", "cape_whip_forward.svg"),
    ("cape_overshoot", "cape_overshoot.svg"),
    ("cape_rebound", "cape_rebound.svg"),
    ("cape_settle_a", "cape_settle_a.svg"),
    ("cape_settle_b", "cape_settle_b.svg"),
];

const CAPE_LAYER_ORDER: &[CapeLayerId] = &[
    CapeLayerId::CapeFar,
    CapeLayerId::CapeMain,
    CapeLayerId::CapeNear,
    CapeLayerId::CapeLower,
    CapeLayerId::RibbonFar,
    CapeLayerId::RibbonNear,
];

pub fn run(workspace_root: &Path, action: &str) -> Result<(), DemoAssetError> {
    match action {
        "build" => build(workspace_root),
        "preview" => preview(workspace_root),
        _ => Err(DemoAssetError::UnknownAction(action.to_owned())),
    }
}

fn build(workspace_root: &Path) -> Result<(), DemoAssetError> {
    let bundle = load_asset_bundle(workspace_root)?;
    let generated_dir = workspace_root.join("crates/demo/assets/generated");
    fs::create_dir_all(&generated_dir)?;
    let output_path = generated_dir.join("animation.kfa");
    fs::write(&output_path, encode_kfa(&bundle)?)?;
    println!("generated {}", output_path.display());
    Ok(())
}

fn preview(workspace_root: &Path) -> Result<(), DemoAssetError> {
    let bundle = load_asset_bundle(workspace_root)?;
    let output_dir = workspace_root.join("target/knightty-demo-preview");
    fs::create_dir_all(&output_dir)?;

    let states = [
        ("00-distant-shot.png", "idle_a", "cape_idle_a"),
        ("01-knight-closeup.png", "idle_b", "cape_idle_b"),
        ("02-anticipation.png", "anticipation", "cape_anticipation"),
        ("03-slash.png", "slash_smear", "cape_whip_forward"),
        ("04-logo.png", "logo", "cape_settle_a"),
    ];

    let mut rendered = Vec::new();
    for (file_name, frame_name, cape_name) in states {
        let rgba = preview_rgba(&bundle, frame_name, cape_name, true, true)?;
        write_png(
            &output_dir.join(file_name),
            WIDTH as u32,
            HEIGHT as u32,
            &rgba,
        )?;
        rendered.push(rgba);
    }
    write_png(
        &output_dir.join("contact-sheet.png"),
        (WIDTH as usize * rendered.len()) as u32,
        (HEIGHT as usize * 2) as u32,
        &contact_sheet(&rendered),
    )?;

    let cape_frames = CAPE_POSES
        .iter()
        .map(|(name, _)| preview_rgba(&bundle, "idle_a", name, false, true))
        .collect::<Result<Vec<_>, _>>()?;
    write_png(
        &output_dir.join("cape-contact-sheet.png"),
        (WIDTH as usize * cape_frames.len()) as u32,
        (HEIGHT as usize * 2) as u32,
        &contact_sheet(&cape_frames),
    )?;
    write_png(
        &output_dir.join("cape-motion-strip.png"),
        (WIDTH as usize * cape_frames.len()) as u32,
        HEIGHT as u32,
        &single_row_sheet(&cape_frames),
    )?;

    let character_frames = SOURCE_FRAMES
        .iter()
        .filter(|(name, _)| *name != "logo")
        .map(|(name, _)| preview_rgba(&bundle, name, "cape_idle_a", true, false))
        .collect::<Result<Vec<_>, _>>()?;
    write_png(
        &output_dir.join("character-contact-sheet.png"),
        (WIDTH as usize * character_frames.len()) as u32,
        (HEIGHT as usize * 2) as u32,
        &contact_sheet(&character_frames),
    )?;

    let terminal = preview_rgba(&bundle, "follow_b", "cape_settle_a", true, true)?;
    let source = scale_nearest(&terminal, WIDTH as usize, HEIGHT as usize, 2);
    write_png(
        &output_dir.join("source-320x180.png"),
        SOURCE_WIDTH as u32,
        SOURCE_HEIGHT as u32,
        &source,
    )?;
    write_png(
        &output_dir.join("logical-160x90.png"),
        WIDTH as u32,
        HEIGHT as u32,
        &terminal,
    )?;
    let small = downscale_half(&terminal, WIDTH as usize, HEIGHT as usize);
    write_png(
        &output_dir.join("small-80x45.png"),
        (WIDTH / 2) as u32,
        (HEIGHT / 2) as u32,
        &small,
    )?;
    let half_block = scale_nearest(&small, WIDTH as usize / 2, HEIGHT as usize / 2, 2);
    write_png(
        &output_dir.join("half-block-preview.png"),
        WIDTH as u32,
        (HEIGHT / 2 * 2) as u32,
        &half_block,
    )?;
    write_png(
        &output_dir.join("terminal-contact-sheet.png"),
        (WIDTH as usize * rendered.len()) as u32,
        (HEIGHT as usize * 2) as u32,
        &contact_sheet(&rendered),
    )?;

    println!("generated {}", output_dir.display());
    Ok(())
}

fn load_asset_bundle(workspace_root: &Path) -> Result<AssetBundle, DemoAssetError> {
    let source_dir = workspace_root.join("crates/demo/assets/source");
    let mut frames = Vec::with_capacity(SOURCE_FRAMES.len());
    for (name, file_name) in SOURCE_FRAMES {
        frames.push(load_svg_frame(name, &source_dir.join(file_name))?);
    }

    let mut cape_poses = Vec::with_capacity(CAPE_POSES.len());
    for (name, file_name) in CAPE_POSES {
        cape_poses.push(load_cape_pose(name, &source_dir.join(file_name))?);
    }
    let cape_layers = validate_cape_topology(&cape_poses)?;
    Ok(AssetBundle {
        frames,
        cape_layers,
        cape_poses,
    })
}

fn load_svg_frame(name: &str, path: &Path) -> Result<SourceFrame, DemoAssetError> {
    let source = fs::read_to_string(path)?;
    let document = Document::parse(&source)
        .map_err(|error| DemoAssetError::SvgParse(path.to_path_buf(), error.to_string()))?;
    validate_svg(path, &document)?;

    let mut pixmap = Pixmap::new(WIDTH as u32, HEIGHT as u32).ok_or(DemoAssetError::Pixmap)?;
    for node in document.descendants().filter(Node::is_element) {
        match node.tag_name().name() {
            "svg" | "g" => {}
            "polygon" => draw_polygon(path, &mut pixmap, node)?,
            "rect" => draw_rect(path, &mut pixmap, node)?,
            "circle" => draw_circle(path, &mut pixmap, node)?,
            tag => {
                return Err(DemoAssetError::UnsupportedElement(
                    path.to_path_buf(),
                    tag.into(),
                ));
            }
        }
    }

    Ok(SourceFrame {
        name: name.to_owned(),
        pixels: quantize_pixmap(&pixmap),
    })
}

fn load_cape_pose(name: &str, path: &Path) -> Result<CapePoseSource, DemoAssetError> {
    let source = fs::read_to_string(path)?;
    let document = Document::parse(&source)
        .map_err(|error| DemoAssetError::SvgParse(path.to_path_buf(), error.to_string()))?;
    validate_svg(path, &document)?;

    let mut layers = Vec::new();
    for node in document.descendants().filter(Node::is_element) {
        match node.tag_name().name() {
            "svg" | "g" => {}
            "polygon" => {
                let id = node
                    .attribute("id")
                    .ok_or_else(|| DemoAssetError::MissingAttribute(path.to_path_buf(), "id"))?;
                let id = CapeLayerId::from_str(id).ok_or_else(|| {
                    DemoAssetError::InvalidCapeTopology(format!(
                        "{} has unknown cape layer `{id}`",
                        path.display()
                    ))
                })?;
                let fill = node_fill(path, node)?.ok_or_else(|| {
                    DemoAssetError::InvalidCapeTopology(format!(
                        "{} cape layers cannot use transparent fill",
                        path.display()
                    ))
                })?;
                let points = node.attribute("points").ok_or_else(|| {
                    DemoAssetError::MissingAttribute(path.to_path_buf(), "points")
                })?;
                let points = parse_points(path, points)?;
                if points.len() < 3 {
                    return Err(DemoAssetError::InvalidNumber(
                        path.to_path_buf(),
                        points.len().to_string(),
                    ));
                }
                let vertices = points
                    .into_iter()
                    .map(|(x, y)| {
                        if !(0.0..=f32::from(SOURCE_WIDTH)).contains(&x)
                            || !(0.0..=f32::from(SOURCE_HEIGHT)).contains(&y)
                        {
                            return Err(DemoAssetError::InvalidCapeTopology(format!(
                                "{} has out-of-bounds cape vertex",
                                path.display()
                            )));
                        }
                        Ok(Point {
                            x: x * f32::from(WIDTH) / f32::from(SOURCE_WIDTH),
                            y: y * f32::from(HEIGHT) / f32::from(SOURCE_HEIGHT),
                        })
                    })
                    .collect::<Result<Vec<_>, DemoAssetError>>()?;
                layers.push(CapeLayerSource {
                    id,
                    color: fill,
                    vertices,
                });
            }
            tag => {
                return Err(DemoAssetError::UnsupportedElement(
                    path.to_path_buf(),
                    tag.into(),
                ));
            }
        }
    }

    let mut ordered = Vec::with_capacity(CAPE_LAYER_ORDER.len());
    for id in CAPE_LAYER_ORDER {
        let matches = layers.iter().filter(|layer| layer.id == *id).count();
        if matches != 1 {
            return Err(DemoAssetError::InvalidCapeTopology(format!(
                "{} must contain exactly one `{}` layer",
                path.display(),
                id.as_str()
            )));
        }
        let layer = layers
            .iter()
            .find(|layer| layer.id == *id)
            .expect("layer count was checked");
        ordered.push(layer.clone());
    }

    Ok(CapePoseSource {
        name: name.to_owned(),
        layers: ordered,
    })
}

fn validate_cape_topology(
    poses: &[CapePoseSource],
) -> Result<Vec<CapeLayerDescriptor>, DemoAssetError> {
    let Some(first) = poses.first() else {
        return Err(DemoAssetError::InvalidCapeTopology(
            "at least one cape pose is required".to_owned(),
        ));
    };
    let mut descriptors = Vec::with_capacity(CAPE_LAYER_ORDER.len());
    for (index, first_layer) in first.layers.iter().enumerate() {
        let vertex_count = first_layer.vertices.len();
        let winding = polygon_area(&first_layer.vertices).signum();
        if winding == 0.0 {
            return Err(DemoAssetError::InvalidCapeTopology(format!(
                "{} has degenerate `{}` layer",
                first.name,
                first_layer.id.as_str()
            )));
        }
        let anchor = first_layer.vertices[0];
        for pose in poses {
            let layer = &pose.layers[index];
            if layer.id != first_layer.id
                || layer.color != first_layer.color
                || layer.vertices.len() != vertex_count
            {
                return Err(DemoAssetError::InvalidCapeTopology(format!(
                    "{} does not match `{}` topology",
                    pose.name,
                    first_layer.id.as_str()
                )));
            }
            if polygon_area(&layer.vertices).signum() != winding {
                return Err(DemoAssetError::InvalidCapeTopology(format!(
                    "{} flips `{}` winding",
                    pose.name,
                    first_layer.id.as_str()
                )));
            }
            let dx = layer.vertices[0].x - anchor.x;
            let dy = layer.vertices[0].y - anchor.y;
            if dx.hypot(dy) > 4.0 {
                return Err(DemoAssetError::InvalidCapeTopology(format!(
                    "{} moves `{}` anchor too far",
                    pose.name,
                    first_layer.id.as_str()
                )));
            }
        }
        descriptors.push(CapeLayerDescriptor {
            id: first_layer.id,
            color: first_layer.color,
            vertex_count,
        });
    }
    Ok(descriptors)
}

fn validate_svg(path: &Path, document: &Document<'_>) -> Result<(), DemoAssetError> {
    let root = document.root_element();
    if root.tag_name().name() != "svg" {
        return Err(DemoAssetError::InvalidSvg(
            path.to_path_buf(),
            "missing svg root",
        ));
    }
    if root.attribute("viewBox") != Some("0 0 320 180") {
        return Err(DemoAssetError::InvalidSvg(
            path.to_path_buf(),
            "viewBox must be exactly `0 0 320 180`",
        ));
    }

    for node in document.descendants().filter(Node::is_element) {
        let tag = node.tag_name().name();
        if matches!(
            tag,
            "text" | "image" | "filter" | "mask" | "clipPath" | "linearGradient" | "radialGradient"
        ) {
            return Err(DemoAssetError::UnsupportedElement(
                path.to_path_buf(),
                tag.into(),
            ));
        }
        for attr in node.attributes() {
            let name = attr.name();
            let value = attr.value();
            if name.starts_with("xmlns") {
                continue;
            }
            if matches!(name, "href" | "xlink:href" | "src")
                || value.contains("url(")
                || value.contains("://")
            {
                return Err(DemoAssetError::ExternalReference(path.to_path_buf()));
            }
            if name == "style" || name == "opacity" || name == "fill-opacity" {
                return Err(DemoAssetError::InvalidSvg(
                    path.to_path_buf(),
                    "style and opacity attributes are not supported",
                ));
            }
            if name == "fill" {
                let _ = parse_fill(path, value)?;
            }
        }
    }
    Ok(())
}

fn draw_polygon(
    path: &Path,
    pixmap: &mut Pixmap,
    node: Node<'_, '_>,
) -> Result<(), DemoAssetError> {
    let Some(color) = node_fill(path, node)? else {
        return Ok(());
    };
    let points = node
        .attribute("points")
        .ok_or_else(|| DemoAssetError::MissingAttribute(path.to_path_buf(), "points"))?;
    let points = parse_points(path, points)?;
    if points.len() < 3 {
        return Err(DemoAssetError::InvalidNumber(
            path.to_path_buf(),
            points.len().to_string(),
        ));
    }

    let mut builder = PathBuilder::new();
    builder.move_to(points[0].0, points[0].1);
    for point in &points[1..] {
        builder.line_to(point.0, point.1);
    }
    builder.close();
    let Some(path) = builder.finish() else {
        return Ok(());
    };
    fill_path(pixmap, &path, color);
    Ok(())
}

fn draw_rect(path: &Path, pixmap: &mut Pixmap, node: Node<'_, '_>) -> Result<(), DemoAssetError> {
    let Some(color) = node_fill(path, node)? else {
        return Ok(());
    };
    let x = parse_attr_f32(path, node, "x")?;
    let y = parse_attr_f32(path, node, "y")?;
    let width = parse_attr_f32(path, node, "width")?;
    let height = parse_attr_f32(path, node, "height")?;
    let rect = Rect::from_xywh(x, y, width, height)
        .ok_or_else(|| DemoAssetError::InvalidNumber(path.to_path_buf(), "rect".to_owned()))?;
    let path = PathBuilder::from_rect(rect);
    fill_path(pixmap, &path, color);
    Ok(())
}

fn draw_circle(path: &Path, pixmap: &mut Pixmap, node: Node<'_, '_>) -> Result<(), DemoAssetError> {
    let Some(color) = node_fill(path, node)? else {
        return Ok(());
    };
    let cx = parse_attr_f32(path, node, "cx")?;
    let cy = parse_attr_f32(path, node, "cy")?;
    let radius = parse_attr_f32(path, node, "r")?;
    if radius <= 0.0 {
        return Ok(());
    }
    let mut builder = PathBuilder::new();
    builder.push_circle(cx, cy, radius);
    let Some(path) = builder.finish() else {
        return Ok(());
    };
    fill_path(pixmap, &path, color);
    Ok(())
}

fn fill_path(pixmap: &mut Pixmap, path: &SkiaPath, color: AssetColor) {
    let mut paint = Paint::default();
    let (r, g, b) = color.rgb();
    paint.set_color_rgba8(r, g, b, 255);
    pixmap.fill_path(
        path,
        &paint,
        FillRule::Winding,
        Transform::from_scale(
            f32::from(WIDTH) / f32::from(SOURCE_WIDTH),
            f32::from(HEIGHT) / f32::from(SOURCE_HEIGHT),
        ),
        None,
    );
}

fn node_fill(path: &Path, node: Node<'_, '_>) -> Result<Option<AssetColor>, DemoAssetError> {
    let fill = node
        .attribute("fill")
        .ok_or_else(|| DemoAssetError::MissingAttribute(path.to_path_buf(), "fill"))?;
    parse_fill(path, fill)
}

fn parse_fill(path: &Path, fill: &str) -> Result<Option<AssetColor>, DemoAssetError> {
    match fill {
        "none" | "transparent" => Ok(None),
        "#CDD6F4" | "#cdd6f4" => Ok(Some(AssetColor::Foreground)),
        "#B4BEFE" | "#b4befe" => Ok(Some(AssetColor::Accent)),
        "#6C7086" | "#6c7086" => Ok(Some(AssetColor::MidTone)),
        "#11111B" | "#11111b" => Ok(None),
        _ => Err(DemoAssetError::UnknownColor(
            path.to_path_buf(),
            fill.to_owned(),
        )),
    }
}

fn parse_points(path: &Path, points: &str) -> Result<Vec<(f32, f32)>, DemoAssetError> {
    let normalized = points.replace(',', " ");
    let mut numbers = Vec::new();
    for value in normalized.split_whitespace() {
        numbers.push(parse_f32(path, value)?);
    }
    if numbers.len() % 2 != 0 {
        return Err(DemoAssetError::InvalidNumber(
            path.to_path_buf(),
            points.to_owned(),
        ));
    }
    Ok(numbers
        .chunks_exact(2)
        .map(|pair| (pair[0], pair[1]))
        .collect())
}

fn parse_attr_f32(
    path: &Path,
    node: Node<'_, '_>,
    attribute: &'static str,
) -> Result<f32, DemoAssetError> {
    let value = node
        .attribute(attribute)
        .ok_or_else(|| DemoAssetError::MissingAttribute(path.to_path_buf(), attribute))?;
    parse_f32(path, value)
}

fn parse_f32(path: &Path, value: &str) -> Result<f32, DemoAssetError> {
    let number = value
        .parse::<f32>()
        .map_err(|_| DemoAssetError::InvalidNumber(path.to_path_buf(), value.to_owned()))?;
    if !number.is_finite() {
        return Err(DemoAssetError::InvalidNumber(
            path.to_path_buf(),
            value.to_owned(),
        ));
    }
    Ok(number)
}

fn quantize_pixmap(pixmap: &Pixmap) -> Vec<AssetColor> {
    pixmap
        .pixels()
        .iter()
        .map(|pixel| {
            if pixel.alpha() < 64 {
                AssetColor::Transparent
            } else {
                AssetColor::nearest(pixel.red(), pixel.green(), pixel.blue())
            }
        })
        .collect()
}

fn encode_kfa(bundle: &AssetBundle) -> Result<Vec<u8>, DemoAssetError> {
    let encoded_frames: Vec<_> = bundle
        .frames
        .iter()
        .map(|frame| encode_rle(&frame.pixels))
        .collect();
    let mut out = Vec::new();
    out.extend_from_slice(KFA_MAGIC);
    out.extend_from_slice(&WIDTH.to_le_bytes());
    out.extend_from_slice(&HEIGHT.to_le_bytes());
    out.extend_from_slice(&(bundle.frames.len() as u16).to_le_bytes());
    out.push(bundle.cape_layers.len() as u8);
    out.extend_from_slice(&(bundle.cape_poses.len() as u16).to_le_bytes());

    for (frame, encoded) in bundle.frames.iter().zip(&encoded_frames) {
        write_name(&mut out, &frame.name)?;
        out.extend_from_slice(&(encoded.len() as u32).to_le_bytes());
    }

    for layer in &bundle.cape_layers {
        out.push(layer.id as u8);
        out.push(layer.color as u8);
        out.extend_from_slice(&(layer.vertex_count as u16).to_le_bytes());
    }

    for pose in &bundle.cape_poses {
        write_name(&mut out, &pose.name)?;
    }

    for encoded in encoded_frames {
        out.extend_from_slice(&encoded);
    }

    for pose in &bundle.cape_poses {
        for layer in &pose.layers {
            for point in &layer.vertices {
                let x = (point.x * FIXED_SCALE).round() as u16;
                let y = (point.y * FIXED_SCALE).round() as u16;
                out.extend_from_slice(&x.to_le_bytes());
                out.extend_from_slice(&y.to_le_bytes());
            }
        }
    }
    Ok(out)
}

fn write_name(out: &mut Vec<u8>, name: &str) -> Result<(), DemoAssetError> {
    if name.len() > u8::MAX as usize {
        return Err(DemoAssetError::FrameNameTooLong(name.to_owned()));
    }
    out.push(name.len() as u8);
    out.extend_from_slice(name.as_bytes());
    Ok(())
}

fn encode_rle(pixels: &[AssetColor]) -> Vec<u8> {
    let mut out = Vec::new();
    if pixels.is_empty() {
        return out;
    }
    let mut current = pixels[0];
    let mut count = 0_u16;
    for pixel in pixels {
        if *pixel == current && count < u16::MAX {
            count += 1;
            continue;
        }
        out.push(current as u8);
        out.extend_from_slice(&count.to_le_bytes());
        current = *pixel;
        count = 1;
    }
    out.push(current as u8);
    out.extend_from_slice(&count.to_le_bytes());
    out
}

fn preview_rgba(
    bundle: &AssetBundle,
    frame_name: &str,
    cape_name: &str,
    include_body: bool,
    include_cape: bool,
) -> Result<Vec<u8>, DemoAssetError> {
    let mut rgba = vec![0; WIDTH as usize * HEIGHT as usize * 4];
    fill_background(&mut rgba);
    draw_preview_background(&mut rgba, frame_name);
    if include_cape {
        let pose = bundle
            .cape_poses
            .iter()
            .find(|pose| pose.name == cape_name)
            .ok_or_else(|| DemoAssetError::MissingFrame(cape_name.to_owned()))?;
        composite_cape_layers(&mut rgba, pose);
    }
    if include_body {
        let frame = bundle
            .frames
            .iter()
            .find(|frame| frame.name == frame_name)
            .ok_or_else(|| DemoAssetError::MissingFrame(frame_name.to_owned()))?;
        composite_frame(&mut rgba, frame);
    }
    Ok(rgba)
}

fn fill_background(rgba: &mut [u8]) {
    for pixel in rgba.chunks_exact_mut(4) {
        pixel.copy_from_slice(&[0x11, 0x11, 0x1b, 0xff]);
    }
}

fn draw_preview_background(rgba: &mut [u8], file_name: &str) {
    draw_circle_rgba(rgba, 124.0, 21.0, 10.5, [0xcd, 0xd6, 0xf4, 0xff]);
    draw_circle_rgba(rgba, 128.0, 18.0, 5.0, [0x11, 0x11, 0x1b, 0xff]);
    if file_name.contains("slash") || file_name.contains("logo") {
        draw_line_rgba(rgba, 31.0, 68.0, 139.0, 17.0, 2.0, [0xb4, 0xbe, 0xfe, 0xff]);
    }
    for index in 0..32_u32 {
        let x = 4 + ((index.wrapping_mul(53) % 148) as i32);
        let y = 3 + ((index.wrapping_mul(29) % 80) as i32);
        set_rgba(rgba, x, y, [0x6c, 0x70, 0x86, 0xff]);
    }
}

fn composite_cape_layers(rgba: &mut [u8], pose: &CapePoseSource) {
    let order = [
        CapeLayerId::CapeFar,
        CapeLayerId::RibbonFar,
        CapeLayerId::CapeMain,
        CapeLayerId::CapeLower,
        CapeLayerId::CapeNear,
        CapeLayerId::RibbonNear,
    ];
    for id in order {
        if let Some(layer) = pose.layers.iter().find(|layer| layer.id == id) {
            draw_polygon_rgba(rgba, &layer.vertices, layer.color.rgba());
        }
    }
}

fn composite_frame(rgba: &mut [u8], frame: &SourceFrame) {
    for y in 0..HEIGHT as usize {
        for x in 0..WIDTH as usize {
            let color = frame.pixels[y * WIDTH as usize + x];
            if color == AssetColor::Transparent {
                continue;
            }
            let offset = (y * WIDTH as usize + x) * 4;
            let (r, g, b) = color.rgb();
            rgba[offset..offset + 4].copy_from_slice(&[r, g, b, 0xff]);
        }
    }
}

fn contact_sheet(frames: &[Vec<u8>]) -> Vec<u8> {
    let sheet_width = WIDTH as usize * frames.len();
    let sheet_height = HEIGHT as usize * 2;
    let mut sheet = vec![0; sheet_width * sheet_height * 4];
    for pixel in sheet.chunks_exact_mut(4) {
        pixel.copy_from_slice(&[0x11, 0x11, 0x1b, 0xff]);
    }

    for (frame_index, frame) in frames.iter().enumerate() {
        let origin_x = frame_index * WIDTH as usize;
        blit(
            &mut sheet,
            sheet_width,
            origin_x,
            0,
            frame,
            WIDTH as usize,
            HEIGHT as usize,
        );
        let small = downscale_half(frame, WIDTH as usize, HEIGHT as usize);
        let small_width = WIDTH as usize / 2;
        let small_height = HEIGHT as usize / 2;
        let scaled = scale_nearest(&small, small_width, small_height, 2);
        blit(
            &mut sheet,
            sheet_width,
            origin_x,
            HEIGHT as usize,
            &scaled,
            WIDTH as usize,
            small_height * 2,
        );
    }
    sheet
}

fn single_row_sheet(frames: &[Vec<u8>]) -> Vec<u8> {
    let sheet_width = WIDTH as usize * frames.len();
    let sheet_height = HEIGHT as usize;
    let mut sheet = vec![0; sheet_width * sheet_height * 4];
    for (frame_index, frame) in frames.iter().enumerate() {
        blit(
            &mut sheet,
            sheet_width,
            frame_index * WIDTH as usize,
            0,
            frame,
            WIDTH as usize,
            HEIGHT as usize,
        );
    }
    sheet
}

fn blit(
    target: &mut [u8],
    target_width: usize,
    origin_x: usize,
    origin_y: usize,
    source: &[u8],
    source_width: usize,
    source_height: usize,
) {
    for y in 0..source_height {
        let target_start = ((origin_y + y) * target_width + origin_x) * 4;
        let source_start = y * source_width * 4;
        let len = source_width * 4;
        target[target_start..target_start + len]
            .copy_from_slice(&source[source_start..source_start + len]);
    }
}

fn downscale_half(source: &[u8], width: usize, height: usize) -> Vec<u8> {
    let out_width = width / 2;
    let out_height = height / 2;
    let mut out = vec![0; out_width * out_height * 4];
    for y in 0..out_height {
        for x in 0..out_width {
            let mut counts = [0_u32; 4];
            for dy in 0..2 {
                for dx in 0..2 {
                    let offset = (((y * 2 + dy) * width) + (x * 2 + dx)) * 4;
                    let rgba = &source[offset..offset + 4];
                    let bucket = match [rgba[0], rgba[1], rgba[2]] {
                        [0xcd, 0xd6, 0xf4] => 1,
                        [0xb4, 0xbe, 0xfe] => 2,
                        [0x6c, 0x70, 0x86] => 3,
                        _ => 0,
                    };
                    counts[bucket] += 1;
                }
            }
            let bucket = counts
                .iter()
                .enumerate()
                .max_by_key(|(_, value)| *value)
                .map(|(index, _)| index)
                .unwrap_or(0);
            let color = match bucket {
                1 => [0xcd, 0xd6, 0xf4, 0xff],
                2 => [0xb4, 0xbe, 0xfe, 0xff],
                3 => [0x6c, 0x70, 0x86, 0xff],
                _ => [0x11, 0x11, 0x1b, 0xff],
            };
            let offset = (y * out_width + x) * 4;
            out[offset..offset + 4].copy_from_slice(&color);
        }
    }
    out
}

fn scale_nearest(source: &[u8], width: usize, height: usize, scale: usize) -> Vec<u8> {
    let out_width = width * scale;
    let out_height = height * scale;
    let mut out = vec![0; out_width * out_height * 4];
    for y in 0..out_height {
        for x in 0..out_width {
            let source_x = x / scale;
            let source_y = y / scale;
            let source_offset = (source_y * width + source_x) * 4;
            let out_offset = (y * out_width + x) * 4;
            out[out_offset..out_offset + 4]
                .copy_from_slice(&source[source_offset..source_offset + 4]);
        }
    }
    out
}

fn write_png(path: &Path, width: u32, height: u32, rgba: &[u8]) -> Result<(), DemoAssetError> {
    let file = File::create(path)?;
    let writer = BufWriter::new(file);
    let mut encoder = png::Encoder::new(writer, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header()?;
    writer.write_image_data(rgba)?;
    Ok(())
}

fn draw_polygon_rgba(rgba: &mut [u8], points: &[Point], color: [u8; 4]) {
    if points.len() < 3 {
        return;
    }
    let min_y = points
        .iter()
        .map(|point| point.y)
        .fold(f32::INFINITY, f32::min)
        .floor()
        .max(0.0) as i32;
    let max_y = points
        .iter()
        .map(|point| point.y)
        .fold(f32::NEG_INFINITY, f32::max)
        .ceil()
        .min(f32::from(HEIGHT - 1)) as i32;
    let mut intersections = Vec::with_capacity(points.len());
    for y in min_y..=max_y {
        let scan_y = y as f32 + 0.5;
        intersections.clear();
        for index in 0..points.len() {
            let a = points[index];
            let b = points[(index + 1) % points.len()];
            let low_y = a.y.min(b.y);
            let high_y = a.y.max(b.y);
            if scan_y < low_y || scan_y >= high_y || (a.y - b.y).abs() <= f32::EPSILON {
                continue;
            }
            let t = (scan_y - a.y) / (b.y - a.y);
            intersections.push(a.x + t * (b.x - a.x));
        }
        intersections.sort_by(f32::total_cmp);
        for pair in intersections.chunks_exact(2) {
            let x0 = pair[0].ceil().max(0.0) as i32;
            let x1 = pair[1].floor().min(f32::from(WIDTH - 1)) as i32;
            for x in x0..=x1 {
                set_rgba(rgba, x, y, color);
            }
        }
    }
}

fn draw_circle_rgba(rgba: &mut [u8], cx: f32, cy: f32, radius: f32, color: [u8; 4]) {
    let radius_sq = radius * radius;
    for y in (cy - radius).floor() as i32..=(cy + radius).ceil() as i32 {
        for x in (cx - radius).floor() as i32..=(cx + radius).ceil() as i32 {
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            if dx * dx + dy * dy <= radius_sq {
                set_rgba(rgba, x, y, color);
            }
        }
    }
}

fn draw_line_rgba(
    rgba: &mut [u8],
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    thickness: f32,
    color: [u8; 4],
) {
    let steps = ((x1 - x0).abs().max((y1 - y0).abs()) * 4.0) as i32;
    for step in 0..=steps.max(1) {
        let t = step as f32 / steps.max(1) as f32;
        let x = x0 + (x1 - x0) * t;
        let y = y0 + (y1 - y0) * t;
        draw_circle_rgba(rgba, x, y, thickness, color);
    }
}

fn set_rgba(rgba: &mut [u8], x: i32, y: i32, color: [u8; 4]) {
    if x < 0 || y < 0 || x >= WIDTH as i32 || y >= HEIGHT as i32 {
        return;
    }
    let offset = (y as usize * WIDTH as usize + x as usize) * 4;
    rgba[offset..offset + 4].copy_from_slice(&color);
}

fn polygon_area(points: &[Point]) -> f32 {
    points
        .iter()
        .enumerate()
        .map(|(index, point)| {
            let next = points[(index + 1) % points.len()];
            point.x * next.y - next.x * point.y
        })
        .sum::<f32>()
        * 0.5
}

#[derive(Clone, Debug)]
struct AssetBundle {
    frames: Vec<SourceFrame>,
    cape_layers: Vec<CapeLayerDescriptor>,
    cape_poses: Vec<CapePoseSource>,
}

#[derive(Clone, Debug)]
struct SourceFrame {
    name: String,
    pixels: Vec<AssetColor>,
}

#[derive(Clone, Debug)]
struct CapePoseSource {
    name: String,
    layers: Vec<CapeLayerSource>,
}

#[derive(Clone, Debug)]
struct CapeLayerSource {
    id: CapeLayerId,
    color: AssetColor,
    vertices: Vec<Point>,
}

#[derive(Clone, Copy, Debug)]
struct CapeLayerDescriptor {
    id: CapeLayerId,
    color: AssetColor,
    vertex_count: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
enum CapeLayerId {
    CapeFar = 0,
    CapeMain = 1,
    CapeNear = 2,
    CapeLower = 3,
    RibbonFar = 4,
    RibbonNear = 5,
}

impl CapeLayerId {
    fn from_str(value: &str) -> Option<Self> {
        match value {
            "cape_far" => Some(Self::CapeFar),
            "cape_main" => Some(Self::CapeMain),
            "cape_near" => Some(Self::CapeNear),
            "cape_lower" => Some(Self::CapeLower),
            "ribbon_far" => Some(Self::RibbonFar),
            "ribbon_near" => Some(Self::RibbonNear),
            _ => None,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::CapeFar => "cape_far",
            Self::CapeMain => "cape_main",
            Self::CapeNear => "cape_near",
            Self::CapeLower => "cape_lower",
            Self::RibbonFar => "ribbon_far",
            Self::RibbonNear => "ribbon_near",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
enum AssetColor {
    Transparent = 0,
    Foreground = 1,
    Accent = 2,
    MidTone = 3,
}

impl AssetColor {
    const fn rgb(self) -> (u8, u8, u8) {
        match self {
            Self::Transparent => (0, 0, 0),
            Self::Foreground => (0xcd, 0xd6, 0xf4),
            Self::Accent => (0xb4, 0xbe, 0xfe),
            Self::MidTone => (0x6c, 0x70, 0x86),
        }
    }

    const fn rgba(self) -> [u8; 4] {
        let (r, g, b) = self.rgb();
        [r, g, b, 0xff]
    }

    fn nearest(r: u8, g: u8, b: u8) -> Self {
        [Self::Foreground, Self::Accent, Self::MidTone]
            .into_iter()
            .min_by_key(|color| {
                let (cr, cg, cb) = color.rgb();
                let dr = i32::from(r) - i32::from(cr);
                let dg = i32::from(g) - i32::from(cg);
                let db = i32::from(b) - i32::from(cb);
                dr * dr + dg * dg + db * db
            })
            .unwrap_or(Self::Foreground)
    }
}

#[derive(Clone, Copy, Debug)]
struct Point {
    x: f32,
    y: f32,
}

#[derive(Debug, Error)]
pub enum DemoAssetError {
    #[error("unknown demo-assets action `{0}`; expected build or preview")]
    UnknownAction(String),
    #[error("missing frame `{0}`")]
    MissingFrame(String),
    #[error("frame name `{0}` is too long for KFA")]
    FrameNameTooLong(String),
    #[error("invalid cape topology: {0}")]
    InvalidCapeTopology(String),
    #[error("could not allocate SVG pixmap")]
    Pixmap,
    #[error("SVG parse failed for {0}: {1}")]
    SvgParse(PathBuf, String),
    #[error("invalid SVG {0}: {1}")]
    InvalidSvg(PathBuf, &'static str),
    #[error("unsupported SVG element `{1}` in {0}")]
    UnsupportedElement(PathBuf, String),
    #[error("SVG external references are not allowed in {0}")]
    ExternalReference(PathBuf),
    #[error("unknown SVG palette color `{1}` in {0}")]
    UnknownColor(PathBuf, String),
    #[error("missing SVG attribute `{1}` in {0}")]
    MissingAttribute(PathBuf, &'static str),
    #[error("invalid SVG number `{1}` in {0}")]
    InvalidNumber(PathBuf, String),
    #[error("I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error("PNG encode failed: {0}")]
    Png(#[from] png::EncodingError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kfa_generation_is_deterministic_for_same_bundle() {
        let bundle = test_bundle(vec![AssetColor::Transparent, AssetColor::Foreground]);
        assert_eq!(encode_kfa(&bundle).unwrap(), encode_kfa(&bundle).unwrap());
    }

    #[test]
    fn unknown_svg_colors_are_rejected() {
        let error = parse_fill(Path::new("bad.svg"), "#ff00ff").unwrap_err();
        assert!(matches!(error, DemoAssetError::UnknownColor(_, _)));
    }

    #[test]
    fn external_svg_references_are_rejected() {
        let document = Document::parse(
            r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 320 180"><polygon fill="#CDD6F4" href="file.png" points="0,0 1,0 0,1"/></svg>"##,
        )
        .unwrap();
        let error = validate_svg(Path::new("bad.svg"), &document).unwrap_err();
        assert!(matches!(error, DemoAssetError::ExternalReference(_)));
    }

    #[test]
    fn mismatched_viewbox_is_rejected() {
        let document =
            Document::parse(r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 160 90"/>"##)
                .unwrap();
        let error = validate_svg(Path::new("bad.svg"), &document).unwrap_err();
        assert!(matches!(error, DemoAssetError::InvalidSvg(_, _)));
    }

    #[test]
    fn cape_topology_rejects_mismatched_vertex_count() {
        let mut poses = test_cape_poses();
        poses[1].layers[0].vertices.pop();
        let error = validate_cape_topology(&poses).unwrap_err();
        assert!(matches!(error, DemoAssetError::InvalidCapeTopology(_)));
    }

    #[test]
    fn cape_topology_rejects_anchor_drift() {
        let mut poses = test_cape_poses();
        poses[1].layers[0].vertices[0].x += 8.0;
        let error = validate_cape_topology(&poses).unwrap_err();
        assert!(matches!(error, DemoAssetError::InvalidCapeTopology(_)));
    }

    #[test]
    fn rle_compresses_repeated_colors() {
        let encoded = encode_rle(&[
            AssetColor::Transparent,
            AssetColor::Transparent,
            AssetColor::Accent,
        ]);
        assert_eq!(encoded, vec![0, 2, 0, 2, 1, 0]);
    }

    #[test]
    fn point_parser_requires_even_coordinate_count() {
        let error = parse_points(Path::new("bad.svg"), "1,2 3").unwrap_err();
        assert!(matches!(error, DemoAssetError::InvalidNumber(_, _)));
    }

    fn test_bundle(pixels: Vec<AssetColor>) -> AssetBundle {
        let poses = test_cape_poses();
        let cape_layers = validate_cape_topology(&poses).unwrap();
        AssetBundle {
            frames: vec![SourceFrame {
                name: "a".to_owned(),
                pixels,
            }],
            cape_layers,
            cape_poses: poses,
        }
    }

    fn test_cape_poses() -> Vec<CapePoseSource> {
        let layers = CAPE_LAYER_ORDER
            .iter()
            .map(|id| CapeLayerSource {
                id: *id,
                color: AssetColor::MidTone,
                vertices: vec![
                    Point { x: 10.0, y: 10.0 },
                    Point { x: 16.0, y: 12.0 },
                    Point { x: 12.0, y: 18.0 },
                ],
            })
            .collect::<Vec<_>>();
        vec![
            CapePoseSource {
                name: "a".to_owned(),
                layers: layers.clone(),
            },
            CapePoseSource {
                name: "b".to_owned(),
                layers,
            },
        ]
    }
}
