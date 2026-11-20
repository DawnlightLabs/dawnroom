//! Dawnlight styling for the terminal: safelight gradient (terracotta #C47855 -> sun #FFD12B)
//! progress bars that fill ░▒▓█, the Dawnroom tile glyph, the Dawnlight orb glyph in brand sky, muted text, and a plain fallback. Truecolor when the terminal says so,
//! 16 colours otherwise, nothing with NO_COLOR or when output isn't a terminal.

use std::io::IsTerminal;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Colors {
    None,
    Ansi16,
    True,
}

// Dawnlight brand kit: terracotta -> sun for progress, sky for the orb (11.8:1 on ink), a warm muted grey.
const SAFELIGHT_FROM: (u8, u8, u8) = (0xC4, 0x78, 0x55);
const SAFELIGHT_TO: (u8, u8, u8) = (0xFF, 0xD1, 0x2B);
const SKY: (u8, u8, u8) = (0xA8, 0xDA, 0xF9);
// The Dawnroom tile's flat two-tone: deep terracotta body, sun sweep.
const TILE_BODY: (u8, u8, u8) = (0xA8, 0x5B, 0x3B);
const TILE_SUN: (u8, u8, u8) = (0xFF, 0xD1, 0x2B);
const MUTED: (u8, u8, u8) = (0xA6, 0xA9, 0xAE);
const EMPTY: (u8, u8, u8) = (0x38, 0x3B, 0x40);

pub struct Style {
    pub colors: Colors,
    /// Redraw lines in place (stderr is a terminal).
    pub live: bool,
}

impl Style {
    pub fn detect() -> Self {
        let tty = std::io::stderr().is_terminal();
        let no_color = std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty());
        let colors = if no_color || !tty {
            Colors::None
        } else if matches!(std::env::var("COLORTERM").as_deref(), Ok("truecolor" | "24bit"))
            || std::env::var_os("WT_SESSION").is_some()
            || std::env::var("TERM_PROGRAM").is_ok_and(|t| t == "vscode")
        {
            Colors::True
        } else {
            Colors::Ansi16
        };
        if colors != Colors::None {
            enable_vt();
        }
        Self { colors, live: tty }
    }

    fn paint(&self, rgb: (u8, u8, u8), ansi16: u8, text: &str) -> String {
        match self.colors {
            Colors::None => text.to_string(),
            Colors::Ansi16 => format!("\x1b[{ansi16}m{text}\x1b[0m"),
            Colors::True => format!("\x1b[38;2;{};{};{}m{text}\x1b[0m", rgb.0, rgb.1, rgb.2),
        }
    }

    pub fn accent(&self, text: &str) -> String {
        self.paint(SAFELIGHT_FROM, 33, text)
    }

    /// The Dawnroom tile that opens the banner: a terracotta cell with the sun in its top-right corner.
    pub fn tile(&self) -> String {
        match self.colors {
            Colors::None => "◥".to_string(),
            Colors::Ansi16 => "[93;41m◥[0m".to_string(),
            Colors::True => format!(
                "[38;2;{};{};{};48;2;{};{};{}m◥[0m",
                TILE_SUN.0, TILE_SUN.1, TILE_SUN.2, TILE_BODY.0, TILE_BODY.1, TILE_BODY.2
            ),
        }
    }

    /// The Dawnlight orb glyph, only beside "by Dawnlight Labs".
    pub fn orb(&self) -> String {
        self.paint(SKY, 94, "●")
    }

    pub fn muted(&self, text: &str) -> String {
        self.paint(MUTED, 90, text)
    }

    pub fn bold(&self, text: &str) -> String {
        if self.colors == Colors::None { text.to_string() } else { format!("\x1b[1m{text}\x1b[0m") }
    }

    pub fn error(&self, text: &str) -> String {
        self.paint((0xFF, 0x5A, 0x5A), 91, text)
    }

    /// `width` cells; full cells are █, the leading cell fills ░ → ▒ → ▓, empty cells are dim ░.
    pub fn bar(&self, frac: f64, width: usize) -> String {
        let frac = frac.clamp(0.0, 1.0);
        let filled = frac * width as f64;
        let full = filled.floor() as usize;
        let mut out = String::new();
        for i in 0..width {
            let t = if width > 1 { i as f64 / (width - 1) as f64 } else { 0.0 };
            let rgb = lerp(SAFELIGHT_FROM, SAFELIGHT_TO, t);
            if i < full {
                out.push_str(&self.paint(rgb, if t < 0.5 { 31 } else { 33 }, "█"));
            } else if i == full && full < width {
                let part = filled - full as f64;
                let ch = match part {
                    p if p >= 2.0 / 3.0 => "▓",
                    p if p >= 1.0 / 3.0 => "▒",
                    p if p > 0.0 => "░",
                    _ => "",
                };
                if ch.is_empty() {
                    out.push_str(&self.empty_cell());
                } else {
                    out.push_str(&self.paint(rgb, 33, ch));
                }
            } else {
                out.push_str(&self.empty_cell());
            }
        }
        out
    }

    fn empty_cell(&self) -> String {
        match self.colors {
            Colors::None => "·".into(),
            _ => self.paint(EMPTY, 90, "░"),
        }
    }

    /// Redraw the current line (live) or print a new one (pipes, logs).
    pub fn line(&self, text: &str) {
        if self.live {
            // A line wider than the terminal wraps, and `\r` then only rewinds its last row:
            // every refresh would print a new line. Cut it to fit instead.
            eprint!("\r\x1b[K{}", fit_width(text, term_width().saturating_sub(1)));
        } else {
            eprintln!("{text}");
        }
    }

    /// Finish a live line.
    pub fn end_line(&self) {
        if self.live {
            eprintln!();
        }
    }
}

/// `text` cut to `max` visible characters; ANSI escape sequences pass through and don't count.
pub fn fit_width(text: &str, max: usize) -> String {
    let (mut out, mut seen, mut chars) = (String::new(), 0usize, text.chars());
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            out.push(c);
            for e in chars.by_ref() {
                out.push(e);
                if e.is_ascii_alphabetic() {
                    break;
                }
            }
        } else if seen < max {
            out.push(c);
            seen += 1;
        } else {
            out.push_str("\x1b[0m");
            break;
        }
    }
    out
}

/// Columns of the terminal on stderr; 80 when it can't be read.
fn term_width() -> usize {
    platform_width().filter(|w| *w > 10).unwrap_or(80)
}

#[cfg(windows)]
fn platform_width() -> Option<usize> {
    use windows_sys::Win32::System::Console::{CONSOLE_SCREEN_BUFFER_INFO, GetConsoleScreenBufferInfo, GetStdHandle, STD_ERROR_HANDLE};
    let mut info: CONSOLE_SCREEN_BUFFER_INFO = unsafe { std::mem::zeroed() };
    let ok = unsafe { GetConsoleScreenBufferInfo(GetStdHandle(STD_ERROR_HANDLE), &mut info) };
    (ok != 0).then(|| (info.srWindow.Right - info.srWindow.Left + 1) as usize)
}

#[cfg(unix)]
fn platform_width() -> Option<usize> {
    let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
    (unsafe { libc::ioctl(2, libc::TIOCGWINSZ, &mut ws) } == 0).then_some(ws.ws_col as usize)
}

#[cfg(test)]
#[test]
fn fit_width_cuts_visible_text_only() {
    assert_eq!(fit_width("abcdef", 4), "abcd\x1b[0m");
    assert_eq!(fit_width("\x1b[31mabc\x1b[0m", 2), "\x1b[31mab\x1b[0m");
    assert_eq!(fit_width("abc", 10), "abc");
}

fn lerp(a: (u8, u8, u8), b: (u8, u8, u8), t: f64) -> (u8, u8, u8) {
    let m = |x: u8, y: u8| (x as f64 + (y as f64 - x as f64) * t).round() as u8;
    (m(a.0, b.0), m(a.1, b.1), m(a.2, b.2))
}

/// Old Windows consoles need VT processing switched on for ANSI colours.
#[cfg(windows)]
fn enable_vt() {
    use windows_sys::Win32::System::Console::{ENABLE_VIRTUAL_TERMINAL_PROCESSING, GetConsoleMode, GetStdHandle, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE, SetConsoleMode};
    for which in [STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
        unsafe {
            let h = GetStdHandle(which);
            let mut mode = 0;
            if GetConsoleMode(h, &mut mode) != 0 {
                SetConsoleMode(h, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING);
            }
        }
    }
}

#[cfg(not(windows))]
fn enable_vt() {}

pub fn gb(bytes: u64) -> String {
    format!("{:.1} GB", bytes as f64 / 1e9)
}

pub fn rate(bps: u64) -> String {
    format!("{:.0} MB/s", bps as f64 / 1e6)
}
