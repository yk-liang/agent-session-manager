//! 删除前安全校验：符号链接、越界、目标类型；严格存在性探测。

use std::path::PathBuf;

/// 严格存在性探测：只有 NotFound 才算"不存在"，其余探测错误显性化。
/// 悬空符号链接也算存在（随后由具体场景的校验器拒绝），
/// 避免跳过它却处理另一半造成部分生效。
pub fn present(path: &std::path::Path) -> Result<bool, String> {
    match path.symlink_metadata() {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(format!("无法读取 {:?}: {e}", path)),
    }
}

/// 删除目标的预期类型
pub enum TargetKind {
    /// 普通文件（JSONL 转录等）
    File,
    /// 目录（会话状态目录等）
    Dir,
}

/// 删除前安全校验：目标自身不得是符号链接，canonicalize 后
/// 必须仍位于数据根目录内（防止祖先目录符号链接把删除引出树外），
/// 且目标类型必须符合预期（普通文件 / 目录），拒绝 FIFO 等特殊文件。
/// 通过则返回规范化后的目标路径。
pub fn verify_within_root(
    root: &std::path::Path,
    target: &std::path::Path,
    kind: TargetKind,
) -> Result<PathBuf, String> {
    let meta = target
        .symlink_metadata()
        .map_err(|e| format!("无法读取目标 {:?}: {e}", target))?;
    if meta.file_type().is_symlink() {
        return Err(format!("拒绝删除符号链接: {:?}", target));
    }
    let root_c = root
        .canonicalize()
        .map_err(|e| format!("无法解析数据根目录 {:?}: {e}", root))?;
    let target_c = target
        .canonicalize()
        .map_err(|e| format!("无法解析目标路径 {:?}: {e}", target))?;
    if !target_c.starts_with(&root_c) {
        return Err(format!("目标路径越出数据根目录: {:?}", target_c));
    }
    let m = target_c
        .metadata()
        .map_err(|e| format!("无法读取目标 {:?}: {e}", target_c))?;
    match kind {
        TargetKind::Dir if !m.is_dir() => {
            return Err(format!("目标应为目录，实际不是: {:?}", target_c));
        }
        TargetKind::File if !m.is_file() => {
            return Err(format!("目标应为普通文件，实际不是: {:?}", target_c));
        }
        _ => {}
    }
    Ok(target_c)
}

#[cfg(test)]
mod tests {
    use super::{verify_within_root, TargetKind};

    #[test]
    fn verify_within_root_rules() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("root");
        let outside = tmp.path().join("outside");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();

        // 正常文件：通过
        let ok_file = root.join("ok.jsonl");
        std::fs::write(&ok_file, "x").unwrap();
        assert!(verify_within_root(&root, &ok_file, TargetKind::File).is_ok());

        // 类型不符：把目录当文件删、把文件当目录删，都要拒绝
        let err =
            verify_within_root(&root, &root.join("sub_missing"), TargetKind::File).unwrap_err();
        assert!(err.contains("无法读取目标"), "{err}");
        std::fs::create_dir_all(root.join("adir")).unwrap();
        let err = verify_within_root(&root, &root.join("adir"), TargetKind::File).unwrap_err();
        assert!(err.contains("应为普通文件"), "{err}");
        let err = verify_within_root(&root, &ok_file, TargetKind::Dir).unwrap_err();
        assert!(err.contains("应为目录"), "{err}");

        // 目标自身是符号链接：拒绝
        let link = root.join("link.jsonl");
        std::os::unix::fs::symlink(&ok_file, &link).unwrap();
        let err = verify_within_root(&root, &link, TargetKind::File).unwrap_err();
        assert!(err.contains("符号链接"), "{err}");

        // 祖先符号链接指向树外：canonicalize 后越界，拒绝
        let victim = outside.join("victim.jsonl");
        std::fs::write(&victim, "x").unwrap();
        let escape = root.join("escape");
        std::os::unix::fs::symlink(&outside, &escape).unwrap();
        let err =
            verify_within_root(&root, &escape.join("victim.jsonl"), TargetKind::File)
                .unwrap_err();
        assert!(err.contains("越出数据根目录"), "{err}");
        assert!(victim.exists(), "树外文件必须完好");
    }
}
