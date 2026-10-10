//! Image attachments are read, checked and reduced for preview off the UI thread.

use crate::{agent_model::Attachment, md_images, theme};
use base64::{Engine, engine::general_purpose::STANDARD};
use gpui_kit::{Image, ImageFormat};
use std::{
    hash::{Hash, Hasher},
    io::{Cursor, Read},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    sync::Arc,
};

pub const MAX_IMAGES: usize = 8;

pub fn is_image(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        matches!(
            e.to_ascii_lowercase().as_str(),
            "png" | "jpg" | "jpeg" | "webp" | "gif"
        )
    })
}

pub fn pasted_paths(text: &str) -> Option<Vec<PathBuf>> {
    let paths: Vec<PathBuf> = text
        .lines()
        .filter(|s| !s.trim().is_empty())
        .map(|s| {
            let s = s.trim().trim_matches(['\'', '"']);
            PathBuf::from(
                s.strip_prefix("file://")
                    .map_or_else(|| s.to_string(), crate::md_images::percent_decode),
            )
        })
        .collect();
    (!paths.is_empty() && paths.iter().all(|p| p.is_absolute() && is_image(p))).then_some(paths)
}

pub fn from_path(path: PathBuf) -> Result<(Attachment, Arc<Image>), String> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(&path)
        .map_err(|e| format!("无法读取图片：{e}"))?;
    if !file
        .metadata()
        .map_err(|e| format!("无法读取图片信息：{e}"))?
        .is_file()
    {
        return Err("图片路径必须是普通文件".into());
    }
    let mut bytes = Vec::new();
    file.take(md_images::MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("无法读取图片：{e}"))?;
    from_bytes(crate::agent_model::file_name(&path), Some(path), bytes)
}

pub fn from_bytes(
    name: String,
    path: Option<PathBuf>,
    bytes: Vec<u8>,
) -> Result<(Attachment, Arc<Image>), String> {
    let (image, mime_type) = decode(&bytes)?;
    let preview = thumbnail(&image, theme::AGENT_IMAGE_WIDTH, theme::AGENT_IMAGE_HEIGHT)?;
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hash);
    Ok((
        Attachment::Image {
            id: hash.finish(),
            ordinal: None,
            name,
            path,
            data: STANDARD.encode(&bytes).into(),
            mime_type: mime_type.into(),
        },
        preview,
    ))
}

/// Decode on demand, bounding the enlarged image rather than retaining full-size textures.
pub fn enlarged(data: &str) -> Result<Arc<Image>, String> {
    let bytes = STANDARD
        .decode(data)
        .map_err(|e| format!("无法读取图片内容：{e}"))?;
    let (image, _) = decode(&bytes)?;
    thumbnail(
        &image,
        theme::AGENT_IMAGE_PREVIEW_WIDTH,
        theme::AGENT_IMAGE_PREVIEW_HEIGHT,
    )
}

fn decode(bytes: &[u8]) -> Result<(image::DynamicImage, &'static str), String> {
    if bytes.len() as u64 > md_images::MAX_FILE_BYTES {
        return Err("图片超过 16 MB".into());
    }
    let mut reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| e.to_string())?;
    let mime_type = match reader.format() {
        Some(image::ImageFormat::Png) => "image/png",
        Some(image::ImageFormat::Jpeg) => "image/jpeg",
        Some(image::ImageFormat::WebP) => "image/webp",
        Some(image::ImageFormat::Gif) => "image/gif",
        _ => return Err("图片仅支持 PNG、JPEG、WebP、GIF".into()),
    };
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(md_images::MAX_DECODED_BYTES);
    reader.limits(limits);
    let image = reader.decode().map_err(|e| format!("无法解码图片：{e}"))?;
    Ok((image, mime_type))
}

fn thumbnail(
    image: &image::DynamicImage,
    width: gpui_kit::Pixels,
    height: gpui_kit::Pixels,
) -> Result<Arc<Image>, String> {
    let thumb = image.thumbnail(
        (f32::from(width) as u32 * 2).min(image.width()),
        (f32::from(height) as u32 * 2).min(image.height()),
    );
    let mut preview = Cursor::new(Vec::new());
    thumb
        .write_to(&mut preview, image::ImageFormat::Png)
        .map_err(|e| format!("无法生成图片预览：{e}"))?;
    Ok(Arc::new(Image::from_bytes(
        ImageFormat::Png,
        preview.into_inner(),
    )))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn png() -> Vec<u8> {
        let mut bytes = Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(400, 200)
            .write_to(&mut bytes, image::ImageFormat::Png)
            .unwrap();
        bytes.into_inner()
    }

    #[test]
    fn image_payload_keeps_original_bytes_and_preview_is_small() {
        let bytes = png();
        let (attachment, preview) = from_bytes("test.png".into(), None, bytes.clone()).unwrap();
        let Attachment::Image {
            data, mime_type, ..
        } = attachment
        else {
            panic!()
        };
        assert_eq!(mime_type, "image/png");
        assert_eq!(STANDARD.decode(data.as_ref()).unwrap(), bytes);
        assert_eq!(md_images::dimensions(&preview.bytes), Some((208, 104)));
        assert!(from_bytes("invalid.png".into(), None, b"invalid".to_vec()).is_err());
        assert!(
            from_bytes(
                "huge.png".into(),
                None,
                vec![0; md_images::MAX_FILE_BYTES as usize + 1]
            )
            .is_err()
        );
        assert_eq!(
            pasted_paths("'/tmp/with space.PNG'\nfile:///tmp/a%20b.jpg"),
            Some(vec!["/tmp/with space.PNG".into(), "/tmp/a b.jpg".into()])
        );
        assert_eq!(pasted_paths("explain /tmp/a.png"), None);
    }

    #[test]
    fn enlarged_preview_uses_original_image_and_rejects_invalid_data() {
        let bytes = png();
        let image = enlarged(&STANDARD.encode(bytes)).unwrap();
        assert_eq!(md_images::dimensions(&image.bytes), Some((400, 200)));
        assert!(enlarged("not base64!").is_err());
        assert!(enlarged(&STANDARD.encode(b"not an image")).is_err());
    }

    #[test]
    fn a_named_pipe_cannot_block_image_loading() {
        let pipe = std::env::temp_dir().join(format!("zj-image-pipe-{}.png", std::process::id()));
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&pipe)
                .status()
                .unwrap()
                .success()
        );
        let started = std::time::Instant::now();
        assert!(from_path(pipe.clone()).unwrap_err().contains("普通文件"));
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
        std::fs::remove_file(pipe).unwrap();
    }
}
