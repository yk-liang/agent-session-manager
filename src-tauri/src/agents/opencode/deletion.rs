//! 委托官方 `opencode session delete` 删除会话：每条命令删除一棵会话树，整批有限并行执行。
use super::{is_valid_session_id, lineage, load_session_rows, open_db, project_id, storage};
use crate::agents::deletion::{Outcome, PreserveUsage};
use crate::agents::search::SessionRef;
use crate::agents::find_executable;
use rusqlite::{types::ValueRef, Connection};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;

/// 每条命令都会启动一个私有服务，进程启动是主要开销；少量并行即可，避免争用会话库写锁。
const PARALLEL_COMMANDS: usize = 4;

pub(super) fn delete(
    database: &Path,
    sessions: &[SessionRef],
    preserve: &mut dyn PreserveUsage,
    outcome: &mut Outcome<'_>,
) -> Result<(), String> {
    let database = database.canonicalize().map_err(|error| format!("无法定位 OpenCode 会话库：{error}"))?;
    let cli = Cli::locate(&database)?;
    let conn = open_db(&database)?;
    let mut planned = Vec::new();
    for session in sessions {
        match read_tree(&conn, session) {
            Ok(tree) => planned.push((session, tree)),
            Err(error) => outcome.fail(session, error),
        }
    }
    drop(conn);
    if planned.is_empty() {
        return Ok(());
    }
    // 会话库会随本批删除而变化，删除前改为逐棵比对成员与原始行。
    preserve.save(std::slice::from_ref(&database)).map(drop)?;
    // 已包含在同批其他会话树中的请求随那棵树一起删除，不单独执行命令。
    let (covered, roots): (Vec<_>, Vec<_>) = planned.iter().partition(|(session, _)| {
        planned.iter().any(|(_, tree)| tree.members[1..].contains(session))
    });
    let mut settle = |(session, tree): &(&SessionRef, Tree), result: Result<(), String>| {
        let remaining = match remaining(&database, &tree.members) {
            Ok(remaining) => remaining,
            Err(error) => return outcome.fail(session, error),
        };
        let removed: Vec<_> = tree.members.iter().filter(|member| !remaining.contains(&member.session_id)).cloned().collect();
        let error = result.err().or_else(|| (removed.len() != tree.members.len()).then(|| "OpenCode CLI 未删除全部指定会话".into()));
        outcome.settle(session, removed, error);
    };
    in_parallel(&roots, |(session, tree)| {
        // 成员和用量来源都与保存时一致，保存的用量才覆盖这棵树；其他会话的活动不影响它。
        if read_tree(&open_db(&database)?, session)? != *tree {
            return Err("OpenCode 会话在保存用量后发生变化，请重试".into());
        }
        cli.delete(&session.session_id)
    }, |tree, result| settle(tree, result));
    for tree in covered {
        settle(tree, Ok(()));
    }
    Ok(())
}

/// 一棵待删会话树：成员（根在前）及其会话与消息原始行的指纹，用量正是从这些行读取。
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Tree {
    pub(super) members: Vec<SessionRef>,
    rows: [u8; 32],
}

/// 在同一个读事务中读取成员和指纹；任一会话行读取不完整时无法确认删除范围。
pub(super) fn read_tree(conn: &Connection, session: &SessionRef) -> Result<Tree, String> {
    if !is_valid_session_id(&session.session_id) {
        return Err("会话 ID 无效".into());
    }
    let snapshot = conn.unchecked_transaction().map_err(|error| format!("无法读取 OpenCode 会话库：{error}"))?;
    let (rows, warnings) = load_session_rows(&snapshot).split();
    if !warnings.is_empty() {
        return Err(warnings.join("；"));
    }
    let rows = rows.ok_or("无法读取 OpenCode 子会话关系")?;
    let mut matches = rows.iter().filter(|row| row.id == session.session_id);
    let row = matches.next().ok_or("未找到该 OpenCode 会话")?;
    if matches.next().is_some() {
        return Err("OpenCode 会话 ID 不唯一，请检查会话库".into());
    }
    if project_id(row) != session.project_dir {
        return Err("OpenCode 会话所属项目已变化，请刷新列表后重试".into());
    }
    let lineage = lineage(&rows);
    let members = lineage.subtree(&row.id)?.into_iter().map(|id| match lineage.location(id) {
        Some(location) if is_valid_session_id(id) => Ok(location.session_ref()),
        _ => Err(format!("OpenCode 子会话 {id} 的 ID 无效，无法确认删除范围")),
    }).collect::<Result<Vec<_>, String>>()?;
    let ids: Vec<_> = members.iter().map(|member| member.session_id.as_str()).collect();
    let mut digest = Sha256::new();
    for (table, key) in [("session_v2", "id"), ("session", "id"), ("session_message", "session_id"), ("message", "session_id")] {
        if !storage::exists(&snapshot, table).map_err(|error| error.to_string())? {
            continue;
        }
        query_ids(&snapshot, &format!("SELECT * FROM main.{table} WHERE {key} IN ({{}}) ORDER BY id"), &ids, |row| {
            for index in 0..row.as_ref().column_count() {
                match row.get_ref(index)? {
                    ValueRef::Null => digest.update([0]),
                    ValueRef::Integer(value) => digest.update([&[1][..], &value.to_le_bytes()].concat()),
                    ValueRef::Real(value) => digest.update([&[2][..], &value.to_le_bytes()].concat()),
                    ValueRef::Text(bytes) | ValueRef::Blob(bytes) => {
                        digest.update([&[3][..], &bytes.len().to_le_bytes()].concat());
                        digest.update(bytes);
                    }
                }
            }
            Ok(())
        })?;
    }
    Ok(Tree { members, rows: digest.finalize().into() })
}

fn remaining(database: &Path, members: &[SessionRef]) -> Result<HashSet<String>, String> {
    let ids: Vec<_> = members.iter().map(|member| member.session_id.as_str()).collect();
    let mut remaining = HashSet::new();
    query_ids(&open_db(database)?, "SELECT id FROM session WHERE id IN ({})", &ids, |row| {
        remaining.insert(row.get(0)?);
        Ok(())
    })?;
    Ok(remaining)
}

fn query_ids(
    conn: &Connection,
    sql: &str,
    ids: &[&str],
    mut row: impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<()>,
) -> Result<(), String> {
    let sql = sql.replace("{}", &vec!["?"; ids.len()].join(","));
    let mut read = || -> rusqlite::Result<()> {
        let mut statement = conn.prepare(&sql)?;
        let mut rows = statement.query(rusqlite::params_from_iter(ids))?;
        while let Some(value) = rows.next()? {
            row(value)?;
        }
        Ok(())
    };
    read().map_err(|error| format!("无法读取 OpenCode 会话库：{error}"))
}

/// 已探测过选项的删除命令，整批复用。
struct Cli {
    bin: PathBuf,
    database: PathBuf,
    standalone: bool,
}

impl Cli {
    fn locate(database: &Path) -> Result<Self, String> {
        let bin = find_executable("opencode").ok_or("找不到 opencode 可执行文件，请检查是否已安装")?;
        let help = Command::new(&bin).args(["session", "delete", "--help"]).output()
            .map_err(|error| format!("无法读取 OpenCode 删除命令选项：{error}"))?;
        if !help.status.success() {
            return Err("当前 OpenCode CLI 不支持会话删除".into());
        }
        // 独立进程才能保证数据库路径生效，避免连接到已运行的其他实例。
        let standalone = String::from_utf8_lossy(&help.stdout).lines()
            .any(|line| line.trim_start().starts_with("--standalone "));
        Ok(Self { bin, database: database.to_owned(), standalone })
    }

    fn delete(&self, session_id: &str) -> Result<(), String> {
        let mut command = Command::new(&self.bin);
        command.args(["session", "delete", session_id]).env("OPENCODE_DB", &self.database);
        if let Some(data_root) = self.database.parent().and_then(|dir| dir.ends_with("opencode").then(|| dir.parent()).flatten()) {
            command.env("XDG_DATA_HOME", data_root);
        }
        if self.standalone {
            command.arg("--standalone");
        }
        let output = command.output()
            .map_err(|error| format!("无法运行 OpenCode（{}）： {error}", self.bin.display()))?;
        match output.status.success() {
            true => Ok(()),
            false => Err(format!("opencode session delete 失败：{}", String::from_utf8_lossy(&output.stderr))),
        }
    }
}

/// 每完成一项就在当前线程回调 done。首条命令成功前逐条执行：独立服务启动时
/// 可能要恢复或迁移会话库，同时启动会相互冲突。
fn in_parallel<T: Sync>(items: &[T], work: impl Fn(&T) -> Result<(), String> + Sync, mut done: impl FnMut(&T, Result<(), String>)) {
    let mut serial = 0;
    while let Some(item) = items.get(serial) {
        serial += 1;
        let result = work(item);
        let succeeded = result.is_ok();
        done(item, result);
        if succeeded {
            break;
        }
    }
    let items = &items[serial..];
    let next = AtomicUsize::new(0);
    let (sender, receiver) = mpsc::channel();
    std::thread::scope(|scope| {
        for _ in 0..PARALLEL_COMMANDS.min(items.len()) {
            let (sender, next, work) = (sender.clone(), &next, &work);
            scope.spawn(move || {
                while let Some(item) = items.get(next.fetch_add(1, Ordering::Relaxed)) {
                    let _ = sender.send((item, work(item)));
                }
            });
        }
        drop(sender);
        for (item, result) in receiver {
            done(item, result);
        }
    });
}
