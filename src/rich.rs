//! The part of [rich](https://github.com/Textualize/rich) (15.0) that
//! brainglobe-atlasapi prints with, rendered as rich renders it without
//! colours (output to a file or pipe): console markup and emoji codes, word
//! wrapping, `Text`, `Pretty`, `Padding`, a `Table` without box and a
//! `Panel`.
//!
//! The layout code is a port of rich's: column measurement and collapsing
//! (`Table._calculate_column_widths`, `_collapse_widths`, `ratio_reduce`),
//! `divide_line`, `Text.wrap` / `truncate` / `Lines.justify`, `Padding` and
//! `Panel.fit`, so the tables and panels come out character for character
//! as Python prints them at the same console width.

use serde_json::{Map, Value};
use unicode_width::UnicodeWidthChar;

use crate::list_atlases::AtlasInfo;

/// The console width rich uses: `$COLUMNS`, else the size of a terminal on
/// stdin / stdout / stderr, else 80.
pub fn console_width() -> usize {
    if let Ok(c) = std::env::var("COLUMNS")
        && !c.is_empty()
        && c.chars().all(|ch| ch.is_ascii_digit())
        && let Ok(w) = c.parse()
    {
        return w;
    }
    terminal_width().unwrap_or(80)
}

#[cfg(unix)]
fn terminal_width() -> Option<usize> {
    #[repr(C)]
    struct Winsize {
        ws_row: u16,
        ws_col: u16,
        ws_xpixel: u16,
        ws_ypixel: u16,
    }
    unsafe extern "C" {
        fn ioctl(fd: i32, request: std::ffi::c_ulong, ...) -> i32;
    }
    #[cfg(any(target_os = "macos", target_os = "ios", target_os = "freebsd"))]
    const TIOCGWINSZ: std::ffi::c_ulong = 0x40087468;
    #[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "freebsd")))]
    const TIOCGWINSZ: std::ffi::c_ulong = 0x5413;
    for fd in [0, 1, 2] {
        let mut ws = Winsize {
            ws_row: 0,
            ws_col: 0,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        // SAFETY: TIOCGWINSZ writes one winsize into `ws`.
        let r = unsafe { ioctl(fd, TIOCGWINSZ, &mut ws as *mut Winsize) };
        if r == 0 && ws.ws_col > 0 {
            return Some(ws.ws_col as usize);
        }
    }
    None
}

#[cfg(not(unix))]
fn terminal_width() -> Option<usize> {
    None
}

/// The width of a string in terminal cells (`cell_len`).
pub fn cell_len(s: &str) -> usize {
    s.chars().map(char_width).sum()
}

fn char_width(c: char) -> usize {
    if c < ' ' || c == '\u{7f}' {
        return 0;
    }
    c.width().unwrap_or(0)
}

/// Crop or pad a string to exactly `width` cells (`set_cell_size`).
pub fn set_cell_size(s: &str, width: usize) -> String {
    let mut out = String::new();
    let mut w = 0;
    for c in s.chars() {
        let cw = char_width(c);
        if w + cw > width {
            break;
        }
        out.push(c);
        w += cw;
    }
    // a double-width character that did not fit leaves one cell
    while w < width {
        out.push(' ');
        w += 1;
    }
    out
}

/// Split a word into lines of at most `width` cells (`chop_cells`).
fn chop_cells(text: &str, width: usize) -> Vec<String> {
    let mut lines = vec![String::new()];
    let mut w = 0;
    for c in text.chars() {
        let cw = char_width(c);
        if w + cw > width && !lines.last().unwrap().is_empty() {
            lines.push(String::new());
            w = 0;
        }
        lines.last_mut().unwrap().push(c);
        w += cw;
    }
    lines
}

/// Replace `:name:` emoji codes (`_emoji_replace`): the regex
/// `(:(\S*?)(?:(?:\-)(emoji|text))?:)`, every match consumed, replaced when
/// `name` (lower-cased) is one of rich's codes.
fn emoji_replace(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(i) = rest.find(':') {
        let after = &rest[i + 1..];
        // the shortest run without whitespace up to the next colon
        let end = after.find(|c: char| c == ':' || c.is_whitespace());
        match end {
            Some(j) if after[j..].starts_with(':') => {
                let code = &after[..j];
                let (name, variant) = match code.rsplit_once('-') {
                    Some((n, v)) if v == "emoji" || v == "text" => (n, Some(v)),
                    _ => (code, None),
                };
                out.push_str(&rest[..i]);
                match crate::emoji_codes::get(&name.to_lowercase()) {
                    Some(e) => {
                        out.push_str(e);
                        match variant {
                            Some("emoji") => out.push('\u{fe0f}'),
                            Some("text") => out.push('\u{fe0e}'),
                            _ => {}
                        }
                    }
                    // not an emoji: the match stays as it was
                    None => out.push_str(&rest[i..i + 1 + j + 1]),
                }
                rest = &after[j + 1..];
            }
            _ => {
                out.push_str(&rest[..i + 1]);
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// The plain text of console markup (`Text.from_markup(...).plain`, emoji
/// codes replaced): tags (`[b]`, `[/b]`, `[#c4c4c4]`, `[red dim]`, ...)
/// are dropped, `\[` is a literal bracket.
pub fn markup_plain(markup: &str) -> String {
    // RE_TAGS: (\\*)\[([a-z#/@][^[]*?)]
    let mut out = String::new();
    let b = markup.as_bytes();
    let mut i = 0;
    let mut pending = String::new();
    while i < markup.len() {
        if b[i] == b'\\' || b[i] == b'[' {
            // count backslashes before a bracket
            let start = i;
            while i < markup.len() && b[i] == b'\\' {
                i += 1;
            }
            if i < markup.len() && b[i] == b'[' {
                let slashes = i - start;
                let tag_end = markup[i + 1..].find(']').map(|k| i + 1 + k);
                let tag_ok = |end: usize| {
                    let body = &markup[i + 1..end];
                    let first = body.chars().next();
                    matches!(first, Some(c) if c.is_ascii_lowercase() || c == '#' || c == '/' || c == '@')
                        && !body.contains('[')
                };
                if let Some(end) = tag_end.filter(|&e| tag_ok(e)) {
                    pending.push_str(&"\\".repeat(slashes / 2));
                    if slashes % 2 == 1 {
                        // escaped: the tag is text
                        pending.push_str(&markup[i..=end]);
                    }
                    i = end + 1;
                    continue;
                }
                pending.push_str(&markup[start..=i]);
                i += 1;
                continue;
            }
            pending.push_str(&markup[start..i]);
            continue;
        }
        let c = markup[i..].chars().next().unwrap();
        pending.push(c);
        i += c.len_utf8();
    }
    out.push_str(&emoji_replace(&pending));
    out
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Justify {
    Default,
    Left,
    Center,
    Right,
    Full,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Overflow {
    Fold,
    Crop,
    Ellipsis,
}

/// `re_word`: `\s*\S+\s*` spans.
fn words(text: &str) -> Vec<(usize, usize)> {
    let mut out = vec![];
    let mut pos = 0;
    let b: Vec<(usize, char)> = text.char_indices().collect();
    let mut k = 0;
    while k < b.len() {
        let start = b[k].0;
        let mut j = k;
        while j < b.len() && b[j].1.is_whitespace() {
            j += 1;
        }
        if j == b.len() {
            break;
        }
        while j < b.len() && !b[j].1.is_whitespace() {
            j += 1;
        }
        while j < b.len() && b[j].1.is_whitespace() {
            j += 1;
        }
        let end = if j < b.len() { b[j].0 } else { text.len() };
        out.push((start, end));
        pos = end;
        k = j;
    }
    let _ = pos;
    out
}

/// Line break offsets for `text` at `width` (`divide_line`).
fn divide_line(text: &str, width: usize, fold: bool) -> Vec<usize> {
    let mut breaks = vec![];
    let mut cell_offset = 0usize;
    for (mut start, end) in words(text) {
        let word = &text[start..end];
        let word_length = cell_len(word.trim_end());
        let remaining = width as isize - cell_offset as isize;
        if remaining >= word_length as isize {
            cell_offset += cell_len(word);
        } else if word_length > width {
            if fold {
                let folded = chop_cells(word, width);
                let n = folded.len();
                for (k, line) in folded.iter().enumerate() {
                    if start > 0 {
                        breaks.push(start);
                    }
                    if k == n - 1 {
                        cell_offset = cell_len(line);
                    } else {
                        start += line.len();
                    }
                }
            } else {
                if start > 0 {
                    breaks.push(start);
                }
                cell_offset = cell_len(word);
            }
        } else if cell_offset > 0 && start > 0 {
            breaks.push(start);
            cell_offset = cell_len(word);
        }
    }
    breaks
}

fn truncate(line: &mut String, max_width: usize, overflow: Overflow, pad: bool) {
    let length = cell_len(line);
    if length > max_width {
        *line = match overflow {
            Overflow::Ellipsis => format!(
                "{}\u{2026}",
                set_cell_size(line, max_width.saturating_sub(1))
            ),
            _ => set_cell_size(line, max_width),
        };
    }
    if pad && length < max_width {
        line.push_str(&" ".repeat(max_width - length));
    }
}

/// `Text.wrap`: the lines of `text` at `width`.
pub fn wrap(
    text: &str,
    width: usize,
    justify: Justify,
    overflow: Overflow,
    no_wrap: bool,
) -> Vec<String> {
    let mut lines = vec![];
    for line in text.split('\n') {
        let line = line.replace('\t', "        ");
        let mut new_lines: Vec<String> = if no_wrap {
            vec![line.clone()]
        } else {
            let offsets = divide_line(&line, width, overflow == Overflow::Fold);
            let mut parts = vec![];
            let mut prev = 0;
            for o in offsets.iter().copied().chain(std::iter::once(line.len())) {
                parts.push(line[prev..o].to_string());
                prev = o;
            }
            // rstrip_end(width): drop trailing whitespace over the width
            for p in parts.iter_mut() {
                let len = p.chars().count();
                if len > width {
                    let excess = len - width;
                    let ws = p.chars().rev().take_while(|c| c.is_whitespace()).count();
                    let crop = ws.min(excess);
                    let keep = len - crop;
                    *p = p.chars().take(keep).collect();
                }
            }
            parts
        };
        match justify {
            Justify::Left => {
                for l in new_lines.iter_mut() {
                    truncate(l, width, overflow, true);
                }
            }
            Justify::Center => {
                for l in new_lines.iter_mut() {
                    *l = l.trim_end().to_string();
                    truncate(l, width, overflow, false);
                    let left = (width.saturating_sub(cell_len(l))) / 2;
                    *l = format!("{}{l}", " ".repeat(left));
                    let right = width.saturating_sub(cell_len(l));
                    l.push_str(&" ".repeat(right));
                }
            }
            Justify::Right => {
                for l in new_lines.iter_mut() {
                    *l = l.trim_end().to_string();
                    truncate(l, width, overflow, false);
                    let left = width.saturating_sub(cell_len(l));
                    *l = format!("{}{l}", " ".repeat(left));
                }
            }
            Justify::Full => {
                let n = new_lines.len();
                for (idx, l) in new_lines.iter_mut().enumerate() {
                    if idx == n - 1 {
                        break;
                    }
                    let words: Vec<String> = l.split(' ').map(str::to_string).collect();
                    let words_size: usize = words.iter().map(|w| cell_len(w)).sum();
                    let mut num_spaces = words.len() - 1;
                    let mut spaces = vec![1usize; num_spaces];
                    let mut index = 0;
                    if !spaces.is_empty() {
                        while words_size + num_spaces < width {
                            let k = spaces.len() - index - 1;
                            spaces[k] += 1;
                            num_spaces += 1;
                            index = (index + 1) % spaces.len();
                        }
                    }
                    let mut s = String::new();
                    for (k, w) in words.iter().enumerate() {
                        s.push_str(w);
                        if k < spaces.len() {
                            s.push_str(&" ".repeat(spaces[k]));
                        }
                    }
                    *l = s;
                }
            }
            Justify::Default => {}
        }
        for l in new_lines.iter_mut() {
            truncate(l, width, overflow, false);
        }
        lines.extend(new_lines);
    }
    lines
}

/// What can be rendered.
#[derive(Debug, Clone)]
pub enum Renderable {
    /// Text (already plain), with its own justify / overflow / no_wrap
    Text {
        plain: String,
        justify: Option<Justify>,
        overflow: Option<Overflow>,
        no_wrap: Option<bool>,
    },
    /// `Pretty(obj)`: the object, shown as rich's `pretty_repr` lays it out
    /// at the width it gets
    Pretty(Value),
    /// `Padding(renderable, (top, right, bottom, left))`, expanding
    Padding(Box<Renderable>, [usize; 4]),
    Table(Box<Table>),
}

impl Renderable {
    pub fn text(plain: &str) -> Renderable {
        Renderable::Text {
            plain: plain.to_string(),
            justify: None,
            overflow: None,
            no_wrap: None,
        }
    }
    /// `console.render_str(markup)` / `Text.from_markup(markup)`.
    pub fn markup(markup: &str) -> Renderable {
        Renderable::text(&markup_plain(markup))
    }
}

/// `ConsoleOptions`, the parts used.
#[derive(Debug, Clone, Copy)]
struct Options {
    max_width: usize,
    justify: Option<Justify>,
    overflow: Option<Overflow>,
    no_wrap: Option<bool>,
}

impl Options {
    fn width(self, w: usize) -> Options {
        Options {
            max_width: w,
            ..self
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Measurement {
    minimum: usize,
    maximum: usize,
}

impl Measurement {
    fn with_maximum(self, w: usize) -> Measurement {
        Measurement {
            minimum: self.minimum.min(w),
            maximum: self.maximum.min(w),
        }
    }
    fn with_minimum(self, w: usize) -> Measurement {
        Measurement {
            minimum: self.minimum.max(w),
            maximum: self.maximum.max(w),
        }
    }
    fn normalize(self) -> Measurement {
        let minimum = self.minimum.min(self.maximum);
        Measurement {
            minimum,
            maximum: minimum.max(self.maximum),
        }
    }
    fn clamp(self, min_width: Option<usize>, max_width: Option<usize>) -> Measurement {
        let mut m = self;
        if let Some(w) = min_width {
            m = m.with_minimum(w);
        }
        if let Some(w) = max_width {
            m = m.with_maximum(w);
        }
        m
    }
}

/// `Measurement.get`.
fn measure(r: &Renderable, options: Options) -> Measurement {
    let max_width = options.max_width;
    if max_width < 1 {
        return Measurement {
            minimum: 0,
            maximum: 0,
        };
    }
    let m = match r {
        Renderable::Text { plain, .. } => {
            let max_text = plain.lines().map(cell_len).max().unwrap_or(0);
            let min_text = plain
                .split_whitespace()
                .map(cell_len)
                .max()
                .unwrap_or(max_text);
            Measurement {
                minimum: min_text,
                maximum: max_text,
            }
        }
        Renderable::Pretty(v) => {
            let w = pretty_repr(v, max_width)
                .lines()
                .map(cell_len)
                .max()
                .unwrap_or(0);
            Measurement {
                minimum: w,
                maximum: w,
            }
        }
        Renderable::Padding(inner, [_, right, _, left]) => {
            let extra = left + right;
            if max_width < extra + 1 {
                Measurement {
                    minimum: max_width,
                    maximum: max_width,
                }
            } else {
                // (rich measures the content at the full width)
                let m = measure(inner, options);
                Measurement {
                    minimum: m.minimum + extra,
                    maximum: m.maximum + extra,
                }
                .with_maximum(max_width)
            }
        }
        Renderable::Table(t) => t.measure(options),
    };
    let m = m.normalize().with_maximum(max_width);
    if m.maximum < 1 {
        return Measurement {
            minimum: 0,
            maximum: 0,
        };
    }
    m.normalize()
}

/// `console.render_lines(renderable, options, pad=True)`: lines of exactly
/// `options.max_width` cells.
fn render_lines(r: &Renderable, options: Options) -> Vec<String> {
    let width = options.max_width;
    let lines = render(r, options);
    lines
        .into_iter()
        .map(|l| set_cell_size(&l, width))
        .collect()
}

/// Render to lines (not padded to the width).
fn render(r: &Renderable, options: Options) -> Vec<String> {
    match r {
        Renderable::Text {
            plain,
            justify,
            overflow,
            no_wrap,
        } => {
            let justify = justify.or(options.justify).unwrap_or(Justify::Default);
            let overflow = overflow.or(options.overflow).unwrap_or(Overflow::Fold);
            let no_wrap = no_wrap.or(options.no_wrap).unwrap_or(false);
            wrap(plain, options.max_width, justify, overflow, no_wrap)
        }
        Renderable::Pretty(v) => {
            let repr = pretty_repr(v, options.max_width);
            let justify = options.justify.unwrap_or(Justify::Default);
            let overflow = options.overflow.unwrap_or(Overflow::Fold);
            wrap(
                &repr,
                options.max_width,
                justify,
                overflow,
                options.no_wrap.unwrap_or(false),
            )
        }
        Renderable::Padding(inner, [top, right, bottom, left]) => {
            let width = options.max_width;
            let inner_lines =
                render_lines(inner, options.width(width.saturating_sub(left + right)));
            let mut out = vec![];
            for _ in 0..*top {
                out.push(" ".repeat(width));
            }
            for l in inner_lines {
                out.push(format!("{}{l}{}", " ".repeat(*left), " ".repeat(*right)));
            }
            for _ in 0..*bottom {
                out.push(" ".repeat(width));
            }
            out
        }
        Renderable::Table(t) => t.render(options),
    }
}

/// A table column (`rich.table.Column`), the options used.
#[derive(Debug, Clone)]
pub struct Column {
    pub header: String,
    pub justify: Justify,
    pub no_wrap: bool,
    pub width: Option<usize>,
    pub min_width: Option<usize>,
    pub max_width: Option<usize>,
    pub overflow: Overflow,
    cells: Vec<Renderable>,
}

impl Column {
    pub fn new(header: &str) -> Column {
        Column {
            header: header.to_string(),
            justify: Justify::Left,
            no_wrap: false,
            width: None,
            min_width: None,
            max_width: None,
            overflow: Overflow::Ellipsis,
            cells: vec![],
        }
    }
    pub fn justify(mut self, j: Justify) -> Column {
        self.justify = j;
        self
    }
    pub fn no_wrap(mut self, v: bool) -> Column {
        self.no_wrap = v;
        self
    }
    pub fn width(mut self, w: usize) -> Column {
        self.width = Some(w);
        self
    }
    pub fn min_width(mut self, w: usize) -> Column {
        self.min_width = Some(w);
        self
    }
    pub fn max_width(mut self, w: usize) -> Column {
        self.max_width = Some(w);
        self
    }
}

/// A table without box (`Table(box=None, ...)`), cell padding (0, 1).
#[derive(Debug, Clone)]
pub struct Table {
    pub columns: Vec<Column>,
    pub title: Option<String>,
    pub show_header: bool,
    rows: usize,
}

impl Table {
    pub fn new(title: Option<&str>) -> Table {
        Table {
            columns: vec![],
            title: title.map(str::to_string),
            show_header: true,
            rows: 0,
        }
    }

    pub fn add_column(&mut self, c: Column) {
        let mut c = c;
        // a column added after rows gets empty cells for them
        c.cells = vec![Renderable::text(""); self.rows];
        self.columns.push(c);
    }

    /// `add_row(*cells)`: missing cells are empty.
    pub fn add_row(&mut self, cells: Vec<Renderable>) {
        let mut it = cells.into_iter();
        for c in self.columns.iter_mut() {
            c.cells
                .push(it.next().unwrap_or_else(|| Renderable::text("")));
        }
        self.rows += 1;
    }

    const PAD: usize = 2;

    fn column_cells(&self, c: &Column) -> Vec<Renderable> {
        let mut v = vec![];
        if self.show_header {
            v.push(Renderable::Padding(
                Box::new(Renderable::markup(&c.header)),
                [0, 1, 0, 1],
            ));
        }
        for cell in &c.cells {
            v.push(Renderable::Padding(Box::new(cell.clone()), [0, 1, 0, 1]));
        }
        v
    }

    fn measure_column(&self, c: &Column, options: Options) -> Measurement {
        let max_width = options.max_width;
        if max_width < 1 {
            return Measurement {
                minimum: 0,
                maximum: 0,
            };
        }
        if let Some(w) = c.width {
            return Measurement {
                minimum: w + Self::PAD,
                maximum: w + Self::PAD,
            }
            .with_maximum(max_width);
        }
        let ms: Vec<Measurement> = self
            .column_cells(c)
            .iter()
            .map(|r| measure(r, options))
            .collect();
        Measurement {
            minimum: ms.iter().map(|m| m.minimum).max().unwrap_or(1),
            maximum: ms.iter().map(|m| m.maximum).max().unwrap_or(max_width),
        }
        .with_maximum(max_width)
        .clamp(
            c.min_width.map(|w| w + Self::PAD),
            c.max_width.map(|w| w + Self::PAD),
        )
    }

    fn calculate_column_widths(&self, options: Options) -> Vec<usize> {
        let max_width = options.max_width;
        let ranges: Vec<Measurement> = self
            .columns
            .iter()
            .map(|c| self.measure_column(c, options))
            .collect();
        let mut widths: Vec<usize> = ranges
            .iter()
            .map(|r| if r.maximum == 0 { 1 } else { r.maximum })
            .collect();
        let mut table_width: usize = widths.iter().sum();
        if table_width > max_width {
            let wrapable: Vec<bool> = self
                .columns
                .iter()
                .map(|c| c.width.is_none() && !c.no_wrap)
                .collect();
            widths = collapse_widths(widths, &wrapable, max_width);
            table_width = widths.iter().sum();
            if table_width > max_width {
                let excess = table_width - max_width;
                widths = ratio_reduce(excess, &vec![1; widths.len()], &widths.clone(), &widths);
            }
            widths = self
                .columns
                .iter()
                .zip(&widths)
                .map(|(c, &w)| self.measure_column(c, options.width(w)).maximum)
                .collect();
        }
        widths
    }

    fn measure(&self, options: Options) -> Measurement {
        let max_width: usize = self.calculate_column_widths(options).iter().sum();
        let ms: Vec<Measurement> = self
            .columns
            .iter()
            .map(|c| self.measure_column(c, options.width(max_width)))
            .collect();
        Measurement {
            minimum: ms.iter().map(|m| m.minimum).sum(),
            maximum: ms.iter().map(|m| m.maximum).sum(),
        }
    }

    fn render(&self, options: Options) -> Vec<String> {
        let mut out = vec![];
        if self.columns.is_empty() {
            out.push(String::new());
            return out;
        }
        let widths = self.calculate_column_widths(options);
        let table_width: usize = widths.iter().sum();
        if let Some(title) = &self.title {
            let o = Options {
                max_width: table_width,
                justify: Some(Justify::Center),
                ..options
            };
            out.extend(render_lines(&Renderable::markup(title), o));
        }
        let cells: Vec<Vec<Renderable>> =
            self.columns.iter().map(|c| self.column_cells(c)).collect();
        let nrows = cells[0].len();
        for row in 0..nrows {
            let header_row = row == 0 && self.show_header;
            let rendered: Vec<Vec<String>> = self
                .columns
                .iter()
                .zip(&widths)
                .zip(&cells)
                .map(|((c, &w), cs)| {
                    let o = Options {
                        max_width: w,
                        justify: Some(c.justify),
                        overflow: Some(c.overflow),
                        no_wrap: Some(c.no_wrap),
                    };
                    render_lines(&cs[row], o)
                })
                .collect();
            let height = rendered.iter().map(Vec::len).max().unwrap_or(1).max(1);
            let aligned: Vec<Vec<String>> = rendered
                .into_iter()
                .zip(&widths)
                .map(|(mut lines, &w)| {
                    let blank = " ".repeat(w);
                    let missing = height - lines.len();
                    if header_row {
                        // headers align to the bottom
                        let mut v = vec![blank.clone(); missing];
                        v.append(&mut lines);
                        v
                    } else {
                        lines.extend(std::iter::repeat_n(blank, missing));
                        lines
                    }
                })
                .collect();
            for line_no in 0..height {
                out.push(aligned.iter().map(|c| c[line_no].as_str()).collect());
            }
        }
        out
    }
}

/// `Table._collapse_widths`.
fn collapse_widths(mut widths: Vec<usize>, wrapable: &[bool], max_width: usize) -> Vec<usize> {
    let mut total: usize = widths.iter().sum();
    let mut excess = total as isize - max_width as isize;
    if wrapable.iter().any(|&w| w) {
        while total > 0 && excess > 0 {
            let max_column = widths
                .iter()
                .zip(wrapable)
                .filter(|(_, a)| **a)
                .map(|(&w, _)| w)
                .max()
                .unwrap_or(0);
            let second = widths
                .iter()
                .zip(wrapable)
                .map(|(&w, &a)| if a && w != max_column { w } else { 0 })
                .max()
                .unwrap_or(0);
            let diff = max_column - second;
            let ratios: Vec<usize> = widths
                .iter()
                .zip(wrapable)
                .map(|(&w, &a)| usize::from(w == max_column && a))
                .collect();
            if !ratios.iter().any(|&r| r > 0) || diff == 0 {
                break;
            }
            let max_reduce = vec![(excess as usize).min(diff); widths.len()];
            widths = ratio_reduce(excess as usize, &ratios, &max_reduce, &widths);
            total = widths.iter().sum();
            excess = total as isize - max_width as isize;
        }
    }
    widths
}

/// Python's `round()` of a non-negative ratio: half to even.
fn py_round(num: usize, den: usize) -> usize {
    let q = num / den;
    let r = num % den;
    match (2 * r).cmp(&den) {
        std::cmp::Ordering::Less => q,
        std::cmp::Ordering::Greater => q + 1,
        std::cmp::Ordering::Equal => q + (q % 2),
    }
}

/// `ratio_reduce`.
fn ratio_reduce(
    total: usize,
    ratios: &[usize],
    maximums: &[usize],
    values: &[usize],
) -> Vec<usize> {
    let ratios: Vec<usize> = ratios
        .iter()
        .zip(maximums)
        .map(|(&r, &m)| if m > 0 { r } else { 0 })
        .collect();
    let mut total_ratio: usize = ratios.iter().sum();
    if total_ratio == 0 {
        return values.to_vec();
    }
    let mut total_remaining = total;
    let mut result = vec![];
    for ((&ratio, &maximum), &value) in ratios.iter().zip(maximums).zip(values) {
        if ratio > 0 && total_ratio > 0 {
            let distributed = maximum.min(py_round(ratio * total_remaining, total_ratio));
            result.push(value.saturating_sub(distributed));
            total_remaining -= distributed;
            total_ratio -= ratio;
        } else {
            result.push(value);
        }
    }
    result
}

const ROUNDED: [&str; 6] = [
    "\u{256d}", "\u{2500}", "\u{256e}", "\u{2502}", "\u{2570}", "\u{256f}",
];

/// `Panel.fit(renderable, title=..., width=...)` (rounded box, padding
/// (0, 1)), rendered at `console_width`.
pub fn panel_fit(
    renderable: Renderable,
    title: Option<&str>,
    width: Option<usize>,
    console_width: usize,
) -> String {
    let inner = Renderable::Padding(Box::new(renderable), [0, 1, 0, 1]);
    let options = Options {
        max_width: console_width,
        justify: None,
        overflow: None,
        no_wrap: None,
    };
    let width = match width {
        Some(w) => w.min(console_width),
        None => console_width,
    };
    let mut child_width = measure(&inner, options.width(width.saturating_sub(2))).maximum;
    let title = title.map(|t| format!(" {} ", markup_plain(t).replace('\n', " ")));
    if let Some(t) = &title {
        child_width = (console_width - 2).min(child_width.max(cell_len(t) + 2));
    }
    let width = child_width + 2;
    let lines = render_lines(&inner, options.width(child_width));
    let [tl, top, tr, v, bl, br] = ROUNDED;
    let mut out = String::new();
    match &title {
        Some(t) if width > 4 => {
            let mut t = t.clone();
            truncate(&mut t, width - 4, Overflow::Ellipsis, false);
            let excess = (width - 4).saturating_sub(cell_len(&t));
            let left = excess / 2;
            out.push_str(&format!(
                "{tl}{top}{}{t}{}{top}{tr}\n",
                top.repeat(left),
                top.repeat(excess - left)
            ));
        }
        _ => out.push_str(&format!("{tl}{}{tr}\n", top.repeat(width - 2))),
    }
    for l in lines {
        out.push_str(&format!("{v}{l}{v}\n"));
    }
    out.push_str(&format!("{bl}{}{br}\n", top.repeat(width - 2)));
    out
}

/// `rich.print(markup)`: the plain text, word-wrapped at the console width.
pub fn rprint_string(markup: &str, width: usize) -> String {
    let plain = markup_plain(markup);
    let mut s = wrap(&plain, width, Justify::Default, Overflow::Fold, false).join("\n");
    s.push('\n');
    s
}

/// `rich.print(markup)` to stdout.
pub fn rprint(markup: &str) {
    print!("{}", rprint_string(markup, console_width()));
}

/// rich's `pretty_repr(obj, max_width)`: the one-line repr, with every
/// container that does not fit expanded one item per line (indent 4), as
/// `rich.pretty` does.
pub fn pretty_repr(v: &Value, max_width: usize) -> String {
    struct Line<'a> {
        whitespace: String,
        text: String,
        node: Option<&'a Value>,
        suffix: &'static str,
        last: bool,
        expanded: bool,
    }
    fn expandable(v: &Value) -> bool {
        match v {
            Value::Array(a) => !a.is_empty(),
            Value::Object(m) => !m.is_empty(),
            _ => false,
        }
    }
    fn render_line(l: &Line) -> String {
        let node = if l.expanded {
            String::new()
        } else {
            l.node.map(py_repr_value).unwrap_or_default()
        };
        if l.last {
            format!("{}{}{node}", l.whitespace, l.text)
        } else {
            format!("{}{}{node}{}", l.whitespace, l.text, l.suffix)
        }
    }
    let mut lines = vec![Line {
        whitespace: String::new(),
        text: String::new(),
        node: Some(v),
        suffix: "",
        last: false,
        expanded: false,
    }];
    let mut i = 0;
    while i < lines.len() {
        let l = &lines[i];
        let fits = {
            let start = cell_len(&l.whitespace) + cell_len(&l.text) + cell_len(l.suffix);
            start + l.node.map(|n| cell_len(&py_repr_value(n))).unwrap_or(0) <= max_width
        };
        if l.node.is_some_and(expandable) && !l.expanded && !fits {
            let node = l.node.unwrap();
            let (open, close) = match node {
                Value::Array(_) => ("[", "]"),
                _ => ("{", "}"),
            };
            let ws = l.whitespace.clone();
            let child_ws = format!("{ws}    ");
            let (suffix, last) = (l.suffix, l.last);
            let mut new = vec![Line {
                whitespace: ws.clone(),
                text: format!("{}{open}", l.text),
                node: None,
                suffix: "",
                last: false,
                expanded: true,
            }];
            let children: Vec<(String, &Value)> = match node {
                Value::Array(a) => a.iter().map(|c| (String::new(), c)).collect(),
                Value::Object(m) => m
                    .iter()
                    .map(|(k, c)| (format!("{}: ", crate::utils::py_repr_str(k)), c))
                    .collect(),
                _ => vec![],
            };
            let n = children.len();
            for (k, (key, c)) in children.into_iter().enumerate() {
                new.push(Line {
                    whitespace: child_ws.clone(),
                    text: key,
                    node: Some(c),
                    suffix: ",",
                    last: k == n - 1,
                    expanded: false,
                });
            }
            new.push(Line {
                whitespace: ws,
                text: close.to_string(),
                node: None,
                suffix,
                last,
                expanded: true,
            });
            lines.splice(i..=i, new);
        }
        i += 1;
    }
    lines.iter().map(render_line).collect::<Vec<_>>().join("\n")
}

/// Python's `repr` of a JSON value, as `Pretty` shows a short one.
pub fn py_repr_value(v: &Value) -> String {
    match v {
        Value::Null => "None".into(),
        Value::Bool(b) => if *b { "True" } else { "False" }.into(),
        Value::Number(n) if n.is_f64() => crate::utils::py_float_repr(n.as_f64().unwrap()),
        Value::Number(n) => n.to_string(),
        Value::String(s) => crate::utils::py_repr_str(s),
        Value::Array(a) => format!(
            "[{}]",
            a.iter().map(py_repr_value).collect::<Vec<_>>().join(", ")
        ),
        Value::Object(m) => format!(
            "{{{}}}",
            m.iter()
                .map(|(k, v)| format!("{}: {}", crate::utils::py_repr_str(k), py_repr_value(v)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// `str(atlas)`: the metadata table in a panel (`_rich_atlas_metadata`), at
/// the console width.
pub fn atlas_metadata_panel(
    atlas_name: &str,
    metadata: &Map<String, Value>,
) -> crate::Result<String> {
    atlas_metadata_panel_at(atlas_name, metadata, console_width())
}

/// [`atlas_metadata_panel`] at a given console width. Fails as Python does
/// on metadata it cannot format (a `null` name or link: a `TypeError`).
pub fn atlas_metadata_panel_at(
    atlas_name: &str,
    metadata: &Map<String, Value>,
    width: usize,
) -> crate::Result<String> {
    // `f"{value}"`: str() of the value
    let s = |k: &str| -> crate::Result<String> {
        match metadata.get(k) {
            Some(Value::String(s)) => Ok(s.clone()),
            Some(v) => Ok(py_repr_value(v)),
            None => Err(crate::Error::key(k)),
        }
    };
    let type_name = |v: Option<&Value>| match v {
        Some(Value::Null) => "NoneType",
        Some(Value::Bool(_)) => "bool",
        Some(Value::Number(n)) if n.is_f64() => "float",
        Some(Value::Number(_)) => "int",
        Some(Value::Array(_)) => "list",
        Some(Value::Object(_)) => "dict",
        _ => "str",
    };
    if !matches!(metadata.get("name"), Some(Value::String(_))) {
        // metadata["name"] + f" ..."
        metadata
            .get("name")
            .ok_or_else(|| crate::Error::key("name"))?;
        return Err(crate::Error::Type(format!(
            "unsupported operand type(s) for +: '{}' and 'str'",
            type_name(metadata.get("name"))
        )));
    }
    let (name, version, species, citation) =
        (s("name")?, s("version")?, s("species")?, s("citation")?);
    let link = match metadata.get("atlas_link") {
        Some(Value::String(l)) => l.clone(),
        Some(v) => {
            return Err(crate::Error::Type(format!(
                "argument of type '{}' is not iterable",
                type_name(Some(v))
            )));
        }
        None => return Err(crate::Error::key("atlas_link")),
    };
    let orientation = s("orientation")?;
    let mut title: String = atlas_name.replace('_', " ");
    // str.capitalize(): first character upper, the rest lower
    title = {
        let mut c = title.chars();
        match c.next() {
            Some(f) => f.to_uppercase().collect::<String>() + &c.as_str().to_lowercase(),
            None => String::new(),
        }
    };
    let mut tb = Table::new(Some(&rich_escape(&title)));
    tb.add_column(
        Column::new("")
            .justify(Justify::Right)
            .min_width(8)
            .max_width(40),
    );
    tb.add_column(Column::new("").min_width(20).max_width(48));
    let row = |a: &str, b: Renderable| vec![Renderable::markup(a), b];
    tb.add_row(row(
        "name:",
        Renderable::markup(&format!("{name} [#A9A9A9](v{version})")),
    ));
    tb.add_row(row(
        "species:",
        Renderable::markup(&format!("[i]{species}")),
    ));
    tb.add_row(row("citation:", Renderable::markup(&citation)));
    tb.add_row(row("link:", Renderable::markup(&link)));
    tb.add_row(vec![Renderable::markup("")]);
    tb.add_row(row(
        "orientation:",
        Renderable::markup(&format!("[bold]{orientation}")),
    ));
    let pretty = |k: &str| Renderable::Pretty(metadata.get(k).cloned().unwrap_or(Value::Null));
    tb.add_row(row("symmetric:", pretty("symmetric")));
    tb.add_row(row("resolution:", pretty("resolution")));
    tb.add_row(row("shape:", pretty("shape")));
    Ok(panel_fit(
        Renderable::Table(Box::new(tb)),
        None,
        None,
        width,
    ))
}

/// A title is markup in rich; plain text here.
fn rich_escape(s: &str) -> String {
    s.replace('[', "\\[")
}

/// `show_atlases`' table in its panel, at the console width.
pub fn atlases_table(
    rows: &[(String, AtlasInfo)],
    show_local_path: bool,
    table_width: usize,
) -> String {
    atlases_table_at(rows, show_local_path, table_width, console_width())
}

/// [`atlases_table`] at a given console width.
pub fn atlases_table_at(
    rows: &[(String, AtlasInfo)],
    show_local_path: bool,
    table_width: usize,
    console_width: usize,
) -> String {
    let mut table = Table::new(None);
    table.add_column(Column::new("Name").no_wrap(true).width(32));
    table.add_column(Column::new("Downloaded").justify(Justify::Center));
    table.add_column(Column::new("Updated").justify(Justify::Center));
    table.add_column(Column::new("Local version").justify(Justify::Center));
    table.add_column(Column::new("Latest version").justify(Justify::Center));
    if show_local_path {
        table.add_column(Column::new("Local path"));
    }
    for (name, info) in rows {
        let (downloaded, updated) = if info.downloaded {
            (
                "[green]:heavy_check_mark:[/green]".to_string(),
                if info.updated == Some(true) {
                    "[green]:heavy_check_mark:[/green]".to_string()
                } else {
                    "[red dim]x".to_string()
                },
            )
        } else {
            (String::new(), String::new())
        };
        let version = match &info.version {
            Some(v) if !v.is_empty() && !v.contains('-') => format!("[#c4c4c4]{v}"),
            _ => String::new(),
        };
        let mut row = vec![
            Renderable::markup(&format!("[bold]{name}")),
            Renderable::markup(&downloaded),
            Renderable::markup(&updated),
            Renderable::markup(&version),
            Renderable::markup(&format!("[#c4c4c4]{}", info.latest_version)),
        ];
        if show_local_path {
            row.push(Renderable::markup(&info.local));
        }
        table.add_row(row);
    }
    panel_fit(
        Renderable::Table(Box::new(table)),
        Some("Brainglobe Atlases"),
        Some(table_width),
        console_width,
    )
}
