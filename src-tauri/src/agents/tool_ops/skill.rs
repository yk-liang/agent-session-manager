//! 技能：Claude Code 的 Skill、OpenCode 的 skill 工具，以及读取 `skills/<名称>/SKILL.md` 的调用。
use super::Op;
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
    let (description, body) = frontmatter(text);
    Op::Skill {
        name: name.into(),
        args: args.map(str::trim).filter(|args| !args.is_empty()).map(Into::into),
        path,
        description,
        body: Some(body.into()),
    }
}

/// 读取 `skills/<名称>/SKILL.md` 的结果视为加载技能；返回技能描述和去掉 frontmatter 的正文。
pub(crate) fn read(path: &str, text: &str) -> Option<Op> {
    let dir = path.strip_suffix("/SKILL.md").filter(|dir| dir.rsplit('/').nth(1) == Some("skills"))?;
    let (description, body) = frontmatter(text);
    let name = dir.rsplit('/').next().unwrap_or(dir);
    Some(Op::Skill { name: name.into(), args: None, path: Some(dir.into()), description, body: Some(body.into()) })
}

/// 单次读取 SKILL.md 的调用改为技能卡片。
pub(crate) fn promote(ops: &mut [Op], result: &Value) {
    let path = match &*ops {
        [Op::Read { path, .. }] => path.as_str(),
        [Op::Shell(shell)] => match shell.single_read() {
            Some(path) => path,
            None => return,
        },
        _ => return,
    };
    let content = &result["content"];
    let text = content.as_str().or_else(|| content.as_array()
        .filter(|parts| parts.len() == 1)
        .and_then(|parts| parts[0]["text"].as_str()));
    if let Some(skill) = text.filter(|_| result["is_error"] != true).and_then(|text| read(path, text)) {
        ops[0] = skill;
    }
}

/// frontmatter 里的 `description`，支持单行和 `>-`、`|` 折叠写法。
fn frontmatter(text: &str) -> (Option<String>, &str) {
    let Some((head, body)) = text.strip_prefix("---\n").and_then(|rest| rest.split_once("\n---\n")) else {
        return (None, text);
    };
    let mut lines = head.lines();
    let description = lines.by_ref().find_map(|line| line.strip_prefix("description:")).map(|value| {
        let value = value.trim();
        if value.is_empty() || value.starts_with(['>', '|']) {
            lines.take_while(|line| line.starts_with(char::is_whitespace)).map(str::trim).collect::<Vec<_>>().join(" ")
        } else {
            value.trim_matches(['"', '\'']).to_owned()
        }
    });
    (description.filter(|d| !d.is_empty()), body.trim_start_matches('\n'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn skill_bodies_drop_wrappers_and_frontmatter() {
        let op = loaded("session-reader", Some("abc"), "Base directory for this skill: /u/.claude/skills/session-reader\n\n# Session Reader\nRun it.");
        assert_eq!(op, Op::Skill { name: "session-reader".into(), args: Some("abc".into()), path: Some("/u/.claude/skills/session-reader".into()), description: None, body: Some("# Session Reader\nRun it.".into()) });

        let mut ops = vec![Op::read("/u/.agents/skills/ego/SKILL.md")];
        let result = json!({"content":"---\nname: ego\ndescription: >-\n  Drive the\n  browser.\n---\n\n# Ego\n"});
        promote(&mut ops, &result);
        assert_eq!(ops, [Op::Skill { name: "ego".into(), args: None, path: Some("/u/.agents/skills/ego".into()), description: Some("Drive the browser.".into()), body: Some("# Ego\n".into()) }]);
        assert!(json!(ops.pop().unwrap().summary()).get("body").is_none());
    }
}
