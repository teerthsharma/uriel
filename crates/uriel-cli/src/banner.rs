#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    TrueColor,
    Ansi256,
    Plain,
}

type Rgb = (u8, u8, u8);
type Cell = (char, Option<Rgb>);

const AMBER: Rgb = (255, 176, 32);
const ORANGE: Rgb = (232, 116, 59);
const SLATE: Rgb = (91, 107, 145);
const EDGE: Rgb = (122, 134, 166);
const RESET: &str = "\x1b[0m";
const TAGLINE: &str = "fewest tokens in, same information out";
const NAME: &str = "URIEL";
const WIDTH: usize = 40;
const FLAME_WIDTH: usize = 31;

const FLAME: [&[(usize, &str)]; 16] = [
    &[(15, ".")],
    &[(14, "/"), (16, "\\")],
    &[(13, "/"), (14, "h"), (15, "H"), (16, "j"), (17, "\\")],
    &[(12, "/"), (14, "╱"), (16, "╲"), (18, "\\")],
    &[(11, "/"), (13, "╱"), (17, "╲"), (19, "\\")],
    &[(10, "/"), (12, "W"), (13, "─"), (14, "─"), (15, "─"), (16, "─"), (17, "─"), (18, "W"), (20, "\\")],
    &[(9, "/"), (12, "│"), (13, "╲"), (17, "╱"), (18, "│"), (21, "\\")],
    &[(8, "("), (12, "│"), (14, "╲"), (16, "╱"), (18, "│"), (22, ")")],
    &[(7, "("), (12, "│"), (15, "W"), (18, "│"), (23, ")")],
    &[(7, "("), (12, "│"), (14, "╱"), (16, "╲"), (18, "│"), (23, ")")],
    &[(7, "("), (12, "│"), (13, "╱"), (17, "╲"), (18, "│"), (23, ")")],
    &[(7, "("), (12, "C"), (13, "─"), (14, "─"), (15, "─"), (16, "─"), (17, "─"), (18, "C"), (23, ")")],
    &[(8, "("), (12, "│"), (18, "│"), (22, ")")],
    &[(9, "\\"), (12, "C"), (13, "─"), (14, "─"), (15, "─"), (16, "─"), (17, "─"), (18, "C"), (21, "/")],
    &[(10, "\\"), (20, "/")],
    &[(11, "\\"), (12, "_"), (13, "_"), (14, "_"), (15, "_"), (16, "_"), (17, "_"), (18, "_"), (19, "/")],
];

const LETTERS: [[&str; 5]; 5] = [
    ["█   █", "█   █", "█   █", "█   █", " ███ "],
    ["████ ", "█   █", "████ ", "█  █ ", "█   █"],
    ["█████", "  █  ", "  █  ", "  █  ", "█████"],
    ["█████", "█    ", "████ ", "█    ", "█████"],
    ["█    ", "█    ", "█    ", "█    ", "█████"],
];

fn lerp(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let mix = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    (mix(a.0, b.0), mix(a.1, b.1), mix(a.2, b.2))
}

fn height_color(row: usize, rows: usize) -> Rgb {
    let t = row as f32 / (rows - 1) as f32;
    if t < 0.5 {
        lerp(AMBER, ORANGE, t * 2.0)
    } else {
        lerp(ORANGE, SLATE, (t - 0.5) * 2.0)
    }
}

fn to_ansi256(c: Rgb) -> u8 {
    let level = |v: u8| (f32::from(v) / 255.0 * 5.0).round() as u8;
    16 + 36 * level(c.0) + 6 * level(c.1) + level(c.2)
}

fn escape(mode: Mode, c: Rgb) -> String {
    match mode {
        Mode::TrueColor => format!("\x1b[38;2;{};{};{}m", c.0, c.1, c.2),
        Mode::Ansi256 => format!("\x1b[38;5;{}m", to_ansi256(c)),
        Mode::Plain => String::new(),
    }
}

fn flame_rows() -> Vec<Vec<Cell>> {
    let rows = FLAME.len();
    FLAME
        .iter()
        .enumerate()
        .map(|(r, items)| {
            let mut row: Vec<Cell> = vec![(' ', None); FLAME_WIDTH];
            for &(col, text) in items.iter() {
                let Some(ch) = text.chars().next() else {
                    continue;
                };
                row[col] = match ch {
                    'H' => ('●', Some(AMBER)),
                    'W' => ('●', Some(ORANGE)),
                    'C' => ('●', Some(SLATE)),
                    'h' => ('(', Some(AMBER)),
                    'j' => (')', Some(AMBER)),
                    '│' | '─' | '╱' | '╲' => (ch, Some(EDGE)),
                    _ => (ch, Some(height_color(r, rows))),
                };
            }
            row
        })
        .collect()
}

fn wordmark_rows() -> Vec<Vec<Cell>> {
    (0..5)
        .map(|i| {
            let color = height_color(i, 5);
            let text = LETTERS.iter().map(|l| l[i]).collect::<Vec<_>>().join(" ");
            text.chars().map(|ch| (ch, Some(color))).collect()
        })
        .collect()
}

fn text_cells(text: &str, color: Rgb) -> Vec<Cell> {
    text.chars().map(|ch| (ch, Some(color))).collect()
}

fn push_line(out: &mut Vec<String>, cells: &[Cell], mode: Mode) {
    let left = (WIDTH - cells.len().min(WIDTH)) / 2;
    let mut line = " ".repeat(left);
    let mut current: Option<Rgb> = None;
    let mut painted = false;
    for &(ch, color) in cells {
        if ch == ' ' {
            line.push(' ');
            continue;
        }
        if mode != Mode::Plain && color != current {
            if let Some(c) = color {
                line.push_str(&escape(mode, c));
                painted = true;
            }
            current = color;
        }
        line.push(ch);
    }
    for _ in (left + cells.len())..WIDTH {
        line.push(' ');
    }
    if painted {
        line.push_str(RESET);
    }
    out.push(line);
}

pub fn render_banner(mode: Mode) -> String {
    let mut lines = Vec::new();
    for row in flame_rows() {
        push_line(&mut lines, &row, mode);
    }
    push_line(&mut lines, &[], mode);
    for row in wordmark_rows() {
        push_line(&mut lines, &row, mode);
    }
    push_line(&mut lines, &[], mode);
    push_line(&mut lines, &text_cells(NAME, ORANGE), mode);
    push_line(&mut lines, &text_cells(TAGLINE, SLATE), mode);
    lines.join("\n")
}

pub fn mode_from_env(no_color: bool, colorterm: &str, _term: &str, is_tty: bool) -> Mode {
    if no_color || !is_tty {
        return Mode::Plain;
    }
    let ct = colorterm.trim().to_ascii_lowercase();
    if ct == "truecolor" || ct == "24bit" {
        Mode::TrueColor
    } else {
        Mode::Ansi256
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strip_escapes(s: &str) -> String {
        let mut out = String::new();
        let mut in_esc = false;
        for ch in s.chars() {
            if in_esc {
                if ch == 'm' {
                    in_esc = false;
                }
            } else if ch == '\x1b' {
                in_esc = true;
            } else {
                out.push(ch);
            }
        }
        out
    }

    #[test]
    fn plain_has_no_esc_byte_and_contains_uriel() {
        let banner = render_banner(Mode::Plain);
        assert!(!banner.contains('\x1b'));
        assert!(banner.contains("URIEL"));
        assert!(banner.contains("fewest tokens in, same information out"));
    }

    #[test]
    fn truecolor_has_tier_colors_and_ends_with_reset() {
        let banner = render_banner(Mode::TrueColor);
        assert!(banner.contains("38;2;255;176;32"));
        assert!(banner.contains("38;2;91;107;145"));
        assert!(banner.contains("38;2;232;116;59"));
        assert!(banner.ends_with("\x1b[0m"));
    }

    #[test]
    fn ansi256_has_no_truecolor_sequences() {
        let banner = render_banner(Mode::Ansi256);
        assert!(!banner.contains("38;2;"));
        assert!(banner.contains("38;5;"));
        assert!(banner.ends_with("\x1b[0m"));
    }

    #[test]
    fn every_line_has_the_same_display_width_in_every_mode() {
        for mode in [Mode::TrueColor, Mode::Ansi256, Mode::Plain] {
            let banner = render_banner(mode);
            for line in banner.lines() {
                assert_eq!(strip_escapes(line).chars().count(), WIDTH, "{mode:?}: {line:?}");
            }
        }
    }

    #[test]
    fn flame_holds_exactly_eight_nodes() {
        let banner = render_banner(Mode::Plain);
        assert_eq!(banner.matches('●').count(), 8);
    }

    #[test]
    fn flame_has_between_fourteen_and_eighteen_rows() {
        assert!((14..=18).contains(&FLAME.len()));
    }

    #[test]
    fn stripping_colour_modes_yields_the_plain_banner() {
        let plain = render_banner(Mode::Plain);
        assert_eq!(strip_escapes(&render_banner(Mode::TrueColor)), plain);
        assert_eq!(strip_escapes(&render_banner(Mode::Ansi256)), plain);
    }

    #[test]
    fn mode_from_env_follows_the_policy_table() {
        let table = [
            (true, "truecolor", "xterm", true, Mode::Plain),
            (false, "truecolor", "xterm", false, Mode::Plain),
            (true, "", "", false, Mode::Plain),
            (false, "truecolor", "xterm", true, Mode::TrueColor),
            (false, "24bit", "xterm", true, Mode::TrueColor),
            (false, "TrueColor", "xterm", true, Mode::TrueColor),
            (false, "", "xterm-256color", true, Mode::Ansi256),
            (false, "", "", true, Mode::Ansi256),
            (false, "yes", "dumb", true, Mode::Ansi256),
        ];
        for (no_color, colorterm, term, tty, expected) in table {
            assert_eq!(mode_from_env(no_color, colorterm, term, tty), expected, "{no_color} {colorterm:?} {term:?} {tty}");
        }
    }

    #[test]
    fn banner_is_byte_identical_over_one_hundred_calls() {
        for mode in [Mode::TrueColor, Mode::Ansi256, Mode::Plain] {
            let first = render_banner(mode);
            for _ in 0..100 {
                assert_eq!(render_banner(mode), first);
            }
        }
    }
}
