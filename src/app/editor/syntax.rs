//! What each part of a command line is, for the long-command editor to
//! color it: the command, its flags and arguments, strings, variables,
//! pipes and the like. A shell's words as a person reads them, not a
//! parser's tree: it never fails, on half-typed lines least of all.
//!
//! Each token keeps its kind and where it is (row and columns), so the
//! editor can do more with them than color them later: complete a flag,
//! say what a command is, lead from a path to its file.

/// What a stretch of a command line is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// The program run: the first word, and the first after a pipe, `&&`,
    /// `||`, `;`, `&`, or `(`.
    Command,
    /// `NAME=value` before a command: set for it.
    Assign,
    /// `-x`, `--long`, `--long=value`.
    Flag,
    /// Any other word.
    Arg,
    /// A word that names a file or folder: has a `/`, or starts with `~`
    /// or `.`.
    Path,
    /// '...' or "...", the quotes with it.
    Str,
    /// `$NAME`, `${...}`, `$(...)`, `` `...` ``, `$?` and the like.
    Var,
    /// `|`, `||`, `&&`, `;`, `&`, `(`, `)`.
    Op,
    /// `>`, `>>`, `<`, `2>`, `&>`, `2>&1`, `<<`.
    Redirect,
    /// `# ...` to the end of the line.
    Comment,
    /// A backslash and what it escapes.
    Escape,
}

/// A stretch of one line of the editor: `kind` from column `start` up to
/// (not with) `end`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Token {
    pub row: usize,
    pub start: usize,
    pub end: usize,
    pub kind: Kind,
}

/// The tokens of the editor's lines, line by line. The lines are one
/// command (the editor joins them with spaces), so a string or a command
/// carries on from one line to the next.
pub fn tokens(lines: &[Vec<char>]) -> Vec<Vec<Token>> {
    // The text as one run of characters, each with its place; a line's
    // end is a space between words.
    let mut chars: Vec<(char, usize, usize)> = Vec::new();
    for (r, line) in lines.iter().enumerate() {
        for (c, ch) in line.iter().enumerate() {
            chars.push((*ch, r, c));
        }
        chars.push(('\n', r, line.len()));
    }
    let mut out: Vec<Vec<Token>> = vec![Vec::new(); lines.len()];
    // A run of characters [a, b) of one kind, split at line ends.
    let push = |a: usize, b: usize, kind: Kind, out: &mut Vec<Vec<Token>>| {
        let mut i = a;
        while i < b {
            let row = chars[i].1;
            let start = chars[i].2;
            let mut j = i;
            while j < b && chars[j].1 == row && chars[j].0 != '\n' {
                j += 1;
            }
            if j > i {
                out[row].push(Token { row, start, end: chars[j - 1].2 + 1, kind });
            }
            // Past the line's end.
            i = j.max(i + 1);
        }
    };
    let n = chars.len();
    let at = |i: usize| chars.get(i).map(|c| c.0);
    let mut i = 0;
    // The next word is a command: at the start, and after an operator.
    let mut command = true;
    while i < n {
        let c = chars[i].0;
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        // A comment: only where a word starts.
        if c == '#' {
            let mut j = i;
            while j < n && chars[j].0 != '\n' {
                j += 1;
            }
            push(i, j, Kind::Comment, &mut out);
            i = j;
            continue;
        }
        // Operators: the longest first.
        let two: String = [Some(c), at(i + 1)].iter().flatten().collect();
        if ["&&", "||"].contains(&two.as_str()) {
            push(i, i + 2, Kind::Op, &mut out);
            i += 2;
            command = true;
            continue;
        }
        // Redirections: a digit or & before > or <, then >, >>, <, <<,
        // >&N.
        let redirect_len = redirect(&chars[i..].iter().map(|c| c.0).take(5).collect::<Vec<_>>());
        if redirect_len > 0 {
            push(i, i + redirect_len, Kind::Redirect, &mut out);
            i += redirect_len;
            continue;
        }
        if matches!(c, '|' | ';' | '&' | '(' | ')') {
            push(i, i + 1, Kind::Op, &mut out);
            i += 1;
            command = c != ')';
            continue;
        }
        // A word: up to whitespace or an operator not in quotes, its
        // strings, variables and escapes colored apart within it.
        let start = i;
        let mut parts: Vec<(usize, usize, Kind)> = Vec::new();
        let mut plain_from = i;
        while i < n {
            let c = chars[i].0;
            if c.is_whitespace() || matches!(c, '|' | ';' | '&' | '(' | ')' | '<' | '>') {
                break;
            }
            let inner = match c {
                '\'' => {
                    let mut j = i + 1;
                    while j < n && chars[j].0 != '\'' {
                        j += 1;
                    }
                    Some((j.min(n - 1) + 1, Kind::Str))
                }
                '"' => {
                    let mut j = i + 1;
                    while j < n && chars[j].0 != '"' {
                        j += if chars[j].0 == '\\' { 2 } else { 1 };
                    }
                    Some((j.min(n - 1) + 1, Kind::Str))
                }
                '`' => {
                    let mut j = i + 1;
                    while j < n && chars[j].0 != '`' {
                        j += 1;
                    }
                    Some((j.min(n - 1) + 1, Kind::Var))
                }
                '\\' => Some(((i + 2).min(n), Kind::Escape)),
                '$' => Some((variable_end(&chars, i), Kind::Var)),
                _ => None,
            };
            match inner {
                Some((end, kind)) => {
                    if plain_from < i {
                        parts.push((plain_from, i, Kind::Arg));
                    }
                    parts.push((i, end, kind));
                    i = end;
                    plain_from = end;
                }
                None => i += 1,
            }
        }
        if plain_from < i {
            parts.push((plain_from, i, Kind::Arg));
        }
        let word: String = chars[start..i].iter().map(|c| c.0).collect();
        // What the word is, for its plain parts.
        let kind = if command && is_assignment(&word) {
            Kind::Assign
        } else if command {
            command = false;
            Kind::Command
        } else if word.starts_with('-') && word.len() > 1 {
            Kind::Flag
        } else if word.contains('/') || word.starts_with('~') || word.starts_with("./") || word == "." || word == ".." {
            Kind::Path
        } else {
            Kind::Arg
        };
        for (a, b, k) in parts {
            push(a, b, if k == Kind::Arg { kind } else { k }, &mut out);
        }
    }
    out
}

/// How long a redirection starting here is: 0 if none.
fn redirect(s: &[char]) -> usize {
    let mut i = 0;
    // 2>, &>
    if s.first().is_some_and(|c| c.is_ascii_digit() || *c == '&') && s.get(1).is_some_and(|c| *c == '>' || *c == '<') {
        i = 1;
    }
    match s.get(i) {
        Some('>') | Some('<') => {}
        _ => return 0,
    }
    let op = s[i];
    i += 1;
    // >>, <<
    if s.get(i) == Some(&op) {
        i += 1;
    }
    // >&1, >&-
    if s.get(i) == Some(&'&') && s.get(i + 1).is_some_and(|c| c.is_ascii_digit() || *c == '-') {
        i += 2;
    }
    i
}

/// Where a `$` variable starting at `i` ends.
fn variable_end(chars: &[(char, usize, usize)], i: usize) -> usize {
    let n = chars.len();
    let close = |open: char, close: char| {
        let mut depth = 0;
        let mut j = i + 1;
        while j < n {
            let c = chars[j].0;
            if c == open {
                depth += 1;
            } else if c == close {
                depth -= 1;
                if depth == 0 {
                    return j + 1;
                }
            }
            j += 1;
        }
        n
    };
    match chars.get(i + 1).map(|c| c.0) {
        Some('{') => close('{', '}'),
        Some('(') => close('(', ')'),
        Some(c) if "?$!#*@-0123456789".contains(c) => i + 2,
        Some(c) if c.is_alphanumeric() || c == '_' => {
            let mut j = i + 1;
            while j < n && (chars[j].0.is_alphanumeric() || chars[j].0 == '_') {
                j += 1;
            }
            j
        }
        _ => i + 1,
    }
}

/// NAME=value, NAME made of letters, digits and _, not starting with a
/// digit.
fn is_assignment(word: &str) -> bool {
    let Some((name, _)) = word.split_once('=') else { return false };
    !name.is_empty() && !name.starts_with(|c: char| c.is_ascii_digit()) && name.chars().all(|c| c.is_alphanumeric() || c == '_')
}

/// The editor's colors: its ground and plain text, and a color for each
/// kind. The first, "Terminal", is the terminal theme's own; the rest
/// are editors' well-known schemes, the same whatever the terminal's.
pub struct Scheme {
    pub name: &'static str,
    pub bg: u32,
    pub fg: u32,
    pub command: u32,
    pub assign: u32,
    pub flag: u32,
    pub path: u32,
    pub string: u32,
    pub var: u32,
    pub op: u32,
    pub comment: u32,
    pub escape: u32,
}

/// The schemes Settings offers, by name, "Terminal" first.
pub const SCHEMES: &[&str] = &["Terminal", "One Dark", "Dracula", "Tokyo Night", "Nord", "Gruvbox Dark", "Monokai", "GitHub Light", "Solarized Light"];

/// The scheme named, in terminal theme `t` (which "Terminal", and any name
/// not known, takes its colors from).
pub fn scheme(name: &str, t: &crate::theme::Theme) -> Scheme {
    let s = |name, bg, fg, command, assign, flag, path, string, var, op, comment, escape| Scheme { name, bg, fg, command, assign, flag, path, string, var, op, comment, escape };
    match SCHEMES.iter().find(|n| n.eq_ignore_ascii_case(name.trim())).copied().unwrap_or("Terminal") {
        "One Dark" => s("One Dark", 0x282C34, 0xABB2BF, 0x61AFEF, 0xC678DD, 0x56B6C2, 0xD7DAE0, 0x98C379, 0xE5C07B, 0xC678DD, 0x7F848E, 0xE06C75),
        "Dracula" => s("Dracula", 0x282A36, 0xF8F8F2, 0x50FA7B, 0xFF79C6, 0x8BE9FD, 0xF8F8F2, 0xF1FA8C, 0xBD93F9, 0xFF79C6, 0x6272A4, 0xFFB86C),
        "Tokyo Night" => s("Tokyo Night", 0x1A1B26, 0xC0CAF5, 0x7AA2F7, 0xBB9AF7, 0x7DCFFF, 0xA9B1D6, 0x9ECE6A, 0xE0AF68, 0x89DDFF, 0x565F89, 0xF7768E),
        "Nord" => s("Nord", 0x2E3440, 0xD8DEE9, 0x88C0D0, 0xB48EAD, 0x8FBCBB, 0xE5E9F0, 0xA3BE8C, 0xEBCB8B, 0x81A1C1, 0x616E88, 0xBF616A),
        "Gruvbox Dark" => s("Gruvbox Dark", 0x282828, 0xEBDBB2, 0xB8BB26, 0xD3869B, 0x83A598, 0xEBDBB2, 0xFABD2F, 0x8EC07C, 0xFE8019, 0x928374, 0xFB4934),
        "Monokai" => s("Monokai", 0x272822, 0xF8F8F2, 0xA6E22E, 0xAE81FF, 0x66D9EF, 0xF8F8F2, 0xE6DB74, 0xFD971F, 0xF92672, 0x75715E, 0xAE81FF),
        "GitHub Light" => s("GitHub Light", 0xFFFFFF, 0x24292F, 0x8250DF, 0xCF222E, 0x0550AE, 0x24292F, 0x0A3069, 0x953800, 0xCF222E, 0x6E7781, 0x116329),
        "Solarized Light" => s("Solarized Light", 0xFDF6E3, 0x586E75, 0x268BD2, 0x6C71C4, 0x2AA198, 0x657B83, 0x859900, 0xB58900, 0xD33682, 0x93A1A1, 0xCB4B16),
        // The terminal theme's sixteen colors: what a shell's highlighter
        // (fish, zsh's) uses, so it reads as the terminal does.
        _ => {
            let a = &t.ansi;
            let faint = crate::canvas::mix(t.bg, t.fg, if t.dark { 120 } else { 140 });
            s("Terminal", t.bg, t.fg, a[4], a[5], a[6], crate::canvas::mix(t.fg, a[6], 90), a[2], a[3], a[5], faint, a[1])
        }
    }
}

impl Scheme {
    /// A kind's color.
    pub fn color(&self, kind: Kind) -> u32 {
        match kind {
            Kind::Command => self.command,
            Kind::Assign => self.assign,
            Kind::Flag => self.flag,
            Kind::Arg => self.fg,
            Kind::Path => self.path,
            Kind::Str => self.string,
            Kind::Var => self.var,
            Kind::Op | Kind::Redirect => self.op,
            Kind::Comment => self.comment,
            Kind::Escape => self.escape,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(s: &str) -> Vec<(String, Kind)> {
        let lines: Vec<Vec<char>> = s.split('\n').map(|l| l.chars().collect()).collect();
        tokens(&lines).into_iter().flatten().map(|t| (lines[t.row][t.start..t.end].iter().collect(), t.kind)).collect()
    }

    #[test]
    fn a_pipeline() {
        assert_eq!(
            kinds("ls -la ~/src | grep \"foo bar\" && echo $HOME"),
            vec![
                ("ls".into(), Kind::Command),
                ("-la".into(), Kind::Flag),
                ("~/src".into(), Kind::Path),
                ("|".into(), Kind::Op),
                ("grep".into(), Kind::Command),
                ("\"foo bar\"".into(), Kind::Str),
                ("&&".into(), Kind::Op),
                ("echo".into(), Kind::Command),
                ("$HOME".into(), Kind::Var),
            ]
        );
    }

    #[test]
    fn assignments_redirections_comments() {
        assert_eq!(
            kinds("FOO=1 make 2>&1 >> log # done"),
            vec![
                ("FOO=1".into(), Kind::Assign),
                ("make".into(), Kind::Command),
                ("2>&1".into(), Kind::Redirect),
                (">>".into(), Kind::Redirect),
                ("log".into(), Kind::Arg),
                ("# done".into(), Kind::Comment),
            ]
        );
    }

    #[test]
    fn across_lines_and_unfinished() {
        // A string open at a line's end goes on into the next.
        assert_eq!(kinds("echo 'a\nb' c"), vec![("echo".into(), Kind::Command), ("'a".into(), Kind::Str), ("b'".into(), Kind::Str), ("c".into(), Kind::Arg)]);
        // Half typed: never a panic, the open string to the end.
        assert_eq!(kinds("git commit -m \"wip"), vec![("git".into(), Kind::Command), ("commit".into(), Kind::Arg), ("-m".into(), Kind::Flag), ("\"wip".into(), Kind::Str)]);
        assert_eq!(kinds("x=$(date"), vec![("x=".into(), Kind::Assign), ("$(date".into(), Kind::Var)]);
        assert!(kinds("").is_empty());
    }

    #[test]
    fn flags_with_values() {
        assert_eq!(kinds("cargo build --release --target=x86"), vec![("cargo".into(), Kind::Command), ("build".into(), Kind::Arg), ("--release".into(), Kind::Flag), ("--target=x86".into(), Kind::Flag)]);
    }
}
