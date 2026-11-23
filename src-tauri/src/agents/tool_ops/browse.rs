//! 搜索、查找、列目录的统计；搜索命中按文件分组，卡片直接展示。
use super::{Hit, Hits, Op};

/// `output` 是这一步单独的输出；多步命令的输出拆不开，不做统计。
pub(crate) fn search(pattern: &str, paths: Vec<String>, files_only: bool, output: Option<&str>) -> Op {
    let in_file = !paths.is_empty() && paths.iter().all(|path| is_file(path));
    let single = (in_file && paths.len() == 1).then(|| paths[0].as_str());
    let hits = output.and_then(|output| if files_only { file_list(output) } else { matches(output, single) });
    let (matches, files) = match &hits {
        Some(hits) => (
            (!files_only).then(|| hits.iter().flat_map(|file| &file.lines).filter(|hit| !hit.context).count()),
            Some(hits.len()),
        ),
        None => (None, None),
    };
    Op::Search { pattern: pattern.into(), paths, in_file, matches, files, hits }
}

pub(crate) fn find(pattern: Option<&str>, paths: Vec<String>, output: Option<&str>) -> Op {
    Op::Find { pattern: pattern.map(Into::into), paths, count: output.map(|output| entries(output).count()) }
}

pub(crate) fn list(paths: Vec<String>, output: Option<&str>) -> Op {
    let count = output.map(|output| entries(output)
        .filter(|line| !line.starts_with("total ") && !line.ends_with(':') && !line.ends_with(" .") && !line.ends_with(" ..") && *line != "." && *line != "..")
        .count());
    Op::List { paths, count }
}

/// 末段带扩展名的视为文件；目录、通配符和花括号展开都按目录范围处理。
fn is_file(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    !path.ends_with('/') && !name.contains(['*', '?', '{']) && name.rfind('.').is_some_and(|dot| dot > 0)
}

/// 输出的非空行，去掉 `Found 3 files`、`No files found` 这类摘要。
fn entries(output: &str) -> impl Iterator<Item = &str> {
    output.lines().map(str::trim_end).filter(|line| {
        let summary = (line.starts_with("No ") && line.ends_with(" found"))
            || line.strip_prefix("Found ").and_then(|rest| rest.split(' ').next()).is_some_and(|n| n.parse::<u64>().is_ok());
        !line.trim().is_empty() && !summary
    })
}

fn file_list(output: &str) -> Option<Vec<Hits>> {
    Some(entries(output).map(|path| Hits { path: path.into(), lines: Vec::new() }).collect())
}

/// 识别 `路径:行号:内容`、`路径-行号-内容`（上下文）、`路径:内容`、单文件的 `行号:内容`，
/// 以及 OpenCode 按文件分组的 `  Line 行号: 内容`；有任何一行认不出就放弃分组。
fn matches(output: &str, single: Option<&str>) -> Option<Vec<Hits>> {
    let mut files: Vec<Hits> = Vec::new();
    let lines: Vec<_> = entries(output).filter(|line| *line != "--").collect();
    if lines.iter().any(|line| line.starts_with(' ') && line.trim_start().starts_with("Line ")) {
        let mut path = None;
        for line in lines {
            match line.trim_start().strip_prefix("Line ").and_then(|rest| rest.split_once(": ")) {
                Some((number, text)) if line.starts_with(' ') => push(&mut files, path?, Hit { line: number.parse().ok(), text: text.into(), context: false }),
                _ => path = Some(line.strip_suffix(':')?),
            }
        }
        return Some(files);
    }
    if let Some(path) = single {
        let all_numbered = lines.iter().all(|line| numbered(line).is_some());
        for line in lines {
            match all_numbered.then(|| numbered(line)).flatten() {
                Some((number, context, text)) => push(&mut files, path, Hit { line: Some(number), text: text.into(), context }),
                None => push(&mut files, path, Hit { line: None, text: line.into(), context: false }),
            }
        }
        return Some(files);
    }
    for line in lines {
        let (path, line, context, text) = located(line, files.last().map(|file| file.path.as_str()))?;
        push(&mut files, path, Hit { line, text: text.into(), context });
    }
    Some(files)
}

/// 多文件输出的一行，依次尝试：上一文件的命中或上下文、`路径:行号:内容`、
/// 尚未出现命中的文件的上下文 `路径-行号-内容`、不带行号的 `路径:内容`。
fn located<'a>(line: &'a str, last: Option<&str>) -> Option<(&'a str, Option<u64>, bool, &'a str)> {
    let path_like = |path: &str| path.contains(['/', '.']) && !path.contains(char::is_whitespace);
    if let Some(last) = last.filter(|last| line.starts_with(*last) && matches!(line.as_bytes().get(last.len()), Some(b':' | b'-'))) {
        if let Some((number, context, text)) = numbered(&line[last.len() + 1..]) {
            return Some((&line[..last.len()], Some(number), context, text));
        }
    }
    let split = line.split_once(':').filter(|(path, _)| path_like(path));
    if let Some((path, (number, _, text))) = split.and_then(|(path, rest)| Some((path, numbered(rest).filter(|(_, context, _)| !context)?))) {
        return Some((path, Some(number), false, text));
    }
    let context = line.match_indices('-').find_map(|(dash, _)| {
        let path = &line[..dash];
        let (number, context, text) = numbered(&line[dash + 1..]).filter(|_| is_file(path) && !path.contains([':', ' ']))?;
        context.then_some((path, Some(number), true, text))
    });
    context.or_else(|| split.map(|(path, rest)| (path, None, false, rest)))
}

fn push(files: &mut Vec<Hits>, path: &str, hit: Hit) {
    match files.last_mut() {
        Some(file) if file.path == path => file.lines.push(hit),
        _ => files.push(Hits { path: path.into(), lines: vec![hit] }),
    }
}

/// `12:内容` 是命中，`12-内容` 是上下文。
fn numbered(line: &str) -> Option<(u64, bool, &str)> {
    let digits = line.len() - line.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    let separator = line[digits..].chars().next().filter(|&c| digits > 0 && (c == ':' || c == '-'))?;
    Some((line[..digits].parse().ok()?, separator == '-', &line[digits + 1..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_output_groups_hits_by_file() {
        let output = "src/a.rs:3:let x = 1;\nsrc/a.rs-4-let y = 2;\n--\nsrc/a.rs:9:x\nsrc/b-1-c.rs-7-before: x\nsrc/b-1-c.rs:8:x\n";
        let Op::Search { matches, files, hits: Some(hits), in_file, .. } = search("x", vec!["src".into()], false, Some(output)) else { panic!() };
        assert_eq!((matches, files, in_file), (Some(3), Some(2), false));
        assert_eq!(hits[0].lines[1], Hit { line: Some(4), text: "let y = 2;".into(), context: true });
        assert_eq!((hits[1].path.as_str(), &hits[1].lines[0]), ("src/b-1-c.rs", &Hit { line: Some(7), text: "before: x".into(), context: true }));

        let Op::Search { matches, hits: Some(hits), in_file, .. } = search("fn", vec!["src/a.ts".into()], false, Some("64:fn a() {\n65-  b\n")) else { panic!() };
        assert_eq!((matches, in_file, hits[0].path.as_str()), (Some(1), true, "src/a.ts"));
        let Op::Search { matches, hits: Some(hits), .. } = search("x", Vec::new(), false, Some("Found 2 matches\n/a.ts:\n  Line 3: x\n\n/b.ts:\n  Line 9: y")) else { panic!() };
        assert_eq!((matches, hits[1].path.as_str(), hits[1].lines[0].line), (Some(2), "/b.ts", Some(9)));
        assert!(matches!(search("x", Vec::new(), false, Some("Error: bad pattern")), Op::Search { hits: None, .. }));
    }
}
