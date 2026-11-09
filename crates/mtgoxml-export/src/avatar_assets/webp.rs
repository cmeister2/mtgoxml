//! Lossless animated WebP output verified against every interval of its RGB timeline.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use serde::Serialize;
use sha2::{Digest, Sha256};
use webp_animation::{
    AnimParams, ColorMode, Decoder, Encoder, EncoderOptions, EncodingConfig, EncodingType,
};

use crate::{Result, digest_bytes};

/// Largest duration represented by an animated WebP frame's 24-bit millisecond field.
const MAX_FRAME_DURATION_MS: u32 = 0x00ff_ffff;

/// Completion evidence returned only after encoded pixels and playback timing verify.
#[derive(Debug, Serialize)]
pub(super) struct Metadata {
    /// Number of RGB source frames before duplicate frames are coalesced by the codec.
    pub(super) source_frames: usize,
    /// Number of displayed frames stored in the animation container.
    pub(super) encoded_frames: usize,
    /// Number of different decoded, full-canvas images.
    pub(super) distinct_frames: usize,
    /// Exact duration of one complete playback cycle.
    pub(super) duration_ms: u32,
    /// Infinite playback, checked in the encoded ANIM chunk.
    pub(super) loop_count: u16,
    /// Every decoded interval was compared byte-for-byte against its source RGB pixels.
    pub(super) pixel_timeline_exact: bool,
    /// Lossless RGB encoding, independently checked by complete timeline decoding.
    pub(super) lossless: bool,
    /// Lossless compression effort, rather than a reduction in pixel quality.
    pub(super) compression_effort: u8,
    /// Actual libwebp compression method retained by the pinned safe wrapper.
    pub(super) compression_method: u8,
    /// Whether the encoder exhaustively tries disposal candidates to minimize animation size.
    pub(super) minimize_size: bool,
    /// Whether the encoder may select lossy frames; always false for preserved avatars.
    pub(super) allow_mixed: bool,
    /// Complete encoded file size in bytes.
    pub(super) size: u64,
    /// SHA-256 of the complete encoded file.
    pub(super) sha256: String,
}

/// Encode and atomically install a lossless, infinitely looping RGB animation.
pub(super) fn write(
    path: &Path,
    width: u32,
    height: u32,
    frames: &[(Vec<u8>, u32)],
) -> Result<Metadata> {
    let total = validate_source(width, height, frames)?;
    let options = EncoderOptions {
        anim_params: AnimParams { loop_count: 0 },
        color_mode: ColorMode::Rgb,
        allow_mixed: false,
        // Use the codec's disposal heuristic to keep complete dataset exports practical.
        // Lossless effort 75 trades a small file-size increase for faster compression;
        // every decoded RGB value and playback interval still has to verify exactly.
        minimize_size: false,
        encoding_config: Some(EncodingConfig {
            encoding_type: EncodingType::Lossless,
            quality: 75.0,
            // webp-animation 0.10's apply_to does not copy this field into WebPConfig.
            // Keep the actual libwebp default (4), rather than claiming method 6 was used.
            method: 4,
        }),
        ..EncoderOptions::default()
    };
    let mut encoder = Encoder::new_with_options((width, height), options)?;
    let mut timestamp = 0_i32;
    for (pixels, duration) in frames {
        encoder.add_frame(pixels, timestamp)?;
        timestamp += *duration as i32;
    }
    let encoded = encoder.finalize(timestamp)?;
    let (encoded_frames, distinct_frames) = verify(&encoded, width, height, frames, total)?;
    let metadata = Metadata {
        source_frames: frames.len(),
        encoded_frames,
        distinct_frames,
        duration_ms: total,
        loop_count: 0,
        pixel_timeline_exact: true,
        lossless: true,
        compression_effort: 75,
        compression_method: 4,
        minimize_size: false,
        allow_mixed: false,
        size: encoded.len() as u64,
        sha256: digest_bytes(&encoded)?,
    };
    let temporary = path.with_extension("webp.tmp");
    fs::write(&temporary, &encoded)?;
    fs::rename(temporary, path)?;
    Ok(metadata)
}

/// Reject malformed input and timestamps that cannot fit the codec or container format.
fn validate_source(width: u32, height: u32, frames: &[(Vec<u8>, u32)]) -> Result<u32> {
    let pixels = (width as usize)
        .checked_mul(height as usize)
        .and_then(|count| count.checked_mul(3))
        .filter(|_| width > 0 && height > 0 && width <= 16_383 && height <= 16_383)
        .ok_or("invalid animated WebP dimensions")?;
    let first = frames
        .first()
        .ok_or("animated WebP requires source frames")?;
    let mut different = false;
    let mut total = 0_u32;
    for (rgb, duration) in frames {
        // Check every source buffer and accumulate timestamps before passing pixels to libwebp.
        if rgb.len() != pixels || *duration == 0 || *duration > MAX_FRAME_DURATION_MS {
            return Err("invalid animated WebP RGB frame size or duration".into());
        }
        total = total
            .checked_add(*duration)
            .filter(|total| *total <= i32::MAX as u32)
            .ok_or("animated WebP timeline exceeds the codec timestamp range")?;
        different |= rgb != &first.0;
    }
    if !different {
        // libwebp legitimately optimizes identical frames into a still image with no timeline.
        return Err("animated WebP requires at least two visually different frames".into());
    }
    Ok(total)
}

/// Read an unsigned 24-bit little-endian WebP field from an already bounded slice.
fn u24(bytes: &[u8]) -> u32 {
    u32::from(bytes[0]) | (u32::from(bytes[1]) << 8) | (u32::from(bytes[2]) << 16)
}

/// Validate RIFF boundaries, canvas, infinite looping, and every stored frame duration.
fn container_durations(bytes: &[u8], width: u32, height: u32) -> Result<Vec<u32>> {
    if bytes.len() < 12
        || &bytes[..4] != b"RIFF"
        || &bytes[8..12] != b"WEBP"
        || u32::from_le_bytes(bytes[4..8].try_into()?) as usize != bytes.len() - 8
    {
        return Err("invalid animated WebP RIFF container".into());
    }
    let mut offset = 12_usize;
    let mut canvas = false;
    let mut animation = false;
    let mut durations = Vec::new();
    while offset < bytes.len() {
        // Bound each RIFF payload before interpreting its canvas, animation, or frame fields.
        let header_end = offset
            .checked_add(8)
            .filter(|end| *end <= bytes.len())
            .ok_or("truncated animated WebP chunk header")?;
        let size = u32::from_le_bytes(bytes[offset + 4..header_end].try_into()?) as usize;
        let end = header_end
            .checked_add(size)
            .filter(|end| *end <= bytes.len())
            .ok_or("truncated animated WebP chunk")?;
        let payload = &bytes[header_end..end];
        match &bytes[offset..offset + 4] {
            b"VP8X" => {
                if canvas
                    || payload.len() != 10
                    || payload[0] & 2 == 0
                    || u24(&payload[4..7]) + 1 != width
                    || u24(&payload[7..10]) + 1 != height
                {
                    return Err("invalid animated WebP canvas or animation flag".into());
                }
                canvas = true;
            }
            b"ANIM" => {
                if animation
                    || !canvas
                    || payload.len() != 6
                    || u16::from_le_bytes(payload[4..6].try_into()?) != 0
                {
                    return Err(
                        "animated WebP must declare one infinitely looping animation".into(),
                    );
                }
                animation = true;
            }
            b"ANMF" => {
                // Preserve positive stored durations independently of the decoder's timestamps.
                if !animation || payload.len() < 16 {
                    return Err("invalid animated WebP frame header".into());
                }
                let duration = u24(&payload[12..15]);
                if duration == 0 {
                    return Err("animated WebP frame has a zero duration".into());
                }
                durations.push(duration);
            }
            _ => {}
        }
        offset = end
            .checked_add(size & 1)
            .filter(|end| *end <= bytes.len())
            .ok_or("truncated animated WebP chunk padding")?;
    }
    if !animation || durations.len() < 2 {
        return Err("encoded WebP has no multi-frame animation".into());
    }
    Ok(durations)
}

/// Compare complete decoded canvases over all source intervals, allowing codec coalescing.
fn verify(
    encoded: &[u8],
    width: u32,
    height: u32,
    frames: &[(Vec<u8>, u32)],
    total: u32,
) -> Result<(usize, usize)> {
    let durations = container_durations(encoded, width, height)?;
    let decoder = Decoder::new(encoded)?;
    if decoder.dimensions() != (width, height) {
        return Err("animated WebP decoded canvas differs from the source".into());
    }
    let mut decoded_count = 0_usize;
    let mut previous_end = 0_u32;
    let mut source_index = 0_usize;
    let mut source_end = frames[0].1;
    let mut different = BTreeSet::new();
    for frame in decoder {
        // Each decoded canvas must cover the same interval declared by its container frame.
        let duration = *durations
            .get(decoded_count)
            .ok_or("unexpected decoded WebP frame")?;
        let end = previous_end
            .checked_add(duration)
            .filter(|end| *end <= total)
            .ok_or("animated WebP decoded duration exceeds the source timeline")?;
        if frame.timestamp() != end as i32 || frame.dimensions() != (width, height) {
            return Err(
                "animated WebP decoded timestamps differ from the container timeline".into(),
            );
        }
        let rgba = frame.data();
        let mut covered = previous_end;
        while covered < end {
            // A codec-coalesced frame can span several identical source intervals.
            while source_end <= covered {
                // Advance only past source frames fully covered by the current playback instant.
                source_index += 1;
                source_end += frames
                    .get(source_index)
                    .ok_or("unexpected decoded WebP interval")?
                    .1;
            }
            let expected = &frames[source_index].0;
            if rgba.len() != expected.len() / 3 * 4
                || !rgba
                    .chunks_exact(4)
                    .zip(expected.chunks_exact(3))
                    .all(|(actual, rgb)| &actual[..3] == rgb && actual[3] == 255)
            {
                return Err(format!(
                    "animated WebP pixels differ from source frame {source_index}"
                )
                .into());
            }
            covered = end.min(source_end);
        }
        different.insert(Sha256::digest(rgba).to_vec());
        previous_end = end;
        decoded_count += 1;
    }
    // The wrapper's iterator ends on decoder failure; container count and final time catch it.
    if decoded_count != durations.len() || previous_end != total || different.len() < 2 {
        return Err(
            "animated WebP decoder did not reproduce the complete animated timeline".into(),
        );
    }
    Ok((decoded_count, different.len()))
}

/// Small pixel and timing regressions exercise the real vendored codec and decoder.
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// Give parallel tests distinct temporary output names.
    static NEXT_FILE: AtomicU64 = AtomicU64::new(0);

    /// Produce an asymmetric six-pixel image so RGB channel and row order remain observable.
    fn frame(color: [u8; 3]) -> Vec<u8> {
        let mut rgb = color.repeat(6);
        rgb[..3].copy_from_slice(&[13, 29, 47]);
        rgb[15..].copy_from_slice(&[91, 127, 211]);
        rgb
    }

    /// Adjacent duplicate frames may merge while every RGB value and millisecond stays exact.
    #[test]
    fn lossless_timeline_survives_duplicate_frame_coalescing() {
        let path = std::env::temp_dir().join(format!(
            "mtgoxml-webp-{}-{}.webp",
            std::process::id(),
            NEXT_FILE.fetch_add(1, Ordering::Relaxed)
        ));
        let frames = vec![
            (frame([255, 1, 2]), 100),
            (frame([255, 1, 2]), 150),
            (frame([3, 254, 4]), 80),
            (frame([3, 254, 4]), 20),
            (frame([5, 6, 253]), 300),
            (frame([255, 1, 2]), 50),
        ];
        let metadata = write(&path, 3, 2, &frames).unwrap();
        let bytes = fs::read(&path).unwrap();
        fs::remove_file(&path).unwrap();
        assert_eq!(metadata.source_frames, 6);
        assert!(metadata.encoded_frames < metadata.source_frames);
        assert_eq!(metadata.distinct_frames, 3);
        assert_eq!(metadata.duration_ms, 700);
        assert_eq!(metadata.loop_count, 0);
        assert!(metadata.pixel_timeline_exact);
        assert_eq!(metadata.size, bytes.len() as u64);
        assert_eq!(metadata.sha256, digest_bytes(&bytes).unwrap());
        // A valid encoded animation must still fail against changed source pixels or timing.
        let mut wrong_pixels = frames.clone();
        wrong_pixels[1].0[3] ^= 1;
        assert!(verify(&bytes, 3, 2, &wrong_pixels, 700).is_err());
        assert!(verify(&bytes, 3, 2, &frames, 701).is_err());
        let mut finite_loop = bytes.clone();
        let animation = finite_loop
            .windows(4)
            .position(|chunk| chunk == b"ANIM")
            .unwrap();
        finite_loop[animation + 12] = 1;
        assert!(verify(&finite_loop, 3, 2, &frames, 700).is_err());
    }

    /// Invalid dimensions, durations, buffers, and still images fail before installing output.
    #[test]
    fn rejects_invalid_source_frames_and_timestamps() {
        let mut frames = vec![(frame([1, 2, 3]), 100), (frame([4, 5, 6]), 100)];
        assert!(validate_source(0, 2, &frames).is_err());
        assert!(validate_source(3, 2, &[]).is_err());
        frames[0].1 = 0;
        assert!(validate_source(3, 2, &frames).is_err());
        frames[0].1 = MAX_FRAME_DURATION_MS + 1;
        assert!(validate_source(3, 2, &frames).is_err());
        frames[0].1 = 100;
        frames[0].0.pop();
        assert!(validate_source(3, 2, &frames).is_err());
        let still = vec![(frame([1, 2, 3]), 100), (frame([1, 2, 3]), 200)];
        assert!(validate_source(3, 2, &still).is_err());
        let long = vec![(frame([1, 2, 3]), MAX_FRAME_DURATION_MS); 129];
        assert!(validate_source(3, 2, &long).is_err());
    }
}
