//! 技能：Claude Code 的 Skill、OpenCode 的 skill 工具，以及读取 `skills/<名称>/SKILL.md` 的调用。
use super::{block_text, Op};
use serde_json::Value;

/// 已加载的技能正文：去掉 `<skill_content>` 标签、技能目录说明和 frontmatter。
pub(crate) fn loaded(name: &str, args: Option<&str>, text: &str) -> Op {
    let text = text.trim_start();
    let text = text.strip_prefix("<skill_content").and_then(|rest| rest.split_once('>')).map_or(text, |(_, rest)| rest);
    let text = text.trim().trim_end_matches("</skill_content>").trim_start();
    let text = text.strip_prefix("# Skill: ").and_then(|rest| rest.split_once('\n')).map_or(text, |(_, rest)| rest).trim_start();
    let (path, text) = match text.strip_prefix("Base directory for this skill: ").and_then(|rest| rest.split_once('\n')) {
        Some((path, rest)) => (Some(path.trim().to_owned()), rest.trim_start()),
        None => (None, text),
    };
    let (_, body) = frontmatter(text);
    Op::Skill {
        name: name.into(),
        args: args.map(str::trim).filter(|args| !args.is_empty()).map(Into::into),
        path,
        body: Some(body.into()),
    }
}

/// 读取 SKILL.md 的结果：名称取 frontmatter 的 `name`，没有时要求路径形如 `skills/<名称>/SKILL.md`。
fn read(path: &str, text: &str) -> Option<Op> {
    let dir = if path == "SKILL.md" { "." } else { path.strip_suffix("/SKILL.md")? };
    let (name, body) = frontmatter(text);
    let name = name.or_else(|| (dir.rsplit('/').nth(1) == Some("skills")).then(|| dir.rsplit('/').next()).flatten())?;
    Some(Op::Skill { name: name.into(), args: None, path: Some(dir.into()), body: Some(body.into()) })
}

pub(super) fn promote(ops: &mut [Op], result: &Value) {
    if result["is_error"] == true { return; }
    if let Some(text) = result["output"].as_str().or_else(|| block_text(result)) {
        promote_text(ops, text);
    }
}

/// 从第一行起读取 SKILL.md 的单次调用改为技能卡片。
pub(crate) fn promote_text(ops: &mut [Op], text: &str) {
    let path = match &*ops {
        [Op::Read { path, start, tail: None, .. }] if start.is_none_or(|line| line == 1) => path.as_str(),
        [Op::Shell(shell)] => match shell.single_read() {
            Some(path) => path,
            None => return,
        },
        _ => return,
    };
    if let Some(skill) = read(path, text) {
        ops[0] = skill;
    }
}

/// 提取 frontmatter 里的 `name` 和其后的正文。
fn frontmatter(text: &str) -> (Option<&str>, &str) {
    let Some((head, body)) = text.strip_prefix("---\n").and_then(|rest| rest.split_once("\n---\n")) else {
        return (None, text);
    };
    let name = head.lines().find_map(|line| line.strip_prefix("name:")).map(|value| value.trim().trim_matches(['\'', '"']))
        .filter(|name| !name.is_empty() && !name.starts_with(['|', '>']));
    (name, body.trim_start_matches('\n'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn skill_bodies_drop_wrappers_and_frontmatter() {
        let op = loaded("session-reader", Some("abc"), "Base directory for this skill: /u/.claude/skills/session-reader\n\n# Session Reader\nRun it.");
        assert_eq!(op, Op::Skill { name: "session-reader".into(), args: Some("abc".into()), path: Some("/u/.claude/skills/session-reader".into()), body: Some("# Session Reader\nRun it.".into()) });

        let mut ops = vec![Op::read("/u/.agents/skills/ego/SKILL.md")];
        let result = json!({"content":"---\nname: ego\ndescription: >-\n  Drive the\n  browser.\n---\n\n# Ego\n"});
        promote(&mut ops, &result);
        assert_eq!(ops, [Op::Skill { name: "ego".into(), args: None, path: Some("/u/.agents/skills/ego".into()), body: Some("# Ego\n".into()) }]);
        assert!(json!(ops.pop().unwrap().summary()).get("body").is_none());

        let mut partial = vec![Op::read_range("/u/.agents/skills/ego/SKILL.md", 24, 2, Some(40))];
        promote(&mut partial, &json!({"content":"Two lines from the middle."}));
        assert!(matches!(partial.as_slice(), [Op::Read { start: Some(24), .. }]));
        let mut shell = [super::super::shell("sed -n '1,25p' SKILL.md", None, None, None)];
        promote(&mut shell, &result);
        assert!(matches!(&shell[0], Op::Skill { name, .. } if name == "ego"));
    }
}
