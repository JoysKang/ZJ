//! Strict decoding for bounded, read-only views. Never guesses a legacy encoding.
use std::io;

pub fn decode(
    bytes: &[u8],
    requested: Option<&'static encoding_rs::Encoding>,
) -> io::Result<(String, Option<&'static str>)> {
    let bom = encoding_rs::Encoding::for_bom(bytes);
    let encoding = requested.or_else(|| bom.map(|(encoding, _)| encoding));
    let skip = bom
        .filter(|(detected, _)| Some(*detected) == encoding)
        .map_or(0, |(_, length)| length);
    let Some(encoding) = encoding else {
        if bytes.contains(&0) {
            return Err(io::Error::other("二进制文件，未打开"));
        }
        return String::from_utf8(bytes.to_vec())
            .map(|text| (text, None))
            .map_err(|_| io::Error::other(crate::files::Restricted::NotUtf8));
    };
    let text = encoding
        .decode_without_bom_handling_and_without_replacement(&bytes[skip..])
        .ok_or_else(|| {
            io::Error::other(format!(
                "文件包含无效的 {} 序列，未显示替换字符",
                encoding.name()
            ))
        })?;
    let readonly =
        (requested.is_some() || encoding != encoding_rs::UTF_8).then_some(encoding.name());
    if text.contains('\0')
        || (readonly.is_some()
            && text
                .chars()
                .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t' | '\u{c}')))
    {
        return Err(io::Error::other("文件包含二进制控制字符，未按文本打开"));
    }
    if readonly.is_some()
        && (text.len() > crate::files::MAX_FILE_BYTES
            || text.split('\n').any(|line| line.len() > 256 * 1024))
    {
        return Err(io::Error::other(
            "解码文本超过 8 MiB 或单行超过 256 KiB，未载入编辑器",
        ));
    }
    // Normal UTF-8 BOM files retain their existing editable round-trip behavior.
    Ok((text.into_owned(), readonly))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bom_and_explicit_encodings_decode_strictly() {
        assert_eq!(
            decode(&[0xff, 0xfe, 0x2d, 0x4e, 0x87, 0x65], None).unwrap(),
            ("中文".into(), Some("UTF-16LE"))
        );
        assert_eq!(
            decode(&[0xfe, 0xff, 0x4e, 0x2d, 0x65, 0x87], None)
                .unwrap()
                .0,
            "中文"
        );
        assert_eq!(
            decode(&[0xd6, 0xd0, 0xce, 0xc4], Some(encoding_rs::GBK)).unwrap(),
            ("中文".into(), Some("GBK"))
        );
        assert_eq!(
            decode(b"\xef\xbb\xbfhello", None).unwrap(),
            ("hello".into(), None)
        );
        assert!(decode(&[0xff, 0xfe, 0x00], None).is_err());
        assert!(decode(&[0x81], Some(encoding_rs::GBK)).is_err());
        assert!(decode(b"binary\0data", Some(encoding_rs::GBK)).is_err());
    }
}
