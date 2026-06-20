use thiserror::Error;

use crate::canvas::PaletteIndex;
use crate::raster::Point;

const KFA_MAGIC: &[u8; 4] = b"KFA2";
const FIXED_SCALE: f32 = 16.0;
const REQUIRED_CAPE_LAYER_COUNT: usize = 6;
const DEMO_ASSET: &[u8] = include_bytes!("../../assets/generated/animation.kfa");

pub fn load_demo_asset() -> Result<KfaAsset, AssetError> {
    KfaAsset::from_bytes(DEMO_ASSET)
}

#[derive(Clone, Debug)]
pub struct KfaAsset {
    width: u16,
    height: u16,
    frames: Vec<AssetFrame>,
    cape_layers: Vec<CapeLayer>,
    cape_poses: Vec<CapePose>,
}

impl KfaAsset {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, AssetError> {
        let mut cursor = Cursor::new(bytes);
        let magic = cursor.read_array::<4>()?;
        if &magic != KFA_MAGIC {
            if magic[..3] == KFA_MAGIC[..3] {
                return Err(AssetError::UnsupportedVersion(magic[3]));
            }
            return Err(AssetError::InvalidMagic);
        }

        let width = cursor.read_u16()?;
        let height = cursor.read_u16()?;
        let frame_count = cursor.read_u16()? as usize;
        let cape_layer_count = cursor.read_u8()? as usize;
        let cape_pose_count = cursor.read_u16()? as usize;
        let expected_pixels = usize::from(width)
            .checked_mul(usize::from(height))
            .ok_or(AssetError::InvalidDimensions { width, height })?;

        if cape_layer_count != REQUIRED_CAPE_LAYER_COUNT || cape_pose_count == 0 {
            return Err(AssetError::InvalidCapeTopology);
        }

        let mut frame_descriptors = Vec::with_capacity(frame_count);
        for _ in 0..frame_count {
            let name_len = cursor.read_u8()? as usize;
            let name = read_name(&mut cursor, name_len, AssetError::InvalidFrameName)?;
            let byte_len = cursor.read_u32()? as usize;
            frame_descriptors.push((name, byte_len));
        }

        let mut seen_layers = [false; REQUIRED_CAPE_LAYER_COUNT];
        let mut cape_layers = Vec::with_capacity(cape_layer_count);
        for _ in 0..cape_layer_count {
            let raw_id = cursor.read_u8()?;
            let id = CapeLayerId::from_u8(raw_id).ok_or(AssetError::InvalidCapeLayer(raw_id))?;
            let slot = id as usize;
            if seen_layers[slot] {
                return Err(AssetError::DuplicateCapeLayer(raw_id));
            }
            seen_layers[slot] = true;
            let color = AssetColor::from_u8(cursor.read_u8()?)?;
            let vertex_count = cursor.read_u16()?;
            if vertex_count < 3 {
                return Err(AssetError::InvalidCapeTopology);
            }
            cape_layers.push(CapeLayer {
                id,
                color,
                vertex_count,
            });
        }
        if seen_layers.iter().any(|seen| !seen) {
            return Err(AssetError::InvalidCapeTopology);
        }

        let mut cape_pose_names = Vec::with_capacity(cape_pose_count);
        for _ in 0..cape_pose_count {
            let name_len = cursor.read_u8()? as usize;
            cape_pose_names.push(read_name(
                &mut cursor,
                name_len,
                AssetError::InvalidCapePoseName,
            )?);
        }

        let mut frames = Vec::with_capacity(frame_count);
        for (name, byte_len) in frame_descriptors {
            let data = cursor.read_bytes(byte_len)?;
            let pixels = decode_rle(data, expected_pixels)?;
            frames.push(AssetFrame { name, pixels });
        }

        let mut cape_poses = Vec::with_capacity(cape_pose_count);
        for name in cape_pose_names {
            let mut layers = Vec::with_capacity(cape_layers.len());
            for layer in &cape_layers {
                let mut vertices = Vec::with_capacity(layer.vertex_count as usize);
                for _ in 0..layer.vertex_count {
                    let x = cursor.read_u16()? as f32 / FIXED_SCALE;
                    let y = cursor.read_u16()? as f32 / FIXED_SCALE;
                    if x < 0.0 || y < 0.0 || x > f32::from(width) || y > f32::from(height) {
                        return Err(AssetError::InvalidVertex);
                    }
                    vertices.push(Point::new(x, y));
                }
                layers.push(CapePoseLayer { vertices });
            }
            cape_poses.push(CapePose { name, layers });
        }

        if cursor.remaining() != 0 {
            return Err(AssetError::TrailingData);
        }

        Ok(Self {
            width,
            height,
            frames,
            cape_layers,
            cape_poses,
        })
    }

    pub const fn width(&self) -> u16 {
        self.width
    }

    pub const fn height(&self) -> u16 {
        self.height
    }

    pub fn frame(&self, name: &str) -> Option<&AssetFrame> {
        self.frames.iter().find(|frame| frame.name == name)
    }

    pub fn cape_layers(&self) -> &[CapeLayer] {
        &self.cape_layers
    }

    pub fn cape_layer_index(&self, id: CapeLayerId) -> Option<usize> {
        self.cape_layers.iter().position(|layer| layer.id == id)
    }

    pub fn cape_pose(&self, name: &str) -> Option<&CapePose> {
        self.cape_poses.iter().find(|pose| pose.name == name)
    }

    pub fn max_cape_vertices(&self) -> usize {
        self.cape_layers
            .iter()
            .map(|layer| layer.vertex_count as usize)
            .max()
            .unwrap_or(0)
    }

    #[cfg(test)]
    pub fn frames(&self) -> &[AssetFrame] {
        &self.frames
    }

    #[cfg(test)]
    pub fn cape_poses(&self) -> &[CapePose] {
        &self.cape_poses
    }
}

#[derive(Clone, Debug)]
pub struct AssetFrame {
    name: String,
    pixels: Vec<AssetColor>,
}

impl AssetFrame {
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn pixel_at(&self, width: u16, x: i32, y: i32) -> AssetColor {
        let Ok(x) = usize::try_from(x) else {
            return AssetColor::Transparent;
        };
        let Ok(y) = usize::try_from(y) else {
            return AssetColor::Transparent;
        };
        let width = usize::from(width);
        let index = y.saturating_mul(width).saturating_add(x);
        self.pixels
            .get(index)
            .copied()
            .unwrap_or(AssetColor::Transparent)
    }

    #[cfg(test)]
    pub fn pixels(&self) -> &[AssetColor] {
        &self.pixels
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CapeLayer {
    id: CapeLayerId,
    color: AssetColor,
    vertex_count: u16,
}

impl CapeLayer {
    #[cfg(test)]
    pub const fn id(self) -> CapeLayerId {
        self.id
    }

    pub const fn color(self) -> AssetColor {
        self.color
    }

    #[cfg(test)]
    pub const fn vertex_count(self) -> u16 {
        self.vertex_count
    }
}

#[derive(Clone, Debug)]
pub struct CapePose {
    name: String,
    layers: Vec<CapePoseLayer>,
}

impl CapePose {
    #[cfg(test)]
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn layer_vertices(&self, layer_index: usize) -> Option<&[Point]> {
        self.layers
            .get(layer_index)
            .map(|layer| layer.vertices.as_slice())
    }
}

#[derive(Clone, Debug)]
struct CapePoseLayer {
    vertices: Vec<Point>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
#[repr(u8)]
pub enum CapeLayerId {
    CapeFar = 0,
    CapeMain = 1,
    CapeNear = 2,
    CapeLower = 3,
    RibbonFar = 4,
    RibbonNear = 5,
}

impl CapeLayerId {
    #[cfg(test)]
    pub const ALL: [Self; REQUIRED_CAPE_LAYER_COUNT] = [
        Self::CapeFar,
        Self::CapeMain,
        Self::CapeNear,
        Self::CapeLower,
        Self::RibbonFar,
        Self::RibbonNear,
    ];

    const fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::CapeFar),
            1 => Some(Self::CapeMain),
            2 => Some(Self::CapeNear),
            3 => Some(Self::CapeLower),
            4 => Some(Self::RibbonFar),
            5 => Some(Self::RibbonNear),
            _ => None,
        }
    }

    #[cfg(test)]
    pub const fn as_str(self) -> &'static str {
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
pub enum AssetColor {
    Transparent = 0,
    Foreground = 1,
    Accent = 2,
    MidTone = 3,
}

impl AssetColor {
    pub const fn to_palette(self) -> Option<PaletteIndex> {
        match self {
            Self::Transparent => None,
            Self::Foreground => Some(PaletteIndex::Foreground),
            Self::Accent => Some(PaletteIndex::Accent),
            Self::MidTone => Some(PaletteIndex::MidTone),
        }
    }

    fn from_u8(value: u8) -> Result<Self, AssetError> {
        match value {
            0 => Ok(Self::Transparent),
            1 => Ok(Self::Foreground),
            2 => Ok(Self::Accent),
            3 => Ok(Self::MidTone),
            value => Err(AssetError::InvalidColor(value)),
        }
    }
}

fn read_name(
    cursor: &mut Cursor<'_>,
    name_len: usize,
    error: AssetError,
) -> Result<String, AssetError> {
    let name = cursor.read_bytes(name_len)?;
    let name = std::str::from_utf8(name).map_err(|_| error)?.to_owned();
    Ok(name)
}

fn decode_rle(data: &[u8], expected_pixels: usize) -> Result<Vec<AssetColor>, AssetError> {
    let mut cursor = Cursor::new(data);
    let mut pixels = Vec::with_capacity(expected_pixels);
    while cursor.remaining() > 0 {
        let color = AssetColor::from_u8(cursor.read_u8()?)?;
        let count = cursor.read_u16()? as usize;
        if count == 0 {
            return Err(AssetError::CorruptedRun);
        }
        if pixels.len().saturating_add(count) > expected_pixels {
            return Err(AssetError::CorruptedRun);
        }
        pixels.resize(pixels.len() + count, color);
    }

    if pixels.len() != expected_pixels {
        return Err(AssetError::Truncated);
    }
    Ok(pixels)
}

struct Cursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Cursor<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn remaining(&self) -> usize {
        self.bytes.len().saturating_sub(self.offset)
    }

    fn read_u8(&mut self) -> Result<u8, AssetError> {
        Ok(self.read_array::<1>()?[0])
    }

    fn read_u16(&mut self) -> Result<u16, AssetError> {
        Ok(u16::from_le_bytes(self.read_array::<2>()?))
    }

    fn read_u32(&mut self) -> Result<u32, AssetError> {
        Ok(u32::from_le_bytes(self.read_array::<4>()?))
    }

    fn read_array<const N: usize>(&mut self) -> Result<[u8; N], AssetError> {
        let bytes = self.read_bytes(N)?;
        let mut out = [0; N];
        out.copy_from_slice(bytes);
        Ok(out)
    }

    fn read_bytes(&mut self, len: usize) -> Result<&'a [u8], AssetError> {
        let end = self.offset.checked_add(len).ok_or(AssetError::Truncated)?;
        let bytes = self
            .bytes
            .get(self.offset..end)
            .ok_or(AssetError::Truncated)?;
        self.offset = end;
        Ok(bytes)
    }
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum AssetError {
    #[error("invalid KFA magic")]
    InvalidMagic,
    #[error("unsupported KFA version byte `{0}`")]
    UnsupportedVersion(u8),
    #[error("invalid KFA dimensions {width} x {height}")]
    InvalidDimensions { width: u16, height: u16 },
    #[error("invalid KFA frame name")]
    InvalidFrameName,
    #[error("invalid KFA cape pose name")]
    InvalidCapePoseName,
    #[error("invalid KFA palette index `{0}`")]
    InvalidColor(u8),
    #[error("invalid KFA cape layer `{0}`")]
    InvalidCapeLayer(u8),
    #[error("duplicate KFA cape layer `{0}`")]
    DuplicateCapeLayer(u8),
    #[error("invalid KFA cape topology")]
    InvalidCapeTopology,
    #[error("invalid KFA cape vertex")]
    InvalidVertex,
    #[error("truncated KFA data")]
    Truncated,
    #[error("corrupted KFA run")]
    CorruptedRun,
    #[error("trailing KFA data")]
    TrailingData,
}

#[cfg(test)]
mod tests {
    use super::{AssetColor, AssetError, CapeLayerId, KFA_MAGIC, KfaAsset};

    #[test]
    fn invalid_magic_is_rejected() {
        assert_eq!(
            KfaAsset::from_bytes(b"NOPE").unwrap_err(),
            AssetError::InvalidMagic
        );
    }

    #[test]
    fn unsupported_version_is_rejected() {
        let mut data = b"KFA1".to_vec();
        data.extend_from_slice(&[1, 0, 1, 0, 0, 0]);
        assert_eq!(
            KfaAsset::from_bytes(&data).unwrap_err(),
            AssetError::UnsupportedVersion(b'1')
        );
    }

    #[test]
    fn truncated_asset_is_rejected() {
        assert_eq!(
            KfaAsset::from_bytes(KFA_MAGIC).unwrap_err(),
            AssetError::Truncated
        );
    }

    #[test]
    fn corrupted_run_is_rejected() {
        let data = test_kfa(&[1, 3, 0]);
        assert_eq!(
            KfaAsset::from_bytes(&data).unwrap_err(),
            AssetError::CorruptedRun
        );
    }

    #[test]
    fn invalid_color_is_rejected() {
        let data = test_kfa(&[9, 1, 0]);
        assert_eq!(
            KfaAsset::from_bytes(&data).unwrap_err(),
            AssetError::InvalidColor(9)
        );
    }

    #[test]
    fn invalid_cape_topology_is_rejected() {
        let mut data = test_kfa(&[1, 1, 0]);
        data[10] = 5;
        assert_eq!(
            KfaAsset::from_bytes(&data).unwrap_err(),
            AssetError::InvalidCapeTopology
        );
    }

    #[test]
    fn valid_asset_decodes_frame_pixels_and_cape_vertices() {
        let data = test_kfa(&[1, 1, 0]);
        let asset = KfaAsset::from_bytes(&data).unwrap();
        assert_eq!(asset.width(), 1);
        assert_eq!(asset.height(), 1);
        assert_eq!(asset.frames()[0].pixels(), &[AssetColor::Foreground]);
        assert_eq!(asset.cape_layers().len(), CapeLayerId::ALL.len());
        assert_eq!(asset.cape_poses()[0].name(), "cape_idle_a");
        assert_eq!(asset.cape_poses()[0].layer_vertices(0).unwrap().len(), 3);
    }

    fn test_kfa(run: &[u8]) -> Vec<u8> {
        let mut data = Vec::new();
        data.extend_from_slice(KFA_MAGIC);
        data.extend_from_slice(&1_u16.to_le_bytes());
        data.extend_from_slice(&1_u16.to_le_bytes());
        data.extend_from_slice(&1_u16.to_le_bytes());
        data.push(CapeLayerId::ALL.len() as u8);
        data.extend_from_slice(&1_u16.to_le_bytes());
        data.push(4);
        data.extend_from_slice(b"test");
        data.extend_from_slice(&(run.len() as u32).to_le_bytes());
        for id in CapeLayerId::ALL {
            data.push(id as u8);
            data.push(AssetColor::MidTone as u8);
            data.extend_from_slice(&3_u16.to_le_bytes());
        }
        data.push(11);
        data.extend_from_slice(b"cape_idle_a");
        data.extend_from_slice(run);
        for _ in CapeLayerId::ALL {
            for (x, y) in [(0_u16, 0_u16), (16, 0), (0, 16)] {
                data.extend_from_slice(&x.to_le_bytes());
                data.extend_from_slice(&y.to_le_bytes());
            }
        }
        data
    }
}
