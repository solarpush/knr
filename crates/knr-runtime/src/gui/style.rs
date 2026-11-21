//! Pure-Rust typed styling attributes and color primitives.
//!
//! Provides deterministic styling definitions, 32-bit ARGB color packing,
//! hex parsing, standard themes, and geometric primitives.

use serde::{Deserialize, Serialize};

/// 32-bit ARGB color representation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    /// Creates a color from RGBA channels.
    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    /// Creates an opaque color from RGB channels.
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    /// Creates a color from a 32-bit integer packed as `0xAARRGGBB`.
    pub const fn from_u32(val: u32) -> Self {
        Self {
            a: ((val >> 24) & 0xFF) as u8,
            r: ((val >> 16) & 0xFF) as u8,
            g: ((val >> 8) & 0xFF) as u8,
            b: (val & 0xFF) as u8,
        }
    }

    /// Converts the color to a 32-bit integer packed as `0xAARRGGBB`.
    pub const fn to_u32(&self) -> u32 {
        ((self.a as u32) << 24)
            | ((self.r as u32) << 16)
            | ((self.g as u32) << 8)
            | (self.b as u32)
    }

    /// Creates an opaque color from a 24-bit hex integer `0xRRGGBB`.
    pub const fn from_hex(hex: u32) -> Self {
        Self {
            r: ((hex >> 16) & 0xFF) as u8,
            g: ((hex >> 8) & 0xFF) as u8,
            b: (hex & 0xFF) as u8,
            a: 255,
        }
    }

    /// Parses a hex color string like `"#RRGGBB"`, `"#RRGGBBAA"`, `"RRGGBB"`, or `"RGB"`.
    pub fn parse_hex(s: &str) -> Option<Self> {
        let clean = s.trim().strip_prefix('#').unwrap_or(s.trim());
        if !clean.is_ascii() {
            return None;
        }
        match clean.len() {
            3 => {
                let r = u8::from_str_radix(&clean[0..1], 16).ok()?;
                let g = u8::from_str_radix(&clean[1..2], 16).ok()?;
                let b = u8::from_str_radix(&clean[2..3], 16).ok()?;
                Some(Self::rgb(r * 17, g * 17, b * 17))
            }
            6 => {
                let r = u8::from_str_radix(&clean[0..2], 16).ok()?;
                let g = u8::from_str_radix(&clean[2..4], 16).ok()?;
                let b = u8::from_str_radix(&clean[4..6], 16).ok()?;
                Some(Self::rgb(r, g, b))
            }
            8 => {
                let r = u8::from_str_radix(&clean[0..2], 16).ok()?;
                let g = u8::from_str_radix(&clean[2..4], 16).ok()?;
                let b = u8::from_str_radix(&clean[4..6], 16).ok()?;
                let a = u8::from_str_radix(&clean[6..8], 16).ok()?;
                Some(Self::rgba(r, g, b, a))
            }
            _ => None,
        }
    }

    /// Formats the color as a CSS/SVG hex string `#RRGGBB` (or `#RRGGBBAA` if transparent).
    pub fn to_hex_str(&self) -> String {
        if self.a == 255 {
            format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
        } else {
            format!(
                "#{:02x}{:02x}{:02x}{:02x}",
                self.r, self.g, self.b, self.a
            )
        }
    }

    // Standard high-tech systems palette
    pub const TRANSPARENT: Self = Self::rgba(0, 0, 0, 0);
    pub const BLACK: Self = Self::rgb(0, 0, 0);
    pub const WHITE: Self = Self::rgb(255, 255, 255);

    pub const BG_VOID: Self = Self::from_hex(0x050811);
    pub const CARD_SURFACE: Self = Self::from_hex(0x0f172a);
    pub const CARD_BORDER: Self = Self::from_hex(0x1e293b);
    pub const TEXT_PRIMARY: Self = Self::from_hex(0xf8fafc);
    pub const TEXT_SECONDARY: Self = Self::from_hex(0x94a3b8);
    pub const TEXT_MUTED: Self = Self::from_hex(0x64748b);

    pub const ACCENT_CYAN: Self = Self::from_hex(0x38bdf8);
    pub const ACCENT_EMERALD: Self = Self::from_hex(0x10b981);
    pub const ACCENT_AMBER: Self = Self::from_hex(0xf59e0b);
    pub const ACCENT_PURPLE: Self = Self::from_hex(0xa855f7);
    pub const ACCENT_ROSE: Self = Self::from_hex(0xf43f5e);
}

/// 2D rectangle primitive with signed coordinates and unsigned dimensions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

impl Rect {
    /// Constructs a new rectangle.
    pub const fn new(x: i32, y: i32, w: u32, h: u32) -> Self {
        Self { x, y, w, h }
    }

    /// Checks if a point `(px, py)` is inside this rectangle.
    pub fn contains(&self, px: i32, py: i32) -> bool {
        px >= self.x
            && px < self.x + self.w as i32
            && py >= self.y
            && py < self.y + self.h as i32
    }

    /// Computes the intersection of this rectangle with another.
    pub fn intersect(&self, other: &Rect) -> Option<Rect> {
        let x1 = self.x.max(other.x);
        let y1 = self.y.max(other.y);
        let x2 = (self.x + self.w as i32).min(other.x + other.w as i32);
        let y2 = (self.y + self.h as i32).min(other.y + other.h as i32);

        if x2 > x1 && y2 > y1 {
            Some(Rect {
                x: x1,
                y: y1,
                w: (x2 - x1) as u32,
                h: (y2 - y1) as u32,
            })
        } else {
            None
        }
    }
}

/// Declarative style configuration for UI and visual graph elements.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Style {
    pub bg_color: Option<Color>,
    pub text_color: Option<Color>,
    pub border_color: Option<Color>,
    pub border_width: u32,
    pub border_radius: u32,
    pub padding: [u32; 4], // top, right, bottom, left
    pub font_scale: u32,
    pub shadow_blur: u32,
    pub shadow_color: Option<Color>,
}

impl Default for Style {
    fn default() -> Self {
        Self {
            bg_color: None,
            text_color: Some(Color::TEXT_PRIMARY),
            border_color: None,
            border_width: 0,
            border_radius: 0,
            padding: [0, 0, 0, 0],
            font_scale: 1,
            shadow_blur: 0,
            shadow_color: None,
        }
    }
}

impl Style {
    /// Creates a default builder for `Style`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the background color.
    pub fn with_bg(mut self, color: Color) -> Self {
        self.bg_color = Some(color);
        self
    }

    /// Sets the text color.
    pub fn with_text_color(mut self, color: Color) -> Self {
        self.text_color = Some(color);
        self
    }

    /// Sets the border color and width.
    pub fn with_border(mut self, color: Color, width: u32) -> Self {
        self.border_color = Some(color);
        self.border_width = width;
        self
    }

    /// Sets the border radius for rounded rectangles.
    pub fn with_radius(mut self, radius: u32) -> Self {
        self.border_radius = radius;
        self
    }

    /// Sets uniform padding across all four sides.
    pub fn with_padding(mut self, pad: u32) -> Self {
        self.padding = [pad, pad, pad, pad];
        self
    }

    /// Sets font scaling factor.
    pub fn with_font_scale(mut self, scale: u32) -> Self {
        self.font_scale = scale.max(1);
        self
    }

    /// Sets shadow blur radius and color.
    pub fn with_shadow(mut self, blur: u32, color: Color) -> Self {
        self.shadow_blur = blur;
        self.shadow_color = Some(color);
        self
    }
}
