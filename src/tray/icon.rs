//! Pure pixel rendering for the tray icon states. No image files, no
//! external asset pipeline — deeCtx ships fully self-contained, so each
//! state is a small filled circle in a brand color, generated at runtime.

pub const ICON_SIZE: u32 = 32;

/// Accent green — masking active. Same token as `src/dashboard.html`.
pub const COLOR_ACTIVE: [u8; 3] = [0x12, 0x74, 0x4f];
/// Ink-faint grey — masking stopped.
pub const COLOR_STOPPED: [u8; 3] = [0x83, 0x88, 0x7e];
/// Warn amber — stopped, but with a restore warning to show.
pub const COLOR_WARNING: [u8; 3] = [0xb3, 0x78, 0x1c];

/// Renders a filled circle of `rgb` on a transparent `ICON_SIZE`x`ICON_SIZE`
/// canvas, as a tightly-packed RGBA8 buffer (row-major, 4 bytes/pixel).
pub fn render_circle_rgba(rgb: [u8; 3]) -> Vec<u8> {
    let size = ICON_SIZE;
    let mut rgba = vec![0u8; (size * size * 4) as usize];
    let center = (size as f32 - 1.0) / 2.0;
    let radius = size as f32 / 2.0 - 2.0;
    for y in 0..size {
        for x in 0..size {
            let dx = x as f32 - center;
            let dy = y as f32 - center;
            let idx = ((y * size + x) * 4) as usize;
            if (dx * dx + dy * dy).sqrt() <= radius {
                rgba[idx] = rgb[0];
                rgba[idx + 1] = rgb[1];
                rgba[idx + 2] = rgb[2];
                rgba[idx + 3] = 255;
            }
        }
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
    fn center_pixel_is_opaque_and_matches_color() {
        let buf = render_circle_rgba(COLOR_ACTIVE);
        let c = (ICON_SIZE / 2) as usize;
        let idx = (c * ICON_SIZE as usize + c) * 4;
        assert_eq!(&buf[idx..idx + 3], &COLOR_ACTIVE[..]);
        assert_eq!(buf[idx + 3], 255, "center must be fully opaque");
    }

    #[test]
    fn corner_pixel_is_transparent() {
        let buf = render_circle_rgba(COLOR_ACTIVE);
        // top-left corner (0,0) is outside the circle's radius.
        assert_eq!(buf[3], 0, "corner alpha must be 0 (outside the circle)");
    }

    #[test]
    fn different_states_render_different_colors() {
        let active = render_circle_rgba(COLOR_ACTIVE);
        let stopped = render_circle_rgba(COLOR_STOPPED);
        let c = (ICON_SIZE / 2) as usize;
        let idx = (c * ICON_SIZE as usize + c) * 4;
        assert_ne!(active[idx..idx + 3], stopped[idx..idx + 3]);
    }
}
