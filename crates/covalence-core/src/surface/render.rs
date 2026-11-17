//! Draws key images (ADR 0023): a control's resolved look, at its native
//! size and format.
//!
//! Text is shaped and rasterised with `cosmic-text`, from one embedded font
//! and no system fonts, so a key looks the same on every machine and the
//! snapshot tests are exact.

use cosmic_text::{
    Align, Attrs, Buffer, Color as TextColor, Family, FontSystem, Metrics, Shaping, SwashCache,
    Wrap, fontdb,
};
use tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, Transform};

/// Inter SemiBold (SIL Open Font License; `assets/fonts/Inter-LICENSE.txt`).
const INTER: &[u8] = include_bytes!("../../assets/fonts/Inter-SemiBold.ttf");

/// The most lines a label wraps to before it's cut off.
const MAX_LINES: usize = 3;
/// The largest and smallest label sizes, as fractions of the key's height.
const LARGEST: f32 = 0.24;
const SMALLEST: f32 = 0.13;
/// The space around a label, as a fraction of the key's width.
const PADDING: f32 = 0.06;
/// The stale marker's size, as a fraction of the key's width.
const MARKER: f32 = 0.18;
const MARKER_COLOR: Rgb = Rgb(0xf5, 0xa6, 0x23);

/// A colour, written `#rrggbb` (or `#rgb`), as the standard library makes them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    pub const BLACK: Rgb = Rgb(0, 0, 0);
    pub const WHITE: Rgb = Rgb(0xff, 0xff, 0xff);

    pub fn parse(text: &str) -> Option<Rgb> {
        let hex = text.strip_prefix('#')?;
        if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        let digit = |i: usize| u8::from_str_radix(&hex[i..i + 1], 16).ok();
        let pair = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
        match hex.len() {
            3 => Some(Rgb(digit(0)? * 17, digit(1)? * 17, digit(2)? * 17)),
            6 => Some(Rgb(pair(0)?, pair(2)?, pair(4)?)),
            _ => None,
        }
    }
}

/// How a control looks once its bindings are resolved.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Look {
    pub label: String,
    pub text_color: Rgb,
    pub background: Rgb,
    /// A value it shows is stale: drawn dimmed, with a marker (bindings §3).
    pub stale: bool,
}

impl Default for Look {
    /// A blank key.
    fn default() -> Self {
        Look {
            label: String::new(),
            text_color: Rgb::WHITE,
            background: Rgb::BLACK,
            stale: false,
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RenderError {
    #[error("{0:?} images aren't supported yet")]
    Format(String),
    #[error("images rotate by 0, 90, 180 or 270 degrees, not {0}")]
    Rotation(u16),
    #[error("a {0}×{1} image is empty or too large")]
    Size(u32, u32),
    #[error("cannot encode the image: {0}")]
    Encode(String),
}

/// Draws looks into images. Holds the font and its glyph cache.
pub struct Renderer {
    fonts: FontSystem,
    glyphs: SwashCache,
}

impl Default for Renderer {
    fn default() -> Self {
        let mut db = fontdb::Database::new();
        db.load_font_data(INTER.to_vec());
        Renderer {
            fonts: FontSystem::new_with_locale_and_db("en-US".into(), db),
            glyphs: SwashCache::new(),
        }
    }
}

impl Renderer {
    /// `look` as an image of `width` × `height` in `format`, `png` or
    /// `jpeg`, turned `rotation` degrees clockwise for a device mounted that
    /// way (gen-2 Stream Decks: 180). At 90 and 270, the look is drawn
    /// `height` wide before it turns.
    pub fn render(
        &mut self,
        look: &Look,
        width: u32,
        height: u32,
        format: &str,
        rotation: u16,
    ) -> Result<Vec<u8>, RenderError> {
        check_output(format, rotation)?;
        if width > 1024 || height > 1024 {
            return Err(RenderError::Size(width, height));
        }
        let (draw_w, draw_h) = if rotation % 180 == 90 {
            (height, width)
        } else {
            (width, height)
        };
        let mut pixmap = Pixmap::new(draw_w, draw_h).ok_or(RenderError::Size(width, height))?;
        let Rgb(r, g, b) = look.background;
        pixmap.fill(tiny_skia::Color::from_rgba8(r, g, b, 255));
        self.label(&mut pixmap, look);
        if look.stale {
            dim(&mut pixmap);
            marker(&mut pixmap);
        }
        let rgb = rotate(&rgb_of(&pixmap), draw_w, draw_h, rotation);
        match format {
            "png" => {
                encode_png(&rgb, width, height).map_err(|e| RenderError::Encode(e.to_string()))
            }
            _ => encode_jpeg(&rgb, width, height).map_err(|e| RenderError::Encode(e.to_string())),
        }
    }

    /// Draws the label centred, at the largest size that fits in
    /// [`MAX_LINES`] without breaking a word. At the smallest size, words
    /// break and lines past that are cut off.
    fn label(&mut self, pixmap: &mut Pixmap, look: &Look) {
        if look.label.trim().is_empty() {
            return;
        }
        let (w, h) = (pixmap.width() as f32, pixmap.height() as f32);
        let pad = (w * PADDING).round();
        let width = w - 2.0 * pad;
        let attrs = Attrs::new().family(Family::Name("Inter"));
        let mut buffer = Buffer::new(&mut self.fonts, Metrics::new(h * LARGEST, h * LARGEST));
        buffer.set_size(Some(width), None);

        let smallest = (h * SMALLEST).floor();
        let mut size = (h * LARGEST).floor();
        let mut wrap = Wrap::Word;
        loop {
            buffer.set_wrap(wrap);
            buffer.set_metrics(Metrics::new(size, (size * 1.15).round()));
            buffer.set_text(&look.label, &attrs, Shaping::Advanced, Some(Align::Center));
            buffer.shape_until_scroll(&mut self.fonts, false);
            // With word wrap, a word too wide for the key overflows its line.
            let fits = buffer.layout_runs().count() <= MAX_LINES
                && buffer.layout_runs().all(|run| run.line_w <= width + 0.5);
            if fits || wrap == Wrap::WordOrGlyph {
                break;
            }
            if size <= smallest {
                wrap = Wrap::WordOrGlyph;
            } else {
                size -= 1.0;
            }
        }

        let runs: Vec<(f32, f32, Vec<cosmic_text::PhysicalGlyph>)> = buffer
            .layout_runs()
            .take(MAX_LINES)
            .map(|run| {
                let glyphs = run
                    .glyphs
                    .iter()
                    .map(|g| g.physical((0.0, 0.0), 1.0))
                    .collect();
                (run.line_top, run.line_y, glyphs)
            })
            .collect();
        let Some(&(first_top, ..)) = runs.first() else {
            return;
        };
        let line_height = buffer.metrics().line_height;
        let block = runs.len() as f32 * line_height;
        let top = ((h - block) / 2.0).round() - first_top;

        let Rgb(r, g, b) = look.text_color;
        let color = TextColor::rgb(r, g, b);
        for (_, line_y, glyphs) in runs {
            for glyph in glyphs {
                let (gx, gy) = (pad as i32 + glyph.x, (top + line_y) as i32 + glyph.y);
                self.glyphs
                    .with_pixels(&mut self.fonts, glyph.cache_key, color, |x, y, c| {
                        blend(pixmap, gx + x, gy + y, c)
                    });
            }
        }
    }
}

/// Whether the renderer can make images in `format`, turned `rotation`.
pub fn check_output(format: &str, rotation: u16) -> Result<(), RenderError> {
    if !matches!(format, "png" | "jpeg") {
        return Err(RenderError::Format(format.to_owned()));
    }
    if !matches!(rotation, 0 | 90 | 180 | 270) {
        return Err(RenderError::Rotation(rotation));
    }
    Ok(())
}

/// An opaque pixmap's pixels as RGB.
fn rgb_of(pixmap: &Pixmap) -> Vec<u8> {
    pixmap
        .pixels()
        .iter()
        .flat_map(|p| [p.red(), p.green(), p.blue()])
        .collect()
}

/// Turns a `w` × `h` RGB image `rotation` degrees clockwise.
fn rotate(rgb: &[u8], w: u32, h: u32, rotation: u16) -> Vec<u8> {
    if rotation == 0 {
        return rgb.to_vec();
    }
    let (w, h) = (w as usize, h as usize);
    let mut out = vec![0u8; rgb.len()];
    for y in 0..h {
        for x in 0..w {
            // Where (x, y) lands, in an image `out_w` wide.
            let (nx, ny, out_w) = match rotation {
                90 => (h - 1 - y, x, h),
                180 => (w - 1 - x, h - 1 - y, w),
                _ => (y, w - 1 - x, h),
            };
            let from = (y * w + x) * 3;
            let to = (ny * out_w + nx) * 3;
            out[to..to + 3].copy_from_slice(&rgb[from..from + 3]);
        }
    }
    out
}

/// Encodes RGB as baseline JPEG, which Stream Decks take. Quality 90 keeps
/// small text crisp at 72 pixels.
fn encode_jpeg(
    rgb: &[u8],
    width: u32,
    height: u32,
) -> Result<Vec<u8>, jpeg_encoder::EncodingError> {
    let mut out = Vec::new();
    jpeg_encoder::Encoder::new(&mut out, 90).encode(
        rgb,
        width as u16,
        height as u16,
        jpeg_encoder::ColorType::Rgb,
    )?;
    Ok(out)
}

/// Encodes RGB as PNG, compressed for speed: an image goes out once, over
/// a LAN, and the default level is most of a key's drawing time (ADR 0023).
fn encode_png(rgb: &[u8], width: u32, height: u32) -> Result<Vec<u8>, png::EncodingError> {
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, width, height);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::Fast);
    encoder.set_filter(png::Filter::Sub);
    let mut writer = encoder.write_header()?;
    writer.write_image_data(rgb)?;
    writer.finish()?;
    Ok(out)
}

/// Blends one glyph pixel over the opaque background.
fn blend(pixmap: &mut Pixmap, x: i32, y: i32, c: TextColor) {
    let (w, h) = (pixmap.width() as i32, pixmap.height() as i32);
    if x < 0 || y < 0 || x >= w || y >= h || c.a() == 0 {
        return;
    }
    let pixel = &mut pixmap.pixels_mut()[(y * w + x) as usize];
    let a = u16::from(c.a());
    let mix =
        |src: u8, dst: u8| ((u16::from(src) * a + u16::from(dst) * (255 - a) + 127) / 255) as u8;
    *pixel = tiny_skia::PremultipliedColorU8::from_rgba(
        mix(c.r(), pixel.red()),
        mix(c.g(), pixel.green()),
        mix(c.b(), pixel.blue()),
        255,
    )
    .expect("opaque colours are valid");
}

/// Halves the brightness of every pixel.
fn dim(pixmap: &mut Pixmap) {
    for pixel in pixmap.pixels_mut() {
        *pixel = tiny_skia::PremultipliedColorU8::from_rgba(
            pixel.red() / 2,
            pixel.green() / 2,
            pixel.blue() / 2,
            255,
        )
        .expect("opaque colours are valid");
    }
}

/// The stale marker: a triangle in the top-right corner.
fn marker(pixmap: &mut Pixmap) {
    let w = pixmap.width() as f32;
    let size = (w * MARKER).round();
    let mut path = PathBuilder::new();
    path.move_to(w - size, 0.0);
    path.line_to(w, 0.0);
    path.line_to(w, size);
    path.close();
    let Some(path) = path.finish() else { return };
    let mut paint = Paint::default();
    let Rgb(r, g, b) = MARKER_COLOR;
    paint.set_color_rgba8(r, g, b, 255);
    paint.anti_alias = true;
    pixmap.fill_path(
        &path,
        &paint,
        FillRule::Winding,
        Transform::identity(),
        None,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_colours() {
        assert_eq!(Rgb::parse("#ff8000"), Some(Rgb(255, 128, 0)));
        assert_eq!(Rgb::parse("#F80"), Some(Rgb(255, 136, 0)));
        for bad in ["ff8000", "#ff80", "#gg0000", "", "#", "red", "#ff800é"] {
            assert_eq!(Rgb::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn rotates_clockwise() {
        // 2 × 1: red, green.
        let rgb = [255, 0, 0, 0, 255, 0];
        assert_eq!(rotate(&rgb, 2, 1, 180), [0, 255, 0, 255, 0, 0]);
        // Turned 90: 1 wide, red on top.
        assert_eq!(rotate(&rgb, 2, 1, 90), [255, 0, 0, 0, 255, 0]);
        // Turned 270: green on top.
        assert_eq!(rotate(&rgb, 2, 1, 270), [0, 255, 0, 255, 0, 0]);
        // 2 × 2 at 90: [a b / c d] becomes [c a / d b].
        let sq = [1, 1, 1, 2, 2, 2, 3, 3, 3, 4, 4, 4];
        assert_eq!(rotate(&sq, 2, 2, 90), [3, 3, 3, 1, 1, 1, 4, 4, 4, 2, 2, 2]);
    }

    #[test]
    fn draws_jpeg() {
        let mut r = Renderer::default();
        let look = Look {
            label: "Cam 1".into(),
            ..Look::default()
        };
        let jpeg = r.render(&look, 72, 72, "jpeg", 180).unwrap();
        assert_eq!(&jpeg[..2], [0xff, 0xd8], "a JPEG's SOI marker");
        assert_eq!(&jpeg[jpeg.len() - 2..], [0xff, 0xd9]);
    }

    #[test]
    fn rejects_what_it_cannot_draw() {
        let mut r = Renderer::default();
        let look = Look::default();
        assert_eq!(
            r.render(&look, 72, 72, "bmp", 0),
            Err(RenderError::Format("bmp".into()))
        );
        assert_eq!(
            r.render(&look, 72, 72, "png", 45),
            Err(RenderError::Rotation(45))
        );
        assert_eq!(
            r.render(&look, 0, 72, "png", 0),
            Err(RenderError::Size(0, 72))
        );
    }
}
