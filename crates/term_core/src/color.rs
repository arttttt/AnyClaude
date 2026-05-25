//! Terminal colour types and the standard ANSI 256-colour palette.

/// A terminal colour as carried in a `Cell`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum TermColor {
    /// Use the renderer's default foreground / background. Lets the GPU
    /// side apply its theme without having to special-case this here.
    #[default]
    Default,
    /// 0-15 standard, 16-231 6x6x6 cube, 232-255 grayscale ramp.
    Indexed(u8),
    /// True colour (24-bit) from `SGR 38;2;r;g;b` / `SGR 48;2;r;g;b`.
    Rgb(u8, u8, u8),
}

impl TermColor {
    pub const BLACK: Self = Self::Indexed(0);
    pub const RED: Self = Self::Indexed(1);
    pub const GREEN: Self = Self::Indexed(2);
    pub const YELLOW: Self = Self::Indexed(3);
    pub const BLUE: Self = Self::Indexed(4);
    pub const MAGENTA: Self = Self::Indexed(5);
    pub const CYAN: Self = Self::Indexed(6);
    pub const WHITE: Self = Self::Indexed(7);

    /// Resolve to linear-ish RGBA `f32` via the supplied palette. `Default`
    /// returns opaque white; the renderer is expected to override that with
    /// its theme.
    pub fn to_rgba(self, palette: &AnsiPalette) -> [f32; 4] {
        match self {
            Self::Default => [1.0, 1.0, 1.0, 1.0],
            Self::Indexed(idx) => palette.color(idx),
            Self::Rgb(r, g, b) => [
                r as f32 / 255.0,
                g as f32 / 255.0,
                b as f32 / 255.0,
                1.0,
            ],
        }
    }
}

/// Standard ANSI 256-colour palette.
///
/// Layout:
/// - 0-15: 16 base colours (theme-dependent — `default_dark` ships a
///   tomorrow-night-like ramp).
/// - 16-231: 6x6x6 RGB cube.
/// - 232-255: 24-step grayscale ramp.
pub struct AnsiPalette {
    colors: [[f32; 4]; 256],
}

impl AnsiPalette {
    /// A dark base 16 + the standard 240 generated entries. Reasonable
    /// default for the renderer; themes can build their own.
    pub fn default_dark() -> Self {
        let mut colors = [[0.0f32; 4]; 256];
        const BASE: [[u8; 3]; 16] = [
            [0x1d, 0x1f, 0x21], // 0 black
            [0xcc, 0x66, 0x66], // 1 red
            [0xb5, 0xbd, 0x68], // 2 green
            [0xf0, 0xc6, 0x74], // 3 yellow
            [0x81, 0xa2, 0xbe], // 4 blue
            [0xb2, 0x94, 0xbb], // 5 magenta
            [0x8a, 0xbe, 0xb7], // 6 cyan
            [0xc5, 0xc8, 0xc6], // 7 white
            [0x96, 0x98, 0x96], // 8 bright black
            [0xde, 0x93, 0x5f], // 9 bright red
            [0xa3, 0xbe, 0x8c], // 10 bright green
            [0xe5, 0xc0, 0x7b], // 11 bright yellow
            [0x7d, 0xae, 0xa3], // 12 bright blue
            [0xc7, 0x8d, 0xd4], // 13 bright magenta
            [0x70, 0xc0, 0xba], // 14 bright cyan
            [0xff, 0xff, 0xff], // 15 bright white
        ];
        for (i, rgb) in BASE.iter().enumerate() {
            colors[i] = [
                rgb[0] as f32 / 255.0,
                rgb[1] as f32 / 255.0,
                rgb[2] as f32 / 255.0,
                1.0,
            ];
        }
        // 216-entry RGB cube. Standard ramp from xterm.
        for i in 0..216 {
            let r = (i / 36) % 6;
            let g = (i / 6) % 6;
            let b = i % 6;
            let to_f = |v: usize| {
                if v == 0 {
                    0.0
                } else {
                    (55 + 40 * v) as f32 / 255.0
                }
            };
            colors[16 + i] = [to_f(r), to_f(g), to_f(b), 1.0];
        }
        // 24-step grayscale ramp.
        for i in 0..24 {
            let v = (8 + 10 * i) as f32 / 255.0;
            colors[232 + i] = [v, v, v, 1.0];
        }
        Self { colors }
    }

    pub fn color(&self, idx: u8) -> [f32; 4] {
        self.colors[idx as usize]
    }
}
