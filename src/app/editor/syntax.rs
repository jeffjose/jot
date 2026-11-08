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

/// A kind's color in a theme's palette, from its sixteen colors: what a
/// shell's highlighter (fish, zsh's) uses, so it reads as one.
pub fn color(kind: Kind, t: &crate::theme::Theme) -> u32 {
    let a = &t.ansi;
    let faint = crate::canvas::mix(t.bg, t.fg, if t.dark { 120 } else { 140 });
    match kind {
        Kind::Command => a[4],
        Kind::Assign => a[5],
        Kind::Flag => a[6],
        Kind::Arg => t.fg,
        Kind::Path => crate::canvas::mix(t.fg, a[6], 90),
        Kind::Str => a[2],
        Kind::Var => a[3],
        Kind::Op | Kind::Redirect => a[5],
        Kind::Comment => faint,
        Kind::Escape => a[1],
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
