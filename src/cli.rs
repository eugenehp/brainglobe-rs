//! The `brainglobe` command (`brainglobe_atlasapi.cli`): `list`, `install`,
//! `update` and `config`.
//!
//! Python parses it with click (8.5); [`parse_args`] is the part of click's
//! parser this one command uses, with its messages, help page (the docstring
//! re-wrapped as click and `textwrap` wrap it) and exit codes.

#[cfg(all(feature = "remote", not(target_arch = "wasm32")))]
use crate::error::Error;

/// The command's docstring, as click cleans it (`inspect.cleandoc`).
pub const HELP: &str = "Command line dispatcher. Given a command line call to `brainglobe`\nit calls the correct function, depending on which `command` was passed.\n\nParameters\n----------\ncommand : str\n    Name of the command:\n    - list: list available atlases\n    - install: install new atlas\n    - update: update an installed atlas\n    - config: modify config\nshow : bool\n    If True when using `list` shows the local path of installed\n    atlases and when using 'config' it prints the modify config results.\natlas_name : str\n    Used with `update` and `install`, name of the atlas to\n    install\nforce : bool, optional\n    Used with `update`. If True it forces the update.\n    Defaults to False.";

const USAGE: &str = "Usage: brainglobe [OPTIONS] COMMAND";

/// The parsed arguments (`bg_cli`'s parameters).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Args {
    pub command: String,
    pub show: bool,
    pub atlas_name: Option<String>,
    pub force: bool,
    pub key: Option<String>,
    pub value: Option<String>,
}

/// What parsing ended with.
#[derive(Debug, Clone, PartialEq)]
pub enum Parsed {
    Run(Args),
    /// print this to stdout and exit 0 (`-h`)
    Help(String),
    /// print this to stderr and exit with the code (a usage error)
    Error(String, i32),
}

/// `shutil.get_terminal_size().columns`: `$COLUMNS`, else stdout's
/// terminal, else 80.
fn terminal_columns() -> usize {
    if let Ok(c) = std::env::var("COLUMNS")
        && let Ok(n) = c.parse::<usize>()
        && n > 0
    {
        return n;
    }
    stdout_width().unwrap_or(80)
}

#[cfg(unix)]
fn stdout_width() -> Option<usize> {
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
    let mut ws = Winsize {
        ws_row: 0,
        ws_col: 0,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: TIOCGWINSZ writes one winsize into `ws`.
    let r = unsafe { ioctl(1, TIOCGWINSZ, &mut ws as *mut Winsize) };
    (r == 0 && ws.ws_col > 0).then_some(ws.ws_col as usize)
}

#[cfg(not(unix))]
fn stdout_width() -> Option<usize> {
    None
}

/// click's help width: `max(min(columns, 80) - 2, 50)`.
pub fn help_width() -> usize {
    (terminal_columns().min(80).saturating_sub(2)).max(50)
}

/// Python's `textwrap.TextWrapper(width, initial_indent, subsequent_indent,
/// replace_whitespace=False).fill(text)`.
pub fn textwrap_fill(
    text: &str,
    width: usize,
    initial_indent: &str,
    subsequent_indent: &str,
) -> String {
    let chunks = split_chunks(text);
    let mut chunks: Vec<String> = chunks.into_iter().rev().collect();
    let mut lines: Vec<String> = vec![];
    while !chunks.is_empty() {
        let mut cur_line: Vec<String> = vec![];
        let mut cur_len = 0usize;
        let indent = if lines.is_empty() {
            initial_indent
        } else {
            subsequent_indent
        };
        let width_left = width.saturating_sub(indent.chars().count());
        // drop leading whitespace, except at the very start of the text
        if chunks.last().unwrap().trim().is_empty() && !lines.is_empty() {
            chunks.pop();
        }
        while let Some(c) = chunks.last() {
            let l = c.chars().count();
            if cur_len + l <= width_left {
                cur_len += l;
                cur_line.push(chunks.pop().unwrap());
            } else {
                break;
            }
        }
        if let Some(c) = chunks.last()
            && c.chars().count() > width_left
        {
            // _handle_long_word: break it
            let space_left = if width_left < 1 {
                1
            } else {
                width_left - cur_len
            };
            let chunk: Vec<char> = c.chars().collect();
            let mut end = space_left;
            if chunk.len() > space_left
                && let Some(h) = chunk[..space_left].iter().rposition(|&ch| ch == '-')
                && h > 0
                && chunk[..h].iter().any(|&ch| ch != '-')
            {
                end = h + 1;
            }
            if cur_line.is_empty() || space_left > 0 {
                let head: String = chunk[..end.min(chunk.len())].iter().collect();
                let tail: String = chunk[end.min(chunk.len())..].iter().collect();
                cur_line.push(head);
                let last = chunks.len() - 1;
                chunks[last] = tail;
                if chunks[last].is_empty() {
                    chunks.pop();
                }
            }
        }
        if cur_line.last().is_some_and(|c| c.trim().is_empty()) {
            cur_line.pop();
        }
        if !cur_line.is_empty() {
            lines.push(format!("{indent}{}", cur_line.concat()));
        }
    }
    lines.join("\n")
}

/// `TextWrapper._split` with `break_on_hyphens`: whitespace runs, and words
/// (split after a hyphen between letters, as `wordsep_re` does).
fn split_chunks(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    // `[^\d\W]`: a word character that is not a digit (underscore included)
    let lt = |k: usize| {
        chars
            .get(k)
            .is_some_and(|&c| (c.is_alphanumeric() || c == '_') && !c.is_numeric())
    };
    let is = |k: usize, ch: char| chars.get(k) == Some(&ch);
    let mut out = vec![];
    let mut i = 0;
    while i < chars.len() {
        let start = i;
        if chars[i].is_whitespace() {
            while i < chars.len() && chars[i].is_whitespace() {
                i += 1;
            }
            out.push(chars[start..i].iter().collect());
            continue;
        }
        // `\S+? -(?: (?<=lt{2}-) | (?<=lt-lt-)) (?= lt -? lt)`: a word ends
        // after a hyphen between letters
        while i < chars.len() && !chars[i].is_whitespace() {
            if chars[i] == '-' && i > start {
                let behind = (i >= 2 && lt(i - 1) && lt(i - 2))
                    || (i >= 3 && lt(i - 1) && is(i - 2, '-') && lt(i - 3));
                let ahead = lt(i + 1) && (lt(i + 2) || (is(i + 2, '-') && lt(i + 3)));
                if behind && ahead {
                    i += 1;
                    break;
                }
            }
            i += 1;
        }
        out.push(chars[start..i].iter().collect());
    }
    out
}

/// click's `wrap_text(..., preserve_paragraphs=True)`.
pub fn wrap_text(
    text: &str,
    width: usize,
    initial_indent: &str,
    subsequent_indent: &str,
) -> String {
    let text = text.replace('\t', "        ");
    let mut paragraphs: Vec<(usize, bool, String)> = vec![];
    let mut buf: Vec<String> = vec![];
    let mut indent: Option<usize> = None;
    let flush =
        |buf: &mut Vec<String>, indent: Option<usize>, p: &mut Vec<(usize, bool, String)>| {
            if buf.is_empty() {
                return;
            }
            if buf[0].trim() == "\u{8}" {
                p.push((indent.unwrap_or(0), true, buf[1..].join("\n")));
            } else {
                p.push((indent.unwrap_or(0), false, buf.join(" ")));
            }
            buf.clear();
        };
    for line in text.lines() {
        if line.is_empty() {
            flush(&mut buf, indent, &mut paragraphs);
            indent = None;
        } else {
            let mut line = line.to_string();
            if indent.is_none() {
                let orig = line.chars().count();
                line = line.trim_start().to_string();
                indent = Some(orig - line.chars().count());
            }
            buf.push(line);
        }
    }
    flush(&mut buf, indent, &mut paragraphs);
    paragraphs
        .into_iter()
        .map(|(ind, raw, t)| {
            let extra = " ".repeat(ind);
            let ii = format!("{initial_indent}{extra}");
            let si = format!("{subsequent_indent}{extra}");
            if raw {
                t.lines()
                    .map(|l| format!("{ii}{l}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            } else {
                textwrap_fill(&t, width, &ii, &si)
            }
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// The help page (`brainglobe -h`).
pub fn help_page(width: usize) -> String {
    let mut s = format!("{USAGE}\n\n");
    s.push_str(&wrap_text(HELP, width, "  ", "  "));
    s.push_str("\n\nOptions:\n");
    let rows: [(&str, &str); 6] = [
        ("-s, --show", ""),
        ("-a, --atlas_name TEXT", ""),
        ("-f, --force", ""),
        ("-k, --key TEXT", ""),
        ("-v, --value TEXT", ""),
        ("-h, --help", "Show this message and exit."),
    ];
    let first_col = rows
        .iter()
        .map(|(a, _)| a.chars().count())
        .max()
        .unwrap()
        .min(30)
        + 2;
    for (term, help) in rows {
        if help.is_empty() {
            s.push_str(&format!("  {term}\n"));
            continue;
        }
        let pad = first_col.saturating_sub(term.chars().count());
        let text_width = (width.saturating_sub(first_col + 2)).max(10);
        let wrapped = textwrap_fill(help, text_width, "", "");
        let mut lines = wrapped.lines();
        s.push_str(&format!(
            "  {term}{}{}\n",
            " ".repeat(pad),
            lines.next().unwrap_or("")
        ));
        for l in lines {
            s.push_str(&format!("{}{l}\n", " ".repeat(first_col + 2)));
        }
    }
    s
}

/// click's `BadOptionUsage`: shown without the usage lines.
fn option_error(msg: &str) -> Parsed {
    Parsed::Error(format!("Error: {msg}\n"), 2)
}

fn usage_error(msg: &str) -> Parsed {
    Parsed::Error(
        format!("{USAGE}\nTry 'brainglobe --help' for help.\n\nError: {msg}\n"),
        2,
    )
}

/// `difflib.SequenceMatcher(None, a, b).ratio()`.
fn ratio(a: &str, b: &str) -> f64 {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    fn matches(a: &[char], b: &[char]) -> usize {
        // longest common block (first one found, as find_longest_match),
        // then recurse on both sides
        let (mut bi, mut bj, mut bk) = (0, 0, 0);
        let mut prev = vec![0usize; b.len() + 1];
        for i in 0..a.len() {
            let mut cur = vec![0usize; b.len() + 1];
            for j in 0..b.len() {
                if a[i] == b[j] {
                    let k = prev[j] + 1;
                    cur[j + 1] = k;
                    if k > bk {
                        bi = i + 1 - k;
                        bj = j + 1 - k;
                        bk = k;
                    }
                }
            }
            prev = cur;
        }
        if bk == 0 {
            return 0;
        }
        bk + matches(&a[..bi], &b[..bj]) + matches(&a[bi + bk..], &b[bj + bk..])
    }
    let total = a.len() + b.len();
    if total == 0 {
        return 1.0;
    }
    2.0 * matches(&a, &b) as f64 / total as f64
}

/// `difflib.get_close_matches(word, possibilities)` (n=3, cutoff=0.6).
fn close_matches(word: &str, possibilities: &[&str]) -> Vec<String> {
    let mut scored: Vec<(f64, &str)> = possibilities
        .iter()
        .map(|p| (ratio(p, word), *p))
        .filter(|(r, _)| *r >= 0.6)
        .collect();
    // heapq.nlargest on (score, x): highest score, then largest string
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap().then(b.1.cmp(a.1)));
    scored
        .into_iter()
        .take(3)
        .map(|(_, p)| p.to_string())
        .collect()
}

const LONG_OPTS: [&str; 6] = [
    "--show",
    "--atlas_name",
    "--force",
    "--key",
    "--value",
    "--help",
];

fn no_such_option(opt: &str, possibilities: Vec<String>) -> Parsed {
    let mut msg = format!("No such option '{opt}'.");
    match possibilities.len() {
        0 => {}
        1 => msg.push_str(&format!(" Did you mean '{}'?", possibilities[0])),
        _ => {
            let mut p = possibilities;
            p.sort();
            msg.push_str(&format!(" (Possible options: {})", p.join(", ")));
        }
    }
    usage_error(&msg)
}

/// Parse the arguments (after the program name) as click does.
pub fn parse_args(argv: &[String]) -> Parsed {
    #[derive(Clone, Copy)]
    enum Opt {
        Show,
        AtlasName,
        Force,
        Key,
        Value,
        Help,
    }
    let long = |s: &str| match s {
        "--show" => Some(Opt::Show),
        "--atlas_name" => Some(Opt::AtlasName),
        "--force" => Some(Opt::Force),
        "--key" => Some(Opt::Key),
        "--value" => Some(Opt::Value),
        "--help" => Some(Opt::Help),
        _ => None,
    };
    let short = |c: char| match c {
        's' => Some(Opt::Show),
        'a' => Some(Opt::AtlasName),
        'f' => Some(Opt::Force),
        'k' => Some(Opt::Key),
        'v' => Some(Opt::Value),
        'h' => Some(Opt::Help),
        _ => None,
    };
    let takes_value = |o: Opt| matches!(o, Opt::AtlasName | Opt::Key | Opt::Value);

    let mut args = Args::default();
    let mut help = false;
    let mut positional: Vec<String> = vec![];
    let set = |o: Opt, v: Option<String>, args: &mut Args, help: &mut bool| match o {
        Opt::Show => args.show = true,
        Opt::Force => args.force = true,
        Opt::Help => *help = true,
        Opt::AtlasName => args.atlas_name = v,
        Opt::Key => args.key = v,
        Opt::Value => args.value = v,
    };

    let mut i = 0;
    while i < argv.len() {
        let arg = &argv[i];
        i += 1;
        if arg == "--" {
            positional.extend(argv[i..].iter().cloned());
            break;
        }
        if arg.starts_with('-') && arg.len() > 1 {
            let (name, explicit) = match arg.split_once('=') {
                Some((n, v)) => (n.to_string(), Some(v.to_string())),
                None => (arg.clone(), None),
            };
            if let Some(o) = long(&name) {
                if takes_value(o) {
                    let v = match explicit {
                        Some(v) => v,
                        None => {
                            if i >= argv.len() {
                                return option_error(&format!(
                                    "Option '{name}' requires an argument."
                                ));
                            }
                            i += 1;
                            argv[i - 1].clone()
                        }
                    };
                    set(o, Some(v), &mut args, &mut help);
                } else {
                    if explicit.is_some() {
                        return option_error(&format!("Option '{name}' does not take a value."));
                    }
                    set(o, None, &mut args, &mut help);
                }
                continue;
            }
            if arg.starts_with("--") {
                let poss = close_matches(&name, &LONG_OPTS);
                return no_such_option(&name, poss);
            }
            // a group of short options
            let chars: Vec<char> = arg[1..].chars().collect();
            let mut k = 0;
            while k < chars.len() {
                let c = chars[k];
                k += 1;
                let Some(o) = short(c) else {
                    return no_such_option(&format!("-{c}"), vec![]);
                };
                if takes_value(o) {
                    let v = if k < chars.len() {
                        let v: String = chars[k..].iter().collect();
                        k = chars.len();
                        v
                    } else {
                        if i >= argv.len() {
                            return option_error(&format!("Option '-{c}' requires an argument."));
                        }
                        i += 1;
                        argv[i - 1].clone()
                    };
                    set(o, Some(v), &mut args, &mut help);
                } else {
                    set(o, None, &mut args, &mut help);
                }
            }
            continue;
        }
        positional.push(arg.clone());
    }
    if help {
        return Parsed::Help(help_page(help_width()));
    }
    match positional.len() {
        0 => usage_error("Missing argument 'COMMAND'."),
        1 => {
            args.command = positional.remove(0);
            Parsed::Run(args)
        }
        _ => {
            let extra = &positional[1..];
            let s = if extra.len() == 1 { "" } else { "s" };
            usage_error(&format!(
                "Got unexpected extra argument{s} ({})",
                extra.join(" ")
            ))
        }
    }
}

/// Run a parsed command (`bg_cli`); what it prints goes to stdout.
#[cfg(all(feature = "remote", not(target_arch = "wasm32")))]
pub fn run(args: &Args) -> crate::Result<()> {
    match args.command.as_str() {
        "list" => crate::list_atlases::show_atlases(args.show, 88),
        "install" => {
            let Some(name) = &args.atlas_name else {
                return Err(Error::value(
                    "No atlas named passed with command \"install\". Use the \"-a\"                                argument to pass an atlas name",
                ));
            };
            crate::update_atlases::install_atlas(name)
        }
        "update" => {
            let Some(name) = &args.atlas_name else {
                return Err(Error::value(
                    "No atlas named passed with command \"update\". Use the \"-a\"                                argument to pass an atlas name",
                ));
            };
            crate::update_atlases::update_atlas(name, args.force)
        }
        "config" => {
            print!(
                "{}",
                crate::config::cli_modify_config(
                    args.key.as_deref(),
                    args.value.as_deref(),
                    args.show
                )?
            );
            Ok(())
        }
        other => Err(Error::value(format!(
            "Invalid command {other}. use \"brainglobe -h for more info.\""
        ))),
    }
}
