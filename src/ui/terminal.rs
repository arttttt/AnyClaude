use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::Widget;
use std::sync::{Arc, Mutex};
use termwiz::cell::{Blink, CellAttributes, Intensity, Underline};
use termwiz::color::{ColorAttribute, SrgbaTuple};
use termwiz::surface::Surface;

pub struct TerminalBody {
    screen: Arc<Mutex<Surface>>,
}

impl TerminalBody {
    pub fn new(screen: Arc<Mutex<Surface>>) -> Self {
        Self { screen }
    }
}

impl Widget for TerminalBody {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let screen = match self.screen.lock() {
            Ok(screen) => screen,
            Err(_) => return,
        };

        let max_rows = area.height as usize;
        let max_cols = area.width as usize;

        for (row_idx, line) in screen.screen_lines().iter().enumerate().take(max_rows) {
            let y = area.y + row_idx as u16;
            for cell in line.visible_cells() {
                let col_idx = cell.cell_index();
                if col_idx >= max_cols {
                    continue;
                }
                let width = cell.width();
                if col_idx + width > max_cols {
                    continue;
                }
                let x = area.x + col_idx as u16;
                let style = style_from_attributes(cell.attrs());
                let cell_ref = buf.get_mut(x, y);
                cell_ref.set_symbol(cell.str()).set_style(style);
            }
        }
    }
}

fn style_from_attributes(attrs: &CellAttributes) -> Style {
    let mut style = Style::default();

    if let Some(color) = color_from_attribute(attrs.foreground()) {
        style = style.fg(color);
    }
    if let Some(color) = color_from_attribute(attrs.background()) {
        style = style.bg(color);
    }

    match attrs.intensity() {
        Intensity::Bold => {
            style = style.add_modifier(Modifier::BOLD);
        }
        Intensity::Half => {
            style = style.add_modifier(Modifier::DIM);
        }
        Intensity::Normal => {}
    }

    if attrs.italic() {
        style = style.add_modifier(Modifier::ITALIC);
    }
    if attrs.reverse() {
        style = style.add_modifier(Modifier::REVERSED);
    }
    if attrs.strikethrough() {
        style = style.add_modifier(Modifier::CROSSED_OUT);
    }
    if attrs.invisible() {
        style = style.add_modifier(Modifier::HIDDEN);
    }

    if attrs.underline() != Underline::None {
        style = style.add_modifier(Modifier::UNDERLINED);
    }

    match attrs.blink() {
        Blink::Slow => {
            style = style.add_modifier(Modifier::SLOW_BLINK);
        }
        Blink::Rapid => {
            style = style.add_modifier(Modifier::RAPID_BLINK);
        }
        Blink::None => {}
    }

    style
}

fn color_from_attribute(color: ColorAttribute) -> Option<Color> {
    match color {
        ColorAttribute::Default => Some(Color::Reset),
        ColorAttribute::PaletteIndex(idx) => Some(Color::Indexed(idx)),
        ColorAttribute::TrueColorWithDefaultFallback(color)
        | ColorAttribute::TrueColorWithPaletteFallback(color, _) => Some(color_from_tuple(color)),
    }
}

fn color_from_tuple(color: SrgbaTuple) -> Color {
    let SrgbaTuple(red, green, blue, _) = color;
    Color::Rgb(
        float_to_channel(red),
        float_to_channel(green),
        float_to_channel(blue),
    )
}

fn float_to_channel(value: f32) -> u8 {
    let value = value.clamp(0.0, 1.0);
    (value * 255.0).round() as u8
}
