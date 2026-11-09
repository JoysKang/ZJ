//! Bounded project-image decoding. Render callbacks only receive completed BGRA pixels.
use gpui_kit::RenderImage;
use std::{
    io::{self, Cursor, Read},
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
};

pub const MAX_FILE_BYTES: u64 = crate::md_images::MAX_FILE_BYTES;
pub const MAX_DECODED_BYTES: u64 = crate::md_images::MAX_DECODED_BYTES;
pub const MAX_OPEN_IMAGES: usize = 8;

static DECODED_BYTES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

struct PixelReservation(u64);
impl PixelReservation {
    fn new(bytes: u64) -> io::Result<Self> {
        use std::sync::atomic::Ordering;
        DECODED_BYTES
            .try_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_add(bytes)
                    .filter(|total| *total <= MAX_DECODED_BYTES)
            })
            .map_err(|_| io::Error::other("图片解码总预算已用尽，请先关闭其他图片"))?;
        Ok(Self(bytes))
    }
}
impl Drop for PixelReservation {
    fn drop(&mut self) {
        DECODED_BYTES.fetch_sub(self.0, std::sync::atomic::Ordering::AcqRel);
    }
}

pub struct Decoded {
    pub image: Arc<RenderImage>,
    pub width: u32,
    pub height: u32,
    pub bytes: usize,
    pub format: &'static str,
    _pixels: PixelReservation,
}

impl Decoded {
    pub fn decoded_bytes(&self) -> u64 {
        u64::from(self.width) * u64::from(self.height) * 4
    }
}

pub fn supported(path: &Path) -> bool {
    path.extension().and_then(|s| s.to_str()).is_some_and(|s| {
        matches!(
            s.to_ascii_lowercase().as_str(),
            "png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp" | "svg" | "ico" | "tif" | "tiff"
        )
    })
}

pub fn load(root: Option<&Path>, path: &Path) -> io::Result<(PathBuf, Decoded)> {
    load_with_budget(root, path, MAX_DECODED_BYTES)
}

pub fn load_with_budget(
    root: Option<&Path>,
    path: &Path,
    budget: u64,
) -> io::Result<(PathBuf, Decoded)> {
    let (path, file) = crate::files::open_file(root, path)?;
    if file.metadata()?.len() > MAX_FILE_BYTES {
        return Err(io::Error::other("图片文件超过 16 MiB"));
    }
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes)?;
    let decoded = decode_with_budget(&path, &bytes, budget)?;
    Ok((path, decoded))
}

fn check_size(width: u32, height: u32) -> io::Result<()> {
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) * 4 > MAX_DECODED_BYTES {
        return Err(io::Error::other("图片尺寸无效或解码后超过 64 MiB"));
    }
    Ok(())
}

#[cfg(test)]
pub fn decode(path: &Path, bytes: &[u8]) -> io::Result<Decoded> {
    decode_with_budget(path, bytes, MAX_DECODED_BYTES)
}

pub fn decode_with_budget(path: &Path, bytes: &[u8], budget: u64) -> io::Result<Decoded> {
    let check_budget = |width: u32, height: u32| {
        check_size(width, height)?;
        if u64::from(width) * u64::from(height) * 4 > budget {
            return Err(io::Error::other("图片超过当前视图可用解码预算"));
        }
        PixelReservation::new(u64::from(width) * u64::from(height) * 4)
    };
    if workspace_editor_git::parse_lfs_pointer(bytes).is_some() {
        return Err(io::Error::other(
            "此文件是 Git LFS 指针，本地尚无图片内容；请用外部 Git LFS 工具获取内容后重新打开",
        ));
    }
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(io::Error::other("图片文件超过 16 MiB"));
    }
    let svg = path
        .extension()
        .is_some_and(|s| s.eq_ignore_ascii_case("svg"));
    let (mut rgba, format, reservation) = if svg {
        // Project SVGs cannot read linked files, fetch URLs, or decode embedded raster payloads.
        static FONTS: OnceLock<Arc<resvg::usvg::fontdb::Database>> = OnceLock::new();
        let options = resvg::usvg::Options {
            fontdb: FONTS
                .get_or_init(|| {
                    let mut db = resvg::usvg::fontdb::Database::new();
                    db.load_system_fonts();
                    Arc::new(db)
                })
                .clone(),
            image_href_resolver: resvg::usvg::ImageHrefResolver {
                resolve_data: Box::new(|_, _, _| None),
                resolve_string: Box::new(|_, _| None),
            },
            ..Default::default()
        };
        let tree = resvg::usvg::Tree::from_data(bytes, &options).map_err(io::Error::other)?;
        let size = tree.size().to_int_size();
        let reservation = check_budget(size.width(), size.height())?;
        let mut pixels = resvg::tiny_skia::Pixmap::new(size.width(), size.height())
            .ok_or_else(|| io::Error::other("无法分配图片内存"))?;
        resvg::render(
            &tree,
            resvg::tiny_skia::Transform::identity(),
            &mut pixels.as_mut(),
        );
        // tiny-skia is premultiplied RGBA; GPUI expects straight BGRA.
        for pixel in pixels.data_mut().as_chunks_mut::<4>().0 {
            let alpha = u32::from(pixel[3]);
            for channel in &mut pixel[..3] {
                *channel = (u32::from(*channel) * 255 + alpha / 2)
                    .checked_div(alpha)
                    .unwrap_or(0)
                    .min(255) as u8;
            }
        }
        (
            image::RgbaImage::from_raw(size.width(), size.height(), pixels.take())
                .ok_or_else(|| io::Error::other("SVG 像素数据无效"))?,
            "SVG（不加载嵌入或外部图片）",
            reservation,
        )
    } else {
        let mut reader = image::ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
        let format = reader
            .format()
            .ok_or_else(|| io::Error::other("无法识别图片格式"))?;
        let dimensions = image::ImageReader::with_format(Cursor::new(bytes), format)
            .into_dimensions()
            .map_err(io::Error::other)?;
        let reservation = check_budget(dimensions.0, dimensions.1)?;
        let mut limits = image::Limits::default();
        limits.max_alloc = Some(budget);
        reader.limits(limits);
        let rgba = reader.decode().map_err(io::Error::other)?.into_rgba8();
        check_size(rgba.width(), rgba.height())?;
        let name = match format {
            image::ImageFormat::Png => "PNG",
            image::ImageFormat::Jpeg => "JPEG",
            image::ImageFormat::Gif => "GIF（首帧）",
            image::ImageFormat::WebP => "WebP（首帧）",
            image::ImageFormat::Bmp => "BMP",
            image::ImageFormat::Ico => "ICO",
            image::ImageFormat::Tiff => "TIFF",
            _ => "图片",
        };
        (rgba, name, reservation)
    };
    let (width, height) = rgba.dimensions();
    for pixel in rgba.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }
    Ok(Decoded {
        image: Arc::new(RenderImage::new(vec![image::Frame::new(rgba)])),
        width,
        height,
        bytes: bytes.len(),
        format,
        _pixels: reservation,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    pub(crate) fn png(color: [u8; 4]) -> Vec<u8> {
        let mut out = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(4, 3, image::Rgba(color)))
            .write_to(&mut out, image::ImageFormat::Png)
            .unwrap();
        out.into_inner()
    }
    #[test]
    fn complete_raster_files_decode_to_actual_pixels() {
        for format in [
            image::ImageFormat::Png,
            image::ImageFormat::Jpeg,
            image::ImageFormat::Gif,
            image::ImageFormat::WebP,
            image::ImageFormat::Bmp,
        ] {
            let mut bytes = Cursor::new(Vec::new());
            image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
                4,
                3,
                image::Rgb([255, 0, 0]),
            ))
            .write_to(&mut bytes, format)
            .unwrap();
            let decoded = decode(Path::new("image.bin"), &bytes.into_inner()).unwrap();
            assert_eq!((decoded.width, decoded.height), (4, 3));
            assert_eq!(decoded.image.frame_count(), 1);
            let pixel = &decoded.image.as_bytes(0).unwrap()[..4];
            assert!(
                pixel[2] >= 250 && pixel[0] <= 5,
                "wrong BGRA pixel for {format:?}: {pixel:?}"
            );
        }
    }
    #[test]
    fn damaged_and_oversized_images_are_rejected_before_display() {
        let bytes = png([255, 0, 0, 255]);
        assert!(decode(Path::new("broken.png"), &bytes[..24]).is_err());
        assert!(check_size(100_000, 100_000).is_err());
        assert!(check_size(0, 1).is_err());
        assert!(
            decode(
                Path::new("large.png"),
                &vec![0; MAX_FILE_BYTES as usize + 1]
            )
            .is_err()
        );
    }
    #[test]
    fn svg_decodes_vector_pixels_and_rejects_huge_canvas() {
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="4" height="3"><rect width="4" height="3" fill="red"/></svg>"#;
        let decoded = decode(Path::new("vector.svg"), svg).unwrap();
        assert_eq!(&decoded.image.as_bytes(0).unwrap()[..4], &[0, 0, 255, 255]);
        assert!(
            decode(
                Path::new("huge.svg"),
                br#"<svg xmlns="http://www.w3.org/2000/svg" width="100000" height="100000"/>"#
            )
            .is_err()
        );
    }
}
