//! 词法：heredoc、管道与命令段、词与重定向、引号和变量展开。

/// 拆出 heredoc 正文；返回去掉正文的命令和各段正文。
pub(super) fn heredocs(command: &str) -> (String, Vec<String>) {
    let mut head = Vec::new();
    let mut bodies = Vec::new();
    let mut lines = command.lines();
    while let Some(line) = lines.next() {
        head.push(line);
        for (tag, strip_tabs) in heredoc_tags(line) {
            let mut body = Vec::new();
            for line in lines.by_ref() {
                if (if strip_tabs { line.trim_start_matches('\t') } else { line }).trim_end() == tag {
                    break;
                }
                body.push(line);
            }
            bodies.push(body.join("\n"));
        }
    }
    (head.join("\n"), bodies)
}

/// 引号外的 `<<TAG`、`<<-TAG`、`<<'TAG'`；`<<<` 是 here-string。
fn heredoc_tags(line: &str) -> Vec<(String, bool)> {
    let bytes = line.as_bytes();
    let mut tags = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\'' | b'"' | b'`' => i = skip_quoted(bytes, i),
            b'\\' => i += 2,
            b'<' if line[i..].starts_with("<<<") => i += 3,
            b'<' if line[i..].starts_with("<<") => {
                let rest = &line[i + 2..];
                let strip_tabs = rest.starts_with('-');
                let rest = rest.trim_start_matches('-').trim_start().trim_start_matches(['\'', '"']);
                let tag: String = rest.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
                if !tag.is_empty() {
                    tags.push((tag, strip_tabs));
                }
                i += 2;
            }
            _ => i += 1,
        }
    }
    tags
}

/// 按 `&&`、`||`、`;`、`&`、换行拆成管道，再按 `|` 拆段；引号、`$(…)` 和反引号内部不拆，注释丢弃。
/// 第二项表示管道之间出现过 `&&` 或 `||`。
pub(super) fn pipelines(text: &str) -> (Vec<Vec<&str>>, bool) {
    let bytes = text.as_bytes();
    let mut pipelines = Vec::new();
    let mut segments = Vec::new();
    let mut start = 0;
    let mut i = 0;
    let mut depth = 0usize;
    let mut conditional = false;
    while i < bytes.len() {
        let c = bytes[i];
        match c {
            b'\'' | b'"' | b'`' => i = skip_quoted(bytes, i),
            b'\\' => i += 2,
            b'$' if bytes.get(i + 1) == Some(&b'(') => {
                depth += 1;
                i += 2;
            }
            b')' if depth > 0 => {
                depth -= 1;
                i += 1;
            }
            b'#' if depth == 0 && (i == 0 || bytes[i - 1].is_ascii_whitespace()) => {
                push_segment(&mut segments, &text[start..i]);
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
                start = i;
            }
            b'|' | b'&' | b';' | b'\n' if depth == 0 => {
                let pair = bytes.get(i + 1) == Some(&c) && c != b';' && c != b'\n';
                conditional |= pair;
                let pipe_stderr = c == b'|' && bytes.get(i + 1) == Some(&b'&');
                let redirect = c == b'&' && (bytes.get(i + 1) == Some(&b'>') || (i > 0 && bytes[i - 1] == b'>'));
                if redirect {
                    i += 1;
                    continue;
                }
                push_segment(&mut segments, &text[start..i]);
                if (c != b'|' || pair) && !segments.is_empty() {
                    pipelines.push(std::mem::take(&mut segments));
                }
                i += if pair || pipe_stderr { 2 } else { 1 };
                start = i;
            }
            _ => i += 1,
        }
    }
    push_segment(&mut segments, &text[start.min(text.len())..]);
    if !segments.is_empty() {
        pipelines.push(segments);
    }
    (pipelines, conditional)
}

/// 去掉子 shell 和代码块的括号：开头的 `(`、单独的 `{`，以及配不上对的结尾 `)`、单独的 `}`。
fn push_segment<'a>(segments: &mut Vec<&'a str>, text: &'a str) {
    let mut segment = text.trim();
    segment = segment.strip_prefix('(').unwrap_or(segment).trim_start();
    if let Some(rest) = segment.strip_prefix('{').filter(|rest| rest.is_empty() || rest.starts_with(char::is_whitespace)) {
        segment = rest.trim_start();
    }
    while segment.ends_with(')') && paren_depth(segment) < 0 {
        segment = segment[..segment.len() - 1].trim_end();
    }
    if !segment.is_empty() && segment != "}" {
        segments.push(segment);
    }
}

fn paren_depth(text: &str) -> i32 {
    let bytes = text.as_bytes();
    let (mut depth, mut i) = (0, 0);
    while i < bytes.len() {
        match bytes[i] {
            b'\'' | b'"' | b'`' => i = skip_quoted(bytes, i),
            b'\\' => i += 2,
            c => {
                depth += match c { b'(' => 1, b')' => -1, _ => 0 };
                i += 1;
            }
        }
    }
    depth
}

fn skip_quoted(bytes: &[u8], open: usize) -> usize {
    let quote = bytes[open];
    let mut i = open + 1;
    while i < bytes.len() && bytes[i] != quote {
        i += if quote == b'"' && bytes[i] == b'\\' { 2 } else { 1 };
    }
    i + 1
}

pub(super) enum Token<'a> {
    Word(&'a str),
    Redirect { op: &'a str, target: Option<&'a str> },
}

/// 一段命令的词与重定向，词保留原始引号。
pub(super) fn tokens(text: &str) -> Vec<Token<'_>> {
    let bytes = text.as_bytes();
    let mut tokens = Vec::new();
    let mut i = 0;
    let word_end = |mut i: usize| {
        let mut depth = 0usize;
        while i < bytes.len() {
            match bytes[i] {
                b'\'' | b'"' | b'`' => i = skip_quoted(bytes, i),
                b'\\' => i += 2,
                b'$' if bytes.get(i + 1) == Some(&b'(') => {
                    depth += 1;
                    i += 2;
                }
                b')' if depth > 0 => {
                    depth -= 1;
                    i += 1;
                }
                c if depth == 0 && (c.is_ascii_whitespace() || c == b'<' || c == b'>') => break,
                _ => i += 1,
            }
        }
        i.min(bytes.len())
    };
    while i < bytes.len() {
        if bytes[i].is_ascii_whitespace() {
            i += 1;
            continue;
        }
        let digit = bytes[i].is_ascii_digit() && matches!(bytes.get(i + 1), Some(b'<' | b'>'));
        let amp = bytes[i] == b'&' && bytes.get(i + 1) == Some(&b'>');
        if digit || amp || bytes[i] == b'<' || bytes[i] == b'>' {
            let start = i;
            i += if digit || amp { 1 } else { 0 };
            while i < bytes.len() && matches!(bytes[i], b'<' | b'>') {
                i += 1;
            }
            if i < bytes.len() && bytes[i] == b'-' && text[start..i].ends_with("<<") {
                i += 1;
            }
            if i < bytes.len() && bytes[i] == b'&' {
                i += 1;
                while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'-') {
                    i += 1;
                }
                tokens.push(Token::Redirect { op: &text[start..i], target: None });
                continue;
            }
            let op = &text[start..i];
            while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            let end = word_end(i);
            tokens.push(Token::Redirect { op, target: (end > i).then(|| &text[i..end]) });
            i = end;
            continue;
        }
        let end = word_end(i).max(i + 1);
        tokens.push(Token::Word(&text[i..end]));
        i = end;
    }
    tokens
}

pub(super) fn words(text: &str, vars: &[(String, String)]) -> Vec<String> {
    tokens(text).into_iter().filter_map(|token| match token {
        Token::Word(word) => Some(unquote(word, vars)),
        Token::Redirect { .. } => None,
    }).collect()
}

/// 去掉引号和转义，并展开前面赋值过的 `$NAME`、`${NAME}`（单引号内不展开）；其余变量和 `$(…)` 保持原样。
pub(super) fn unquote(word: &str, vars: &[(String, String)]) -> String {
    let mut out = String::with_capacity(word.len());
    let mut chars = word.char_indices().peekable();
    let mut quote = None;
    while let Some((i, c)) = chars.next() {
        match (quote, c) {
            (None, '\'' | '"') => quote = Some(c),
            (Some(open), c) if c == open => quote = None,
            (Some('\''), c) => out.push(c),
            (Some(_), '\\') if matches!(chars.peek(), Some((_, '"' | '\\' | '$' | '`'))) => out.extend(chars.next().map(|(_, c)| c)),
            (None, '\\') => out.extend(chars.next().map(|(_, c)| c)),
            (_, '$') => match variable(&word[i + 1..], vars) {
                Some((value, len)) => {
                    out.push_str(value);
                    chars.nth(len - 1);
                }
                None => out.push('$'),
            },
            (_, c) => out.push(c),
        }
    }
    out
}

/// 命中赋值的变量返回取值和变量名占用的字节数（变量名只含 ASCII）。
fn variable<'a>(rest: &str, vars: &'a [(String, String)]) -> Option<(&'a str, usize)> {
    let (name, len) = match rest.strip_prefix('{') {
        Some(inner) => {
            let name = &inner[..inner.find('}')?];
            (name, name.len() + 2)
        }
        None => {
            let len = rest.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).unwrap_or(rest.len());
            (&rest[..len], len)
        }
    };
    let (_, value) = vars.iter().rev().find(|(var, _)| !name.is_empty() && var == name)?;
    Some((value, len))
}
