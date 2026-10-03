//! One decoding contract for background installation and rendering. Attachment
//! formats are broader (notably SVG), so attachment staging is not validation.

pub(crate) fn decode(bytes: &[u8]) -> image::ImageResult<image::DynamicImage> {
    if bytes.len() > crate::attachments::MAX_ATTACHMENT_BYTES as usize {
        return Err(image::ImageError::Limits(
            image::error::LimitError::from_kind(image::error::LimitErrorKind::InsufficientMemory),
        ));
    }
    // Inspect the exact bytes that will be saved, not the source extension or
    // a second read of a file that could change between validation and copy.
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    reader.decode()
}

pub(crate) fn read(path: &std::path::Path) -> image::ImageResult<Vec<u8>> {
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(crate::attachments::MAX_ATTACHMENT_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > crate::attachments::MAX_ATTACHMENT_BYTES as usize {
        return Err(image::ImageError::Limits(
            image::error::LimitError::from_kind(image::error::LimitErrorKind::InsufficientMemory),
        ));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn background_decoder_rejects_oversized_dimensions_before_pixels() {
        let image = image::RgbaImage::new(8193, 1);
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image)
            .write_to(&mut bytes, image::ImageFormat::Png)
            .unwrap();
        assert!(matches!(
            decode(bytes.get_ref()),
            Err(image::ImageError::Limits(_))
        ));
    }

    #[test]
    fn background_decoder_bounds_pixel_allocation_within_allowed_dimensions() {
        // A valid BMP header needs no large fixture allocation. Its advertised
        // 8192-square RGB output exceeds the byte budget despite allowed sides.
        let mut bytes = vec![0; 54];
        bytes[..2].copy_from_slice(b"BM");
        bytes[2..6].copy_from_slice(&54u32.to_le_bytes());
        bytes[10..14].copy_from_slice(&54u32.to_le_bytes());
        bytes[14..18].copy_from_slice(&40u32.to_le_bytes());
        bytes[18..22].copy_from_slice(&8192u32.to_le_bytes());
        bytes[22..26].copy_from_slice(&8192u32.to_le_bytes());
        bytes[26..28].copy_from_slice(&1u16.to_le_bytes());
        bytes[28..30].copy_from_slice(&24u16.to_le_bytes());
        assert!(matches!(decode(&bytes), Err(image::ImageError::Limits(_))));
    }

    #[test]
    fn managed_background_reads_remain_bounded_if_the_file_changes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("large.png");
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(crate::attachments::MAX_ATTACHMENT_BYTES + 1)
            .unwrap();
        assert!(matches!(read(&path), Err(image::ImageError::Limits(_))));
    }
}
