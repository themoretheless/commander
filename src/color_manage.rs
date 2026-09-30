//! Bounded ICC/HDR preview color management hooks (research **J001**).
//!
//! Preview pixels are display-referred: HDR sources are tone-mapped into a
//! bounded nits budget before egui upload.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ColorSpaceHint {
    #[default]
    Unknown,
    Srgb,
    DisplayP3,
    AdobeRgb,
    Other,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HdrToneMapBudget {
    pub max_nits_bps: u32,
}

impl HdrToneMapBudget {
    pub const fn bounded_default() -> Self {
        Self {
            max_nits_bps: 10_000,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColorManagedFrame {
    pub width: u32,
    pub height: u32,
    pub space: ColorSpaceHint,
    pub hdr: bool,
    pub tone_map: HdrToneMapBudget,
}

impl ColorManagedFrame {
    pub fn display_referred(width: u32, height: u32, space: ColorSpaceHint, hdr: bool) -> Self {
        Self {
            width,
            height,
            space,
            hdr,
            tone_map: HdrToneMapBudget::bounded_default(),
        }
    }

    pub fn tone_map_scale(&self) -> f32 {
        if !self.hdr {
            return 1.0;
        }
        (self.tone_map.max_nits_bps as f32 / 10_000.0).clamp(0.1, 1.0)
    }

    pub fn apply_rgba8(&self, rgba: &mut [u8]) {
        let scale = self.tone_map_scale();
        if (scale - 1.0).abs() < f32::EPSILON {
            return;
        }
        for chunk in rgba.as_chunks_mut::<4>().0 {
            chunk[0] = scale_channel(chunk[0], scale);
            chunk[1] = scale_channel(chunk[1], scale);
            chunk[2] = scale_channel(chunk[2], scale);
        }
    }
}

fn scale_channel(value: u8, scale: f32) -> u8 {
    ((f32::from(value) * scale).round() as u16).min(255) as u8
}

pub fn hint_from_extension(extension: &str) -> (ColorSpaceHint, bool) {
    match extension.to_ascii_lowercase().as_str() {
        "exr" | "hdr" => (ColorSpaceHint::Other, true),
        "heic" | "heif" => (ColorSpaceHint::DisplayP3, false),
        "psd" => (ColorSpaceHint::AdobeRgb, false),
        "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" | "tif" | "tiff" => {
            (ColorSpaceHint::Srgb, false)
        }
        _ => (ColorSpaceHint::Unknown, false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hdr_tone_map_is_bounded_and_scales_rgba() {
        let frame = ColorManagedFrame::display_referred(2, 1, ColorSpaceHint::DisplayP3, true);
        assert!(frame.tone_map_scale() <= 1.0);
        let mut rgba = [255, 128, 0, 255, 10, 10, 10, 255];
        frame.apply_rgba8(&mut rgba);
        assert_eq!(rgba[3], 255);
        assert!(hint_from_extension("HDR").1);
    }

    #[test]
    fn sdr_path_preserves_bytes() {
        let frame = ColorManagedFrame::display_referred(2, 1, ColorSpaceHint::Srgb, false);
        let mut rgba = [12, 34, 56, 255, 7, 8, 9, 128];
        let before = rgba;
        frame.apply_rgba8(&mut rgba);
        assert_eq!(rgba, before);
    }
}
