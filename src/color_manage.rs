//! Bounded ICC/HDR preview color management hooks (research **J001**).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ColorSpaceHint { #[default] Unknown, Srgb, DisplayP3, AdobeRgb, Other }
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HdrToneMapBudget { pub max_nits_bps: u32 }
impl HdrToneMapBudget { pub const fn bounded_default() -> Self { Self { max_nits_bps: 10_000 } } }
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColorManagedFrame { pub width: u32, pub height: u32, pub space: ColorSpaceHint, pub hdr: bool, pub tone_map: HdrToneMapBudget }
impl ColorManagedFrame {
    pub fn display_referred(width: u32, height: u32, space: ColorSpaceHint, hdr: bool) -> Self {
        Self { width, height, space, hdr, tone_map: HdrToneMapBudget::bounded_default() }
    }
    pub fn tone_map_scale(self) -> f32 {
        if !self.hdr { return 1.0; }
        (self.tone_map.max_nits_bps as f32 / 10_000.0).clamp(0.1, 1.0)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hdr_tone_map_is_bounded() {
        let frame = ColorManagedFrame::display_referred(64, 64, ColorSpaceHint::DisplayP3, true);
        assert!(frame.tone_map_scale() <= 1.0);
    }
}
