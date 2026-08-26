//! Windows theme (light/dark) detection + color palette for the strip.

/// Windows COLORREF: 0x00BBGGRR
pub type Rgb = u32;

pub const fn rgb(r: u8, g: u8, b: u8) -> Rgb {
    ((b as u32) << 16) | ((g as u32) << 8) | (r as u32)
}

/// Fill color keyed out of the layered strip (not drawn in UI).
pub const CHROMA: Rgb = rgb(0xFF, 0x00, 0xFF);

#[derive(Clone, Copy)]
pub struct Theme {
    pub fg: Rgb,
    pub fg_outline: Rgb,
    pub dot_ok: Rgb,
    pub dot_low: Rgb,
    pub dot_off: Rgb,
    pub pill_ok: Rgb,
    pub pill_mid: Rgb,
    pub pill_low: Rgb,
    pub pill_off_bg: Rgb,
    pub pill_off_fg: Rgb,
}

pub fn detect() -> Theme {
    // Overlay sits on the taskbar with no panel; always use bright glyphs + dark halo.
    Theme {
        fg: rgb(0xFF, 0xFF, 0xFF),
        fg_outline: rgb(0x14, 0x14, 0x18),
        dot_ok: rgb(0x3D, 0xDC, 0x6A),
        dot_low: rgb(0xFF, 0x4D, 0x4F),
        dot_off: rgb(0xC8, 0xC8, 0xCE),
        pill_ok: rgb(0x1F, 0xB8, 0x54),
        pill_mid: rgb(0xF0, 0xA0, 0x10),
        pill_low: rgb(0xE8, 0x3A, 0x3A),
        pill_off_bg: rgb(0x4A, 0x4A, 0x50),
        pill_off_fg: rgb(0xF2, 0xF2, 0xF5),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgb_packs_colorref_bgr() {
        // COLORREF is 0x00BBGGRR so red stays red in GDI.
        assert_eq!(rgb(0xE8, 0x3A, 0x3A), 0x003A_3A_E8);
        assert_eq!(rgb(0x1F, 0xB8, 0x54), 0x0054_B8_1F);
        assert_eq!(CHROMA, 0x00FF_00_FF);
    }

    #[test]
    fn overlay_uses_bright_white_text() {
        let t = detect();
        assert_eq!(t.fg, rgb(0xFF, 0xFF, 0xFF));
        assert_ne!(t.fg, t.fg_outline);
    }
}
