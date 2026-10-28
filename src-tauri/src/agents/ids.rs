//! 项目 ID 编码：Codex/OpenCode/Pi 用 cwd 合成项目身份。

/// 路径 → 项目 ID。
/// 编码必须无歧义：'/' → '-'；字面 '-' → "%2D"；字面 '%' → "%25"。
/// 输出中裸 '-' 只可能来自 '/'，"%XX" 只可能来自转义，
/// 因此不同路径绝不会碰撞（/a-/b ≠ /a/-b，/a-b/c ≠ /a/b-c）。
pub fn escape_path_id(path: &str) -> String {
    let mut out = String::with_capacity(path.len() + 8);
    for c in path.chars() {
        match c {
            '/' => out.push('-'),
            '-' => out.push_str("%2D"),
            '%' => out.push_str("%25"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::escape_path_id;

    #[test]
    fn path_id_is_collision_free() {
        // 旧实现会把这两对路径分别合并成同一项目
        assert_ne!(escape_path_id("/a-b/c"), escape_path_id("/a/b-c"));
        assert_ne!(escape_path_id("/a-/b"), escape_path_id("/a/-b"));
        assert_eq!(escape_path_id("/a-b/c"), "-a%2Db-c");
        assert_eq!(escape_path_id("/a/b-c"), "-a-b%2Dc");
        // 转义字符自身也要无歧义
        assert_ne!(escape_path_id("/a%2Db"), escape_path_id("/a-b"));
        assert_eq!(escape_path_id("/100%/done"), "-100%25-done");
    }
}
