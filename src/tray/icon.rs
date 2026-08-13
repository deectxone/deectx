//! Tray icon rendering: the deeCtx logo (same glyph as `DeeCtx-web`'s
//! favicon) tinted by state, composited at runtime from alpha masks. No
//! image-decoding dependency or external asset file is needed at
//! runtime — the masks in `icon_masks.rs` are pre-rasterized from
//! `DeeCtx-web/public/favicon.svg` by `scripts/gen_tray_icon_masks.py` and
//! baked in as plain byte arrays; only the fill colors are picked at runtime.

use crate::tray::icon_masks::{BG_MASK, GLYPH_MASK};

pub const ICON_SIZE: u32 = 32;

/// Accent green — masking active. Same token as `src/dashboard.html` and
/// `DeeCtx-web/public/favicon.svg`.
pub const COLOR_ACTIVE: [u8; 3] = [0x12, 0x74, 0x4f];
/// Ink-faint grey — masking stopped.
pub const COLOR_STOPPED: [u8; 3] = [0x83, 0x88, 0x7e];
/// Warn amber — stopped, but with a restore warning to show.
pub const COLOR_WARNING: [u8; 3] = [0xb3, 0x78, 0x1c];
/// Glyph foreground — same cream token as the favicon's stroke color.
const COLOR_GLYPH: [u8; 3] = [0xf5, 0xf4, 0xef];

/// Renders the deeCtx logo — a `rgb`-filled rounded-square background with
/// the brand glyph on top — as a tightly-packed RGBA8 buffer (row-major, 4
/// bytes/pixel) on an `ICON_SIZE`x`ICON_SIZE` canvas.
pub fn render_circle_rgba(rgb: [u8; 3]) -> Vec<u8> {
    let size = ICON_SIZE as usize;
    let mut rgba = vec![0u8; size * size * 4];
    for i in 0..(size * size) {
        let bg_a = BG_MASK[i] as u16;
        let glyph_a = GLYPH_MASK[i] as u16;
        let idx = i * 4;

        // Background: state color at bg_a coverage.
        let mut r = rgb[0] as u16;
        let mut g = rgb[1] as u16;
        let mut b = rgb[2] as u16;
        let mut a = bg_a;

        // Glyph: cream, alpha-blended over whatever the background left.
        if glyph_a > 0 {
            r = (COLOR_GLYPH[0] as u16 * glyph_a + r * (255 - glyph_a)) / 255;
            g = (COLOR_GLYPH[1] as u16 * glyph_a + g * (255 - glyph_a)) / 255;
            b = (COLOR_GLYPH[2] as u16 * glyph_a + b * (255 - glyph_a)) / 255;
            a = a.max(glyph_a);
        }

        rgba[idx] = r as u8;
        rgba[idx + 1] = g as u8;
        rgba[idx + 2] = b as u8;
        rgba[idx + 3] = a as u8;
    }
    rgba
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffer_is_tightly_packed_rgba() {
        let buf = render_circle_rgba(COLOR_ACTIVE);
        assert_eq!(buf.len(), (ICON_SIZE * ICON_SIZE * 4) as usize);
    }

    #[test]
    fn corner_pixel_is_transparent() {
        let buf = render_circle_rgba(COLOR_ACTIVE);
        // top-left corner (0,0) is outside the rounded-square background.
        assert_eq!(
            buf[3], 0,
            "corner alpha must be 0 (outside the rounded square)"
        );
    }

    #[test]
    fn center_left_pixel_is_background_colored() {
        // The glyph's vertical stroke sits left-of-center; a pixel further
        // left, still inside the rounded square, should show pure bg color.
        let buf = render_circle_rgba(COLOR_ACTIVE);
        let c = ICON_SIZE as usize / 2;
        let x = 3usize;
        let idx = (c * ICON_SIZE as usize + x) * 4;
        assert_eq!(&buf[idx..idx + 3], &COLOR_ACTIVE[..]);
        assert_eq!(buf[idx + 3], 255);
    }

    #[test]
    fn different_states_render_different_background_colors() {
        let active = render_circle_rgba(COLOR_ACTIVE);
        let stopped = render_circle_rgba(COLOR_STOPPED);
        let c = ICON_SIZE as usize / 2;
        let x = 3usize;
        let idx = (c * ICON_SIZE as usize + x) * 4;
        assert_ne!(active[idx..idx + 3], stopped[idx..idx + 3]);
    }

    #[test]
    fn glyph_center_pixel_is_cream_tinted_regardless_of_state() {
        // A point on the glyph's curve should read close to the cream
        // foreground color, not the flat state color, for every state.
        let size = ICON_SIZE as usize;
        // Find a pixel with strong glyph coverage to make the assertion
        // robust to minor mask regeneration changes.
        let (gx, gy) = (0..size)
            .flat_map(|y| (0..size).map(move |x| (x, y)))
            .max_by_key(|&(x, y)| GLYPH_MASK[y * size + x])
            .unwrap();
        assert!(
            GLYPH_MASK[gy * size + gx] > 200,
            "expected a strongly-covered glyph pixel"
        );
        let buf = render_circle_rgba(COLOR_ACTIVE);
        let idx = (gy * size + gx) * 4;
        assert!(
            buf[idx] > 200 && buf[idx + 1] > 200 && buf[idx + 2] > 200,
            "glyph pixel should be cream-toned, got {:?}",
            &buf[idx..idx + 3]
        );
    }
}
