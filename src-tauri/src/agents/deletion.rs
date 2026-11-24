//! 批量删除会话：Adapter 一次确认整批删除范围，每项删除前保存并复核用量，再按 Agent 的方式批量执行。
use super::search::SessionRef;
use serde::Serialize;
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

/// 会话删除策略，决定可用操作和恢复方式。
#[derive(Serialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum DeletePolicy {
    /// 移入系统废纸篓，可还原
    Trash,
    /// 永久删除（如委托官方 CLI 直接改库），不可还原
    Permanent,
    ReadOnly,
}

#[derive(Debug, Serialize)]
pub struct DeletionFailure {
    pub session: SessionRef,
    pub error: String,
}

/// 实际移除的会话（含子会话），以及未能完整删除的请求会话。
#[derive(Debug, Default, Serialize)]
pub struct SessionDeletion {
    pub removed: Vec<SessionRef>,
    pub failures: Vec<DeletionFailure>,
}

/// 复核删除目标的日志仍与保存用量时一致。
pub type Unchanged = Box<dyn Fn() -> Result<(), String>>;

/// 删除动作必须晚于用量的持久化保存；保存失败或保存后日志又有变化的会话保留。
pub trait PreserveUsage {
    /// 保存这些删除目标当前的用量，返回删除前用于复核的检查。
    fn save(&mut self, targets: &[PathBuf]) -> Result<Unchanged, String>;
}

/// 每个请求会话恰好记录一次结果，结果确定后上报已处理的数量。
pub struct Outcome<'a> {
    requested: &'a [SessionRef],
    settled: HashSet<SessionRef>,
    deletion: SessionDeletion,
    progress: &'a dyn Fn(usize),
}

impl<'a> Outcome<'a> {
    pub fn new(requested: &'a [SessionRef], progress: &'a dyn Fn(usize)) -> Self {
        Self { requested, settled: HashSet::new(), deletion: SessionDeletion::default(), progress }
    }

    /// error 为 None 表示该请求会话的整棵会话树都已移除。
    pub fn settle(&mut self, session: &SessionRef, removed: impl IntoIterator<Item = SessionRef>, error: Option<String>) {
        if !self.settled.insert(session.clone()) {
            return;
        }
        self.deletion.removed.extend(removed);
        if let Some(error) = error {
            self.deletion.failures.push(DeletionFailure { session: session.clone(), error });
        }
        (self.progress)(self.settled.len());
    }

    pub fn fail(&mut self, session: &SessionRef, error: impl Into<String>) {
        self.settle(session, [], Some(error.into()));
    }

    /// 整批中止时，尚无结果的请求会话都以该错误结束。
    pub fn fail_rest(&mut self, error: &str) {
        for session in self.requested {
            self.fail(session, error);
        }
    }

    pub fn finish(mut self) -> SessionDeletion {
        let mut seen = HashSet::new();
        self.deletion.removed.retain(|session| seen.insert(session.clone()));
        self.deletion
    }
}

/// 移入废纸篓的范围：targets 父在前，sessions 是其中每个会话 ID 与它的日志或状态目录。
#[derive(Debug, Default)]
pub struct TrashPlan {
    pub targets: Vec<PathBuf>,
    pub sessions: Vec<(String, PathBuf)>,
}

impl TrashPlan {
    pub fn add_session(&mut self, id: String, path: PathBuf) {
        self.sessions.push((id, path.clone()));
        self.targets.push(path);
    }

    /// 日志已全部移走的会话；附属文件也都移走才算完整。
    fn removed(&self, project_dir: &str) -> (Vec<SessionRef>, bool) {
        let mut absent = BTreeMap::new();
        for (id, path) in &self.sessions {
            *absent.entry(id).or_insert(true) &= missing(path);
        }
        let removed = absent.into_iter().filter(|(_, missing)| *missing)
            .map(|(id, _)| SessionRef { project_dir: project_dir.to_owned(), session_id: id.clone() })
            .collect();
        (removed, self.targets.iter().all(|path| missing(path)))
    }
}

fn missing(path: &Path) -> bool {
    path.symlink_metadata().is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
}

/// 先逐项保存用量，全部复核后再把整批文件交给 Finder 一次移入废纸篓：每次调用都有固定开销。
pub fn trash(
    sessions: &[SessionRef],
    plan: impl Fn(&SessionRef) -> Result<TrashPlan, String>,
    preserve: &mut dyn PreserveUsage,
    outcome: &mut Outcome<'_>,
) -> Result<(), String> {
    let mut planned = Vec::new();
    for session in sessions {
        match plan(session).and_then(|scope| Ok((preserve.save(&scope.targets)?, scope))) {
            Ok((unchanged, scope)) => planned.push((session, scope, unchanged)),
            Err(error) => outcome.fail(session, error),
        }
    }
    planned.retain(|(session, scope, _)| match plan(session) {
        Ok(current) if current.targets == scope.targets => true,
        Ok(_) => {
            outcome.fail(session, "会话文件已变化，请重新确认后删除");
            false
        }
        Err(error) => {
            outcome.fail(session, error);
            false
        }
    });
    // 最后只比对文件版本，不再重新解析日志。
    planned.retain(|(session, _, unchanged)| unchanged().map_err(|error| outcome.fail(session, error)).is_ok());
    if planned.is_empty() {
        return Ok(());
    }
    // 已包含在同批其他会话树中的请求随那棵树一起移走。
    let covered: HashSet<(&str, &str)> = planned.iter()
        .flat_map(|(session, scope, _)| scope.sessions.iter()
            .filter(|(id, _)| *id != session.session_id)
            .map(|(id, _)| (session.project_dir.as_str(), id.as_str())))
        .collect();
    let mut seen = HashSet::new();
    let targets: Vec<_> = planned.iter()
        .filter(|(session, _, _)| !covered.contains(&(session.project_dir.as_str(), session.session_id.as_str())))
        .flat_map(|(_, scope, _)| &scope.targets)
        .filter(|path| seen.insert(*path))
        .collect();
    // 子会话先于父会话移走，中途失败时父会话仍可见。
    let error = trash::delete_all(targets.into_iter().rev()).err()
        .map(|error| format!("移入废纸篓失败：{error}"));
    for (session, scope, _) in planned {
        let (removed, complete) = scope.removed(&session.project_dir);
        let error = (!complete).then(|| error.clone().unwrap_or_else(|| "部分会话文件未能移入废纸篓".into()));
        outcome.settle(session, removed, error);
    }
    Ok(())
}
