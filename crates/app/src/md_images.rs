//! Images in the Markdown preview (开发说明 R09 / A19): relative and absolute local paths
//! are shown within a budget on the decoded size, read from the image header rather than the
//! compressed file size; remote images are never downloaded. No GPUI here: the preview
//! resolves a block's images in the background, the view only looks them up.

use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};

/// Largest image file read.
pub const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;
/// Largest decoded image (RGBA bytes), about 4096 × 4096.
pub const MAX_DECODED_BYTES: u64 = 64 * 1024 * 1024;

/// What an image URL in a document is shown as.
#[derive(Clone, Debug, PartialEq)]
pub enum Resolved {
    /// A local image within budget.
    Local(PathBuf),
    /// Not shown, and why (for the placeholder's tooltip / logs).
    Blocked(String),
}

/// Every image URL in Markdown text: `![alt](url "title")`, `<img src="url">`, and the URLs
/// of reference definitions (`[id]: url`), which images may use.
pub fn image_urls(text: &str) -> Vec<String> {
    let mut urls = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find("![") {
        rest = &rest[at + 2..];
        // The alt text may hold brackets; the image's `](` closes it.
        let Some(close) = rest.find("](") else {
            continue;
        };
        if rest[..close].contains('\n') && rest[..close].contains("\n\n") {
            continue;
        }
        let after = &rest[close + 2..];
        let target = after.trim_start();
        let url = if let Some(inner) = target.strip_prefix('<') {
            inner.split('>').next().unwrap_or_default()
        } else {
            target
                .split(|c: char| c.is_whitespace() || c == ')')
                .next()
                .unwrap_or_default()
        };
        if !url.is_empty() {
            urls.push(url.to_string());
        }
    }
    let lower = text.to_ascii_lowercase();
    let mut from = 0;
    while let Some(at) = lower[from..].find("<img") {
        let start = from + at;
        let end = lower[start..].find('>').map_or(lower.len(), |e| start + e);
        if let Some(src) = lower[start..end].find("src=") {
            let value = &text[start + src + 4..end];
            let quote = value.chars().next().filter(|c| *c == '"' || *c == '\'');
            let url = match quote {
                Some(q) => value[1..].split(q).next().unwrap_or_default(),
                None => value.split_whitespace().next().unwrap_or_default(),
            };
            if !url.is_empty() {
                urls.push(url.to_string());
            }
        }
        from = end;
    }
    for line in text.lines() {
        let line = line.trim_start();
        if line.starts_with('[')
            && let Some(close) = line.find("]:")
        {
            let url = line[close + 2..]
                .split_whitespace()
                .next()
                .unwrap_or_default();
            let url = url.trim_start_matches('<').trim_end_matches('>');
            if !url.is_empty() {
                urls.push(url.to_string());
            }
        }
    }
    urls.sort();
    urls.dedup();
    urls
}

/// Resolves one URL against the document's folder.
pub fn resolve(base: &Path, url: &str) -> Resolved {
    let lower = url.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("//") {
        return Resolved::Blocked("远程图片不自动加载".into());
    }
    let path = if let Some(path) = lower.strip_prefix("file://").map(|_| &url[7..]) {
        PathBuf::from(percent_decode(path))
    } else if url.contains("://") || lower.starts_with("data:") || lower.starts_with("mailto:") {
        return Resolved::Blocked("不支持的图片地址".into());
    } else {
        let url = url.split(['?', '#']).next().unwrap_or_default();
        base.join(percent_decode(url))
    };
    match check(&path) {
        Ok(()) => Resolved::Local(path),
        Err(reason) => Resolved::Blocked(reason),
    }
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(byte) = text
                .get(i + 1..i + 3)
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
        {
            out.push(byte);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn check(path: &Path) -> Result<(), String> {
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    if !matches!(
        extension.as_str(),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "bmp"
    ) {
        return Err("不是支持的图片格式".into());
    }
    let metadata = fs::metadata(path).map_err(|e| format!("找不到图片：{e}"))?;
    if !metadata.is_file() {
        return Err("不是文件".into());
    }
    if metadata.len() > MAX_FILE_BYTES {
        return Err(format!("图片超过 {} MB", MAX_FILE_BYTES / 1024 / 1024));
    }
    if extension == "svg" {
        return Ok(());
    }
    let mut header = Vec::new();
    fs::File::open(path)
        .and_then(|file| file.take(64 * 1024).read_to_end(&mut header))
        .map_err(|e| format!("无法读取图片：{e}"))?;
    let (width, height) = dimensions(&header).ok_or("无法识别图片尺寸")?;
    if u64::from(width) * u64::from(height) * 4 > MAX_DECODED_BYTES {
        return Err(format!("图片 {width}×{height} 解码后过大"));
    }
    Ok(())
}

/// Width and height from a PNG, GIF, JPEG, WebP or BMP header.
pub fn dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    let be32 = |at: usize| Some(u32::from_be_bytes(bytes.get(at..at + 4)?.try_into().ok()?));
    let le16 = |at: usize| {
        Some(u32::from(u16::from_le_bytes(
            bytes.get(at..at + 2)?.try_into().ok()?,
        )))
    };
    let be16 = |at: usize| {
        Some(u32::from(u16::from_be_bytes(
            bytes.get(at..at + 2)?.try_into().ok()?,
        )))
    };
    let le32 = |at: usize| Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?));
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some((be32(16)?, be32(20)?));
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some((le16(6)?, le16(8)?));
    }
    if bytes.starts_with(b"BM") {
        return Some((le32(18)?, le32(22)?.max(1)));
    }
    if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        return match bytes.get(12..16)? {
            b"VP8 " => Some((le16(26)? & 0x3fff, le16(28)? & 0x3fff)),
            b"VP8L" => {
                let bits = le32(21)?;
                Some(((bits & 0x3fff) + 1, ((bits >> 14) & 0x3fff) + 1))
            }
            b"VP8X" => {
                let w = le32(24)? & 0x00ff_ffff;
                let h = le32(27)? & 0x00ff_ffff;
                Some((w + 1, h + 1))
            }
            _ => None,
        };
    }
    if bytes.starts_with(&[0xff, 0xd8]) {
        let mut at = 2;
        while at + 9 < bytes.len() {
            if bytes[at] != 0xff {
                at += 1;
                continue;
            }
            let marker = bytes[at + 1];
            // Start of frame (not DHT / JPG / DAC).
            if (0xc0..=0xcf).contains(&marker) && !matches!(marker, 0xc4 | 0xc8 | 0xcc) {
                return Some((be16(at + 7)?, be16(at + 5)?));
            }
            at += 2 + be16(at + 2)? as usize;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_inline_html_and_reference_image_urls() {
        let text = "![logo](img/logo.png \"t\") text ![a [b]](<my pic.png>)\n\
                    <IMG alt=x SRC='html.gif'>\n[ref]: ./ref.jpg\n![remote](https://x/y.png)";
        assert_eq!(
            image_urls(text),
            [
                "./ref.jpg",
                "html.gif",
                "https://x/y.png",
                "img/logo.png",
                "my pic.png"
            ]
        );
    }

    #[test]
    fn reads_image_sizes_from_headers() {
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        png.extend_from_slice(&640u32.to_be_bytes());
        png.extend_from_slice(&480u32.to_be_bytes());
        assert_eq!(dimensions(&png), Some((640, 480)));
        let gif = b"GIF89a\x20\x00\x10\x00";
        assert_eq!(dimensions(gif), Some((32, 16)));
        // SOI, an APP0 segment, then SOF0 with height 100 and width 200.
        let jpeg = [
            0xff, 0xd8, 0xff, 0xe0, 0x00, 0x04, 0x00, 0x00, 0xff, 0xc0, 0x00, 0x11, 0x08, 0x00,
            0x64, 0x00, 0xc8, 0x03,
        ];
        assert_eq!(dimensions(&jpeg), Some((200, 100)));
        assert_eq!(dimensions(b"not an image"), None);
    }

    #[test]
    fn resolves_local_images_within_budget_and_blocks_the_rest() {
        let dir = std::env::temp_dir().join(format!("zj-md-images-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("img")).unwrap();
        let png = |w: u32, h: u32| {
            let mut bytes = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
            bytes.extend_from_slice(&w.to_be_bytes());
            bytes.extend_from_slice(&h.to_be_bytes());
            bytes
        };
        fs::write(dir.join("img/small pic.png"), png(10, 10)).unwrap();
        fs::write(dir.join("huge.png"), png(20_000, 20_000)).unwrap();
        fs::write(dir.join("notes.txt"), "x").unwrap();
        assert_eq!(
            resolve(&dir, "img/small%20pic.png"),
            Resolved::Local(dir.join("img/small pic.png"))
        );
        assert_eq!(
            resolve(&dir, "img/small pic.png?raw=1"),
            Resolved::Local(dir.join("img/small pic.png"))
        );
        let absolute = format!("file://{}", dir.join("img/small pic.png").display());
        assert!(matches!(resolve(&dir, &absolute), Resolved::Local(_)));
        for blocked in [
            "https://example.com/a.png",
            "//cdn/a.png",
            "data:image/png;base64,AAAA",
            "huge.png",
            "notes.txt",
            "missing.png",
        ] {
            assert!(
                matches!(resolve(&dir, blocked), Resolved::Blocked(_)),
                "{blocked}"
            );
        }
        fs::remove_dir_all(&dir).unwrap();
    }
}
