//! OpenCode 数据源：~/.local/share/opencode/opencode.db（SQLite）。
//!
//! v1 使用 session/message/part，v2 使用 session_v2/session_message。
//! storage 在只读连接内建立兼容视图，统一读取两种 JSON 结构：
//! - 只读访问，删除委托给官方 `opencode session delete`（会一并清理子代理）
//! - 标题为占位符时回退到首条用户消息
//! - 会话 ID 形如 ses_[0-9A-Za-z]+

pub(crate) mod deferred;
#[cfg(test)]
mod event_tests;
mod events;
#[cfg(test)]
mod model_tests;
mod models;
mod storage;
#[cfg(test)]
mod v2_tests;

pub(crate) fn usage_model_names(ids: &[String]) -> std::collections::HashMap<String, String> {
    let names = models::names();
    ids.iter()
        .filter_map(|id| names.get(id).map(|name| (id.clone(), name.clone())))
        .collect()
}

use rusqlite::{Connection, OpenFlags};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

use super::{AgentSource, ConfigFile, SessionDetail, SessionMeta};

fn data_home() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
        if !xdg.is_empty() {
            return super::expand_tilde(&xdg).join("opencode");
        }
    }
    dirs::home_dir()
        .unwrap_or_default()
        .join(".local")
        .join("share")
        .join("opencode")
}

/// 配置目录（尊重 XDG_CONFIG_HOME，默认 ~/.config/opencode）。
/// 与数据目录（data_home）不同：配置在 config 根，会话库在 data 根。
fn config_home() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            return super::expand_tilde(&xdg).join("opencode");
        }
    }
    dirs::home_dir()
        .unwrap_or_default()
        .join(".config")
        .join("opencode")
}

fn db_path() -> PathBuf {
    data_home().join("opencode.db")
}

pub(crate) fn open_db(path: &Path) -> Result<Connection, String> {
    open_db_scoped(path, None)
}

fn open_db_scoped(path: &Path, session: Option<&str>) -> Result<Connection, String> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| format!("打开 OpenCode 会话库失败: {e}"))?;
    conn.busy_timeout(std::time::Duration::from_millis(300))
        .map_err(|e| e.to_string())?;
    storage::prepare(&conn, session).map_err(|e| format!("读取 OpenCode 数据结构失败: {e}"))?;
    Ok(conn)
}

fn is_valid_session_id(id: &str) -> bool {
    id.starts_with("ses_")
        && id.len() > 4
        && id.len() <= 64
        && id.chars().skip(4).all(|c| c.is_ascii_alphanumeric())
}

use super::ms_to_iso;

fn is_placeholder_title(title: &str) -> bool {
    let t = title.trim();
    if t.eq_ignore_ascii_case("new conversation") {
        return true;
    }
    (t.starts_with("New session - ") || t.starts_with("Child session - ")) && t.len() > 20
}

/// 消息计数共享的过滤条件：只数非 synthetic、有真实正文的 text part。
/// data 列可能存着损坏的 JSON：json_extract 碰上会直接报错中断整条查询，
/// 用 CASE 明确保证 json_valid 先于 json_extract，避免视图谓词下推重排求值。
/// 注意：被排除的损坏行绝不能当"不存在"——corrupt_sessions 会把它们
/// 按会话归属计入完整性状态，否则损坏记录会被静默隐藏
const REAL_MESSAGE_FILTER: &str = "CASE WHEN json_valid(m.data) THEN COALESCE(json_extract(m.data, '$.summary'), 0) != 1 ELSE 0 END";
const REAL_TEXT_FILTER: &str = "CASE WHEN json_valid(p.data) THEN
               json_extract(p.data, '$.type') = 'text'
               AND COALESCE(json_extract(p.data, '$.synthetic'), 0) = 0
               AND COALESCE(json_extract(p.data, '$.ignored'), 0) = 0
               AND TRIM(COALESCE(json_extract(p.data, '$.text'), '')) <> '' ELSE 0 END";

/// 按角色把计数累加进 (user, assistant) 元组；其余角色忽略
fn add_role_count(counts: &mut (u32, u32), role: &str, n: u32) {
    match role {
        "user" => counts.0 += n,
        "assistant" => counts.1 += n,
        _ => {}
    }
}

/// 按会话归属统计损坏记录数（message/part 的 data 不是合法 JSON）。
/// 查询失败返回 None：完整性未知，调用方必须按降级处理，
/// 绝不能把"查不了损坏"当成"没有损坏"
fn corrupt_sessions(conn: &Connection) -> Option<std::collections::HashMap<String, u32>> {
    let sql = "SELECT m.session_id, COUNT(*)
         FROM message m LEFT JOIN part p ON p.message_id = m.id
         WHERE NOT json_valid(m.data) OR (p.id IS NOT NULL AND NOT json_valid(p.data))
         GROUP BY m.session_id";
    let mut stmt = conn.prepare(sql).ok()?;
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, u32>(1)?)))
        .ok()?;
    let mut map = std::collections::HashMap::new();
    for row in rows {
        let Ok((id, n)) = row else { return None };
        *map.entry(id).or_insert(0) += n;
    }
    Some(map)
}

/// 一次性统计所有会话中"有真实文本"的用户/助手消息数（SQLite JSON1 库内完成），
/// 避免列表阶段对每个会话各跑两条查询。
/// 三态：Ok 完整；Degraded 部分行读取失败（保留已统计部分）；Failed 查询本身不可用。
/// 损坏 JSON 由 json_valid 守卫排除（不报错），但它们的归属与数量由
/// corrupt_sessions 单独计入完整性状态，查询返回前必须确认无损坏
fn batch_message_counts(
    conn: &Connection,
) -> super::ParseOutcome<std::collections::HashMap<String, (u32, u32)>> {
    let mut map = std::collections::HashMap::new();
    let sql = format!(
        "SELECT m.session_id, json_extract(m.data, '$.role'), COUNT(DISTINCT m.id)
         FROM message m JOIN part p ON p.message_id = m.id
         WHERE {REAL_MESSAGE_FILTER} AND {REAL_TEXT_FILTER}
         GROUP BY m.session_id, json_extract(m.data, '$.role')"
    );
    let mut stmt = match conn.prepare(&sql) {
        Ok(s) => s,
        Err(e) => return super::ParseOutcome::Failed(format!("消息计数查询准备失败: {e}")),
    };
    let rows = match stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, Option<String>>(1)?,
            r.get::<_, u32>(2)?,
        ))
    }) {
        Ok(r) => r,
        Err(e) => return super::ParseOutcome::Failed(format!("消息计数查询失败: {e}")),
    };
    let mut bad_rows = 0u32;
    for row in rows {
        let Ok((session_id, role, n)) = row else {
            bad_rows += 1;
            continue;
        };
        let entry = map.entry(session_id).or_insert((0, 0));
        add_role_count(entry, role.as_deref().unwrap_or(""), n);
    }
    if bad_rows > 0 {
        return super::ParseOutcome::Degraded(
            map,
            format!("OpenCode 有 {bad_rows} 条消息计数记录读取失败，消息数可能不准"),
        );
    }
    // json_valid 守卫排除了损坏行，但排除 ≠ 完整：
    // 存在损坏记录时计数结果必须判降级，否则损坏会话会被当空会话隐藏
    match corrupt_sessions(conn) {
        Some(corrupt) if !corrupt.is_empty() => {
            let total: u32 = corrupt.values().sum();
            super::ParseOutcome::Degraded(
                map,
                format!("OpenCode 有 {total} 条消息记录不是合法 JSON，相关会话的计数可能不完整"),
            )
        }
        Some(_) => super::ParseOutcome::Ok(map),
        None => super::ParseOutcome::Degraded(
            map,
            "OpenCode 损坏记录检测失败，会话计数完整性未知".to_string(),
        ),
    }
}

/// 单个会话的消息统计（详情视图与批量计数降级回退共用），一条 GROUP BY 查询完成。
/// 返回 None = 计数未知：查询失败、字段转换错误（如 role 类型异常），
/// 或该会话存在损坏记录（json_valid 检出）。转换错误必须传播：
/// 吞掉它们会让部分计数伪装成完整。计数未知绝不能当空会话处理，
/// 由调用方决定保底展示与告警。role 为 NULL 属合法（无角色分组，忽略）。
/// m.data 也加 json_valid 守卫（REAL_TEXT_FILTER 只 guard 了 p.data）。
fn session_message_counts(conn: &Connection, session_id: &str) -> Option<(u32, u32)> {
    // 该会话存在损坏记录：计数不完整，直接判未知（排除 ≠ 完整）
    let mut bad_stmt = conn
        .prepare(
            "SELECT COUNT(*) FROM message m LEFT JOIN part p ON p.message_id = m.id
             WHERE m.session_id = ?1 AND (NOT json_valid(m.data) OR (p.id IS NOT NULL AND NOT json_valid(p.data)))",
        )
        .ok()?;
    let bad: u32 = bad_stmt.query_row([session_id], |r| r.get(0)).ok()?;
    if bad > 0 {
        return None;
    }
    let mut counts = (0, 0);
    let sql = format!(
        "SELECT json_extract(m.data, '$.role'), COUNT(DISTINCT m.id)
         FROM message m JOIN part p ON p.message_id = m.id
         WHERE m.session_id = ?1
               AND {REAL_MESSAGE_FILTER}
               AND {REAL_TEXT_FILTER}
         GROUP BY json_extract(m.data, '$.role')"
    );
    let Ok(mut stmt) = conn.prepare(&sql) else {
        return None;
    };
    let Ok(rows) = stmt.query_map([session_id], |r| {
        // 字段转换错误用 ? 传播成行错误，绝不 unwrap_or_default 吞掉：
        // role 类型异常（如整数 42）意味着该分组计数丢失，必须判未知
        Ok((r.get::<_, Option<String>>(0)?, r.get::<_, u32>(1)?))
    }) else {
        return None;
    };
    for row in rows {
        // 行读取/转换失败 = 计数不完整，同样判未知
        let Ok((role, n)) = row else { return None };
        add_role_count(&mut counts, role.as_deref().unwrap_or(""), n);
    }
    Some(counts)
}

fn first_user_text(
    conn: &Connection,
    session_id: &str,
    warnings: &mut Vec<String>,
) -> Option<String> {
    let order = storage::order_column(conn);
    let sql = if storage::is_v2_session(conn, session_id) {
        // A placeholder title needs only user rows. Expanding the full content
        // view here would rescan every session's tools for each missing title.
        "SELECT '{\"role\":\"user\"}', CASE WHEN json_valid(data) THEN
            json_object('type','text','text',json_extract(data,'$.text')) ELSE '{' END
            FROM main.session_message WHERE session_id=?1 AND type='user' ORDER BY seq,id"
            .to_string()
    } else {
        format!(
            "SELECT m.data, p.data FROM message m JOIN part p ON p.message_id = m.id
             WHERE m.session_id = ?1
             ORDER BY m.{order}, m.id, p.time_created, p.id"
        )
    };
    let mut stmt = match conn.prepare(&sql) {
        Ok(s) => s,
        Err(e) => {
            warnings.push(format!("查询会话 {session_id} 标题失败: {e}"));
            return None;
        }
    };
    let rows = match stmt.query_map([session_id], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    }) {
        Ok(r) => r,
        Err(e) => {
            warnings.push(format!("查询会话 {session_id} 标题失败: {e}"));
            return None;
        }
    };
    let mut bad_rows = 0u32;
    let mut title: Option<String> = None;
    for row in rows {
        let Ok((msg_data, part_data)) = row else {
            bad_rows += 1;
            continue;
        };
        let Ok(msg) = serde_json::from_str::<Value>(&msg_data) else {
            bad_rows += 1;
            continue;
        };
        if msg.get("role").and_then(Value::as_str) != Some("user") {
            continue;
        }
        let Ok(part) = serde_json::from_str::<Value>(&part_data) else {
            bad_rows += 1;
            continue;
        };
        if part.get("type").and_then(Value::as_str) != Some("text") {
            continue;
        }
        if part.get("synthetic") == Some(&Value::Bool(true))
            || part.get("ignored") == Some(&Value::Bool(true))
        {
            continue;
        }
        // 找到标题后仍继续扫完全表：坏行数必须统计准确，
        // 告警绝不能因提前命中标题而丢失
        if title.is_none() {
            if let Some(text) = part.get("text").and_then(Value::as_str) {
                let t = text.trim();
                if !t.is_empty() {
                    title = Some(t.chars().take(80).collect());
                }
            }
        }
    }
    if bad_rows > 0 {
        warnings.push(format!(
            "会话 {session_id} 有 {bad_rows} 条消息记录读取/解析失败，标题可能不准"
        ));
    }
    title
}

struct SessionRow {
    id: String,
    title: Option<String>,
    directory: Option<String>,
    version: Option<String>,
    created_ms: Option<i64>,
    updated_ms: Option<i64>,
    is_child: bool,
    archived: bool,
}

/// 防御式读取 session 表：OpenCode 会随版本增删列，只取认识的字段。
/// 三态：Ok 完整；Degraded 有字段/行读取失败（保留已读部分）；
/// Failed 查询本身不可用。字段类型异常计入降级告警，
/// 绝不再经 .ok() 静默变成空值；列缺失仍属正常（旧版本库）。
fn load_session_rows(conn: &Connection) -> super::ParseOutcome<Vec<SessionRow>> {
    query_session_rows(conn, None)
}

fn query_session_rows(
    conn: &Connection,
    session_id: Option<&str>,
) -> super::ParseOutcome<Vec<SessionRow>> {
    // 分开准备 SQL，避免可空参数的 OR 条件使主键查询退化成全表扫描。
    // SELECT * 保留旧 schema 兼容；两种入口共用字段解码与降级告警。
    let sql = if session_id.is_some() {
        "SELECT * FROM session WHERE id = ?1"
    } else {
        "SELECT * FROM session"
    };
    let mut stmt = match conn.prepare(sql) {
        Ok(s) => s,
        Err(e) => {
            return super::ParseOutcome::Failed(format!("OpenCode session 表查询准备失败: {e}"))
        }
    };
    let names: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
    let mut bad_fields = 0u32;
    let rows = match stmt.query_map(rusqlite::params_from_iter(session_id), |r| {
        let pos = |name: &str| names.iter().position(|n| n == name);
        // 逐字段行内读取：类型异常计入降级告警（绝不静默变空值）；
        // 列缺失返回 None 属正常（旧版本库）。不用闭包包装，
        // 避免多个闭包同时持有 bad_fields 的可变借用
        macro_rules! field_text {
            ($name:literal) => {
                match pos($name) {
                    None => None,
                    Some(i) => match r.get_ref(i) {
                        Err(_) => {
                            bad_fields += 1;
                            None
                        }
                        Ok(rusqlite::types::ValueRef::Null) => None,
                        Ok(rusqlite::types::ValueRef::Text(t)) => {
                            Some(String::from_utf8_lossy(t).to_string())
                        }
                        Ok(_) => {
                            bad_fields += 1;
                            None
                        }
                    },
                }
            };
        }
        macro_rules! field_int {
            ($name:literal) => {
                match pos($name) {
                    None => None,
                    Some(i) => match r.get_ref(i) {
                        Err(_) => {
                            bad_fields += 1;
                            None
                        }
                        Ok(rusqlite::types::ValueRef::Null) => None,
                        Ok(rusqlite::types::ValueRef::Integer(n)) => Some(n),
                        Ok(_) => {
                            bad_fields += 1;
                            None
                        }
                    },
                }
            };
        }
        Ok(SessionRow {
            id: field_text!("id").unwrap_or_default(),
            title: field_text!("title"),
            directory: field_text!("directory"),
            version: field_text!("version"),
            created_ms: field_int!("time_created"),
            updated_ms: field_int!("time_updated"),
            is_child: field_text!("parent_id")
                .map(|p| !p.is_empty())
                .unwrap_or(false),
            archived: field_int!("time_archived").is_some(),
        })
    }) {
        Ok(r) => r,
        Err(e) => return super::ParseOutcome::Failed(format!("OpenCode session 表查询失败: {e}")),
    };
    let mut out = Vec::new();
    let mut bad_rows = 0u32;
    for row in rows {
        match row {
            Ok(r) => out.push(r),
            Err(_) => bad_rows += 1,
        }
    }
    let bad = bad_fields + bad_rows;
    if bad > 0 {
        return super::ParseOutcome::Degraded(
            out,
            format!(
                "OpenCode session 表有 {bad} 处字段/行读取失败（类型异常？），相关会话信息可能不准"
            ),
        );
    }
    super::ParseOutcome::Ok(out)
}

/// catalog 实现，数据库路径可注入（测试用 fixture 库）。
/// 批量计数/会话表读取均三态化：部分失败降级为告警 + 已有数据，
/// 不再让整个 catalog 因局部问题直接失败。
fn catalog_at(path: &Path, include_empty: bool) -> Result<super::Catalog, String> {
    let mut wal = path.as_os_str().to_os_string();
    wal.push("-wal");
    super::catalog_cache::database(
        &[path.to_path_buf(), PathBuf::from(wal)],
        include_empty,
        || scan_catalog_at(path, include_empty),
    )
}

fn scan_catalog_at(path: &Path, include_empty: bool) -> Result<super::Catalog, String> {
    if !path.exists() {
        return Ok(super::Catalog {
            projectless_session_ids: Vec::new(),
            session_group_hints: Vec::new(),
            projects: Vec::new(),
            sessions: Vec::new(),
            warnings: Vec::new(),
        });
    }
    let conn = open_db(path)?;
    let mut warnings = Vec::new();
    // 批量计数三态：必须保留"计数是否完整"——split() 会把 Ok 与
    // Degraded 拍平成同一个 Some(map)，丢失部分缺失的信号。
    // 只有完整计数才能把不在表中的会话判为空；降级/失败时
    // 已有条目也可能只是部分计数（坏行会丢掉某个 role 分组），
    // 不能仅凭存在就信任，一律逐会话重查
    let (counts_complete, counts) = match batch_message_counts(&conn) {
        super::ParseOutcome::Ok(map) => (true, map),
        super::ParseOutcome::Degraded(map, w) => {
            warnings.push(w);
            (false, map)
        }
        super::ParseOutcome::Failed(w) => {
            warnings.push(w);
            (false, std::collections::HashMap::new())
        }
    };
    // 降级时逐会话回退需要区分"有损坏记录"与"查询失败"，
    // 给出准确的告警；检测失败则 None，统一兜底措辞
    let corrupt = if counts_complete {
        None
    } else {
        corrupt_sessions(&conn)
    };
    let (rows, ws) = load_session_rows(&conn).split();
    warnings.extend(ws);
    let Some(rows) = rows else {
        // 会话表完全不可读：告警已记录，返回空目录
        return Ok(super::Catalog {
            projectless_session_ids: Vec::new(),
            session_group_hints: Vec::new(),
            projects: Vec::new(),
            sessions: Vec::new(),
            warnings,
        });
    };
    let historical_models = models::latest(&conn, None, &mut warnings);
    let mut missing_id = 0u32;
    let mut items = Vec::new();
    for row in rows {
        if row.id.is_empty() {
            missing_id += 1;
            continue;
        }
        if row.archived {
            continue;
        }
        // 子代理会话（有 parent_id）不作为普通可恢复会话展示
        if row.is_child {
            continue;
        }
        // 批量计数完整时，缺失条目 = 真正的空会话；
        // 降级/失败时已有条目也可能只是部分计数（如 role 类型异常的
        // 分组行读取失败，user 计数丢失只剩 assistant），
        // 不能仅凭存在就信任，一律逐会话重查。
        // 分支只决定计数值与是否绕过空会话过滤，元数据构造与入列统一收口
        let (cnt, bypass_empty_filter) = if counts_complete {
            (counts.get(&row.id).copied().unwrap_or((0, 0)), false)
        } else {
            match session_message_counts(&conn, &row.id) {
                Some(c) => (c, false),
                None => {
                    // 计数未知：保底展示 + 告警（区分损坏记录/查询失败），
                    // 绝不能把未知当空会话过滤（未知 ≠ 空）
                    let reason = match corrupt.as_ref().and_then(|c| c.get(&row.id)) {
                        Some(n) => format!("有 {n} 条消息记录不是合法 JSON"),
                        None => "消息计数查询失败或字段类型异常".to_string(),
                    };
                    warnings.push(format!(
                        "会话 {} {reason}，已按可见处理，计数可能不准",
                        row.id
                    ));
                    ((0, 0), true)
                }
            }
        };
        let meta = build_meta(
            &conn,
            &row,
            cnt,
            include_empty || bypass_empty_filter,
            &mut warnings,
        );
        let Some(mut meta) = meta else { continue };
        if let Some(model) = historical_models.get(&row.id) {
            model.apply(&mut meta);
        }
        let mut wal = path.as_os_str().to_os_string();
        wal.push("-wal");
        meta.source_files = vec![path.to_path_buf(), PathBuf::from(wal)];
        let display = row
            .directory
            .clone()
            .filter(|d| !d.is_empty())
            .unwrap_or_else(|| "(未知目录)".to_string());
        items.push((meta, display));
    }
    if missing_id > 0 {
        warnings.push(format!(
            "OpenCode 有 {missing_id} 条会话记录缺少 id，已跳过"
        ));
    }
    let mut catalog = super::assemble_catalog(items);
    catalog.warnings = warnings;
    Ok(catalog)
}

fn session_row(conn: &Connection, session_id: &str) -> Result<(SessionRow, Vec<String>), String> {
    let (rows, warnings) = query_session_rows(conn, Some(session_id)).split();
    let rows = rows.ok_or_else(|| warnings.join("；"))?;
    if rows.len() > 1 {
        return Err("OpenCode 会话 ID 不唯一，请检查会话库".into());
    }
    let row = rows.into_iter().next().ok_or_else(|| {
        if warnings.is_empty() {
            "未找到该 OpenCode 会话".into()
        } else {
            warnings.join("；")
        }
    })?;
    Ok((row, warnings))
}

fn read_session_at(path: &Path, session_id: &str) -> Result<SessionDetail, String> {
    read_session_mode_at(path, session_id, false)
}
fn read_session_mode_at(
    path: &Path,
    session_id: &str,
    view: bool,
) -> Result<SessionDetail, String> {
    if !is_valid_session_id(session_id) {
        return Err("非法会话 ID".into());
    }
    let db = open_db_scoped(path, Some(session_id))?;
    // 元数据、计数和分段使用同一个只读快照，CLI 同时追加时不会混合两个版本。
    let snapshot = db.unchecked_transaction().map_err(|e| e.to_string())?;
    let conn = &snapshot;
    let (row, mut warnings) = session_row(conn, session_id)?;
    let counts = session_message_counts(conn, &row.id).unwrap_or_else(|| {
        warnings.push("OpenCode 消息计数不完整，当前显示可读取内容".into());
        (0, 0)
    });
    let mut meta =
        build_meta(conn, &row, counts, true, &mut warnings).ok_or("未找到该 OpenCode 会话")?;
    if let Some(model) = models::latest(conn, Some(session_id), &mut warnings).get(session_id) {
        model.apply(&mut meta);
    }
    let (events, issues) = events::read(conn, path, session_id, view)?;
    warnings.extend(issues);
    Ok(SessionDetail {
        meta,
        events,
        warnings,
    })
}

fn search_events_at(path: &Path, meta: &SessionMeta) -> Result<Vec<Value>, String> {
    if !is_valid_session_id(&meta.session_id) {
        return Err("非法会话 ID".into());
    }
    let db = open_db_scoped(path, Some(&meta.session_id))?;
    let snapshot = db.unchecked_transaction().map_err(|e| e.to_string())?;
    // 只读取目标身份和有序事件；不重算标题、计数或查询模型目录。
    let (row, mut warnings) = session_row(&snapshot, &meta.session_id)?;
    let directory = row
        .directory
        .as_deref()
        .filter(|d| !d.is_empty())
        .unwrap_or("(unknown)");
    if row.id != meta.session_id
        || escape_dir(directory) != meta.project_dir
        || row.archived
        || row.is_child
    {
        return Err("会话来源已变化，请刷新后重试".into());
    }
    let (events, issues) = events::read(&snapshot, path, &meta.session_id, false)?;
    warnings.extend(issues);
    if !warnings.is_empty() {
        return Err(warnings.join("；"));
    }
    Ok(events)
}

pub struct OpenCodeSource;

impl AgentSource for OpenCodeSource {
    fn id(&self) -> &'static str {
        "opencode"
    }
    fn display_name(&self) -> &'static str {
        "OpenCode"
    }
    fn is_installed(&self) -> bool {
        db_path().exists()
    }
    fn data_dir(&self) -> Option<PathBuf> {
        Some(data_home())
    }
    fn watch_paths(&self, usage: bool) -> super::AgentWatchPaths {
        super::AgentWatchPaths {
            root_files: vec!["opencode.db", "opencode.db-wal"],
            root_file_patterns: if usage {
                vec![("opencode", ".db"), ("opencode", ".db-wal")]
            } else {
                vec![]
            },
            ..Default::default()
        }
    }
    fn supports_fork(&self) -> bool {
        true
    }
    fn delete_policy(&self) -> super::DeletePolicy {
        // 委托官方 CLI 删除，直接改库，无法进回收站
        super::DeletePolicy::Permanent
    }
    fn catalog(&self, include_empty: bool) -> Result<super::Catalog, String> {
        let mut catalog = catalog_at(&db_path(), include_empty)?;
        let names = models::names();
        for meta in &mut catalog.sessions {
            models::decorate_meta(meta, &names);
        }
        Ok(catalog)
    }
    fn read_tool_payload(&self, token: &str) -> Option<Result<Value, String>> {
        token
            .starts_with("opencode:")
            .then(|| deferred::read(token))
    }
    fn read_session_view(
        &self,
        _project_dir: &str,
        session_id: &str,
    ) -> Result<SessionDetail, String> {
        let mut detail = read_session_mode_at(&db_path(), session_id, true)?;
        models::decorate_detail(&mut detail, &models::names());
        Ok(detail)
    }
    fn read_session(&self, _project_dir: &str, session_id: &str) -> Result<SessionDetail, String> {
        let mut detail = read_session_at(&db_path(), session_id)?;
        models::decorate_detail(&mut detail, &models::names());
        Ok(detail)
    }
    fn search_events(&self, meta: &SessionMeta) -> Result<Vec<Value>, String> {
        search_events_at(&db_path(), meta)
    }
    fn delete_session(&self, _project_dir: &str, session_id: &str) -> Result<(), String> {
        if !is_valid_session_id(session_id) {
            return Err("非法会话 ID".into());
        }
        // 委托官方 CLI：会一并清理子代理会话，避免直接改库。
        // 先解析 opencode 绝对路径：GUI 应用从 Finder 启动只有精简 PATH，
        // 直接拿名字 spawn 会 ENOENT。删除是破坏性操作，找不到就显式报错
        let bin = super::find_executable("opencode").ok_or_else(|| {
            "找不到 opencode 可执行文件（请确认已安装；若已安装，可能是它不在应用可见的 PATH 中）"
                .to_string()
        })?;
        let output = std::process::Command::new(&bin)
            .args(["session", "delete", session_id])
            .env(
                "XDG_DATA_HOME",
                data_home()
                    .parent()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_default(),
            )
            .output()
            .map_err(|e| format!("调用 opencode 失败（{}）: {e}", bin.display()))?;
        if !output.status.success() {
            return Err(format!(
                "opencode session delete 失败: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        Ok(())
    }
    fn resume_command(&self, session_id: &str, fork: bool) -> Option<String> {
        Some(format!(
            "{} --session {}{}",
            super::shell_quote("opencode"),
            super::shell_quote(session_id),
            if fork { " --fork" } else { "" }
        ))
    }
    fn config_spec(&self) -> Vec<ConfigFile> {
        let dir = config_home();
        // OpenCode 优先读 opencode.jsonc，fallback 到 opencode.json；两者都纳入快照
        vec![
            ConfigFile {
                key: "opencode.jsonc".into(),
                label: "opencode.jsonc".into(),
                path: dir.join("opencode.jsonc"),
            },
            ConfigFile {
                key: "opencode.json".into(),
                label: "opencode.json".into(),
                path: dir.join("opencode.json"),
            },
        ]
    }
}

fn escape_dir(dir: &str) -> String {
    super::escape_path_id(dir)
}

fn build_meta(
    conn: &Connection,
    row: &SessionRow,
    counts: (u32, u32),
    include_empty: bool,
    warnings: &mut Vec<String>,
) -> Option<SessionMeta> {
    let (real_user, assistant_text) = counts;
    if !include_empty && real_user == 0 {
        return None;
    }
    let mut title = row.title.clone().filter(|t| !is_placeholder_title(t));
    if title.is_none() {
        title = first_user_text(conn, &row.id, warnings);
    }
    let dir = row
        .directory
        .clone()
        .filter(|d| !d.is_empty())
        .unwrap_or_else(|| "(unknown)".to_string());
    Some(SessionMeta {
        session_id: row.id.clone(),
        project_dir: escape_dir(&dir),
        cwd: row.directory.clone(),
        title,
        model: None,
        model_name: None,
        version: row.version.clone(),
        git_branch: None,
        entrypoint: None,
        first_ts: row.created_ms.and_then(ms_to_iso),
        last_ts: row.updated_ms.or(row.created_ms).and_then(ms_to_iso),
        user_count: real_user,
        assistant_count: assistant_text,
        real_user_count: real_user,
        assistant_text_count: assistant_text,
        size_bytes: 0,
        has_transcript: true,
        stats_pending: false,
        source_files: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cached_database_catalog_observes_wal_commits_and_archiving() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencode.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;
            CREATE TABLE session (id TEXT, title TEXT, directory TEXT, version TEXT,
                time_created INTEGER, time_updated INTEGER, parent_id TEXT, time_archived INTEGER);
            CREATE TABLE message (id TEXT, session_id TEXT, time_created INTEGER, data TEXT);
            CREATE TABLE part (id TEXT, message_id TEXT, time_created INTEGER, data TEXT);
            INSERT INTO session VALUES ('ses_cached', 'original', '/tmp/x', NULL, 1, 1, NULL, NULL);
            INSERT INTO message VALUES ('m1', 'ses_cached', 1, '{\"role\":\"user\"}');
            INSERT INTO part VALUES ('p1', 'm1', 1, '{\"type\":\"text\",\"text\":\"question\"}');
            PRAGMA wal_checkpoint(TRUNCATE);",
        )
        .unwrap();
        assert_eq!(
            catalog_at(&path, false).unwrap().sessions[0].assistant_text_count,
            0
        );
        let before = path.metadata().unwrap();
        conn.execute_batch(
            "INSERT INTO message VALUES ('m2', 'ses_cached', 2, '{\"role\":\"assistant\"}');
            INSERT INTO part VALUES ('p2', 'm2', 2, '{\"type\":\"text\",\"text\":\"answer\"}');
            UPDATE session SET title='renamed';",
        )
        .unwrap();
        let after = path.metadata().unwrap();
        assert_eq!(before.len(), after.len());
        assert_eq!(
            before.modified().unwrap(),
            after.modified().unwrap(),
            "更改只写入 WAL"
        );
        let updated = catalog_at(&path, false).unwrap();
        assert_eq!(updated.sessions[0].assistant_text_count, 1);
        assert_eq!(updated.sessions[0].title.as_deref(), Some("renamed"));
        conn.execute("UPDATE session SET time_archived=99", [])
            .unwrap();
        assert!(catalog_at(&path, false).unwrap().sessions.is_empty());
    }

    #[test]
    fn config_spec_resolves_opencode_jsonc_under_config_home() {
        // opencode.jsonc + opencode.json 均在配置根（config_home）
        let spec = OpenCodeSource.config_spec();
        assert_eq!(spec.len(), 2);
        assert_eq!(spec[0].key, "opencode.jsonc");
        assert_eq!(spec[1].key, "opencode.json");
        let dir = config_home();
        assert_eq!(spec[0].path, dir.join("opencode.jsonc"));
        assert_eq!(spec[1].path, dir.join("opencode.json"));
        assert_ne!(config_home(), data_home(), "配置目录与数据目录不同");
    }

    #[test]
    fn session_id_validation() {
        assert!(is_valid_session_id("ses_1abcDEF90"));
        assert!(!is_valid_session_id("ses_"));
        assert!(!is_valid_session_id("abc"));
        assert!(!is_valid_session_id("ses_a/b"));
    }

    #[test]
    fn delete_semantics_per_agent() {
        // OpenCode 委托官方 CLI 删除（永久）；其余 agent 移入回收站
        assert_eq!(
            OpenCodeSource.delete_policy(),
            crate::agents::DeletePolicy::Permanent
        );
        for s in crate::agents::all_sources() {
            if s.id() != "opencode" {
                assert_eq!(
                    s.delete_policy(),
                    crate::agents::DeletePolicy::Trash,
                    "agent {} 应移入回收站",
                    s.id()
                );
            }
        }
    }

    #[test]
    fn placeholder_titles() {
        assert!(is_placeholder_title(
            "New session - 2026-09-03T04:51:43.382Z"
        ));
        assert!(is_placeholder_title(
            "Child session - 2026-09-03T04:51:43.382Z"
        ));
        assert!(is_placeholder_title("New Conversation"));
        assert!(!is_placeholder_title("真正的标题"));
    }

    #[test]
    fn ms_to_iso_conversion() {
        assert_eq!(ms_to_iso(0), None);
        assert_eq!(
            ms_to_iso(1788417099162),
            Some("2026-09-03T06:31:39Z".to_string())
        );
    }

    /// 坏行告警绝不能因提前命中标题而丢失：
    /// 合法标题在前、损坏 part 在后，两者都必须被报告
    #[test]
    fn first_user_text_keeps_warning_after_early_title() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencode.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE message (id TEXT, session_id TEXT, time_created INTEGER, data TEXT);
             CREATE TABLE part (id TEXT, message_id TEXT, time_created INTEGER, data TEXT);",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO message VALUES ('m1', 'ses_t1', 1, '{\"role\":\"user\"}')",
            [],
        )
        .unwrap();
        // 第一条 part 即命中标题
        conn.execute(
            "INSERT INTO part VALUES ('p1', 'm1', 1, '{\"type\":\"text\",\"text\":\"标题在此\"}')",
            [],
        )
        .unwrap();
        // 后续 part 损坏：旧实现会在命中后提前返回，丢掉这条告警
        conn.execute(
            "INSERT INTO part VALUES ('p2', 'm1', 2, '{不是合法 JSON')",
            [],
        )
        .unwrap();

        let mut warnings = Vec::new();
        let title = first_user_text(&conn, "ses_t1", &mut warnings);
        assert_eq!(title.as_deref(), Some("标题在此"));
        assert_eq!(
            warnings.len(),
            1,
            "标题命中后的坏行告警不得丢失: {warnings:?}"
        );
        assert!(warnings[0].contains("1 条"), "{}", warnings[0]);
    }

    /// 字段类型异常必须显式降级，绝不再静默变空值
    #[test]
    fn load_session_rows_degrades_on_type_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencode.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE session (id TEXT, title TEXT, directory TEXT, version TEXT,
                time_created TEXT, time_updated INTEGER, parent_id TEXT, time_archived INTEGER);
             INSERT INTO session VALUES ('ses_bad', 't', '/tmp/x', '1.0', 'not-a-number', NULL, NULL, NULL);",
        )
        .unwrap();

        let crate::agents::ParseOutcome::Degraded(rows, w) = load_session_rows(&conn) else {
            panic!("类型异常必须判为 Degraded");
        };
        assert_eq!(rows.len(), 1, "已读行必须保留");
        assert!(rows[0].created_ms.is_none());
        assert!(w.contains("读取失败"), "{w}");
    }

    /// 一条损坏记录不得连坐隐藏健康会话，且损坏必须被告警：
    /// 损坏的 part JSON 被 json_valid 守卫排除后，由 corrupt_sessions
    /// 按会话归属计入完整性状态，批量计数判 Degraded。
    /// 旧实现（形状守卫）把缺失计数补成 (0,0)，include_empty=false 时
    /// 连健康会话一起被过滤掉，且损坏无任何告警
    #[test]
    fn catalog_degraded_counts_do_not_hide_healthy_sessions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencode.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE session (id TEXT, title TEXT, directory TEXT, version TEXT,
                time_created INTEGER, time_updated INTEGER, parent_id TEXT, time_archived INTEGER);
             CREATE TABLE message (id TEXT, session_id TEXT, time_created INTEGER, data TEXT);
             CREATE TABLE part (id TEXT, message_id TEXT, time_created INTEGER, data TEXT);",
        )
        .unwrap();
        // 损坏会话先插入（复现路径的原始构造，现已不影响结果）
        conn.execute(
            "INSERT INTO session VALUES ('ses_bad', NULL, '/tmp/x', NULL, 1, 1, NULL, NULL)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO message VALUES ('mb', 'ses_bad', 1, '{\"role\":\"user\"}')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO part VALUES ('pb', 'mb', 1, '{这不是合法 JSON')",
            [],
        )
        .unwrap();
        // 健康会话：有真实文本
        conn.execute(
            "INSERT INTO session VALUES ('ses_good', NULL, '/tmp/x', NULL, 1, 1, NULL, NULL)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO message VALUES ('mg', 'ses_good', 1, '{\"role\":\"user\"}')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO part VALUES ('pg', 'mg', 1, '{\"type\":\"text\",\"text\":\"健康会话消息\"}')",
            [],
        )
        .unwrap();
        drop(conn);

        let catalog = catalog_at(&path, false).unwrap();
        let ids: Vec<_> = catalog
            .sessions
            .iter()
            .map(|s| s.session_id.clone())
            .collect();
        assert!(
            ids.iter().any(|id| id == "ses_good"),
            "健康会话不得被损坏会话连坐隐藏: {ids:?}"
        );
        assert!(
            ids.iter().any(|id| id == "ses_bad"),
            "计数未知的会话不得被当空会话过滤（未知 ≠ 空）: {ids:?}"
        );
        assert!(
            catalog.warnings.iter().any(|w| w.contains("不是合法 JSON")),
            "损坏记录必须产生告警: {:?}",
            catalog.warnings
        );
    }

    /// 带前导空白（换行/制表符）的 data 是合法 JSON：
    /// 不得误判为损坏，更不得因此把会话当空会话隐藏
    #[test]
    fn catalog_counts_json_with_leading_whitespace() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencode.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE session (id TEXT, title TEXT, directory TEXT, version TEXT,
                time_created INTEGER, time_updated INTEGER, parent_id TEXT, time_archived INTEGER);
             CREATE TABLE message (id TEXT, session_id TEXT, time_created INTEGER, data TEXT);
             CREATE TABLE part (id TEXT, message_id TEXT, time_created INTEGER, data TEXT);",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session VALUES ('ses_ws', NULL, '/tmp/x', NULL, 1, 1, NULL, NULL)",
            [],
        )
        .unwrap();
        // message/part 的 data 都以 \n\t 开头：SQLite 默认 trim 只去空格，
        // 旧的形状守卫会把它们误判为非 JSON
        conn.execute(
            "INSERT INTO message VALUES ('mw', 'ses_ws', 1, '\n\t{\"role\":\"user\"}')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO part VALUES ('pw', 'mw', 1, '\n\t{\"type\":\"text\",\"text\":\"带前导空白的消息\"}')",
            [],
        )
        .unwrap();
        drop(conn);

        let catalog = catalog_at(&path, false).unwrap();
        let ids: Vec<_> = catalog
            .sessions
            .iter()
            .map(|s| s.session_id.clone())
            .collect();
        assert!(
            ids.iter().any(|id| id == "ses_ws"),
            "合法 JSON 不得被误删: {ids:?}"
        );
        assert!(
            catalog.warnings.is_empty(),
            "不得误报损坏: {:?}",
            catalog.warnings
        );
        let meta = catalog
            .sessions
            .iter()
            .find(|s| s.session_id == "ses_ws")
            .unwrap();
        assert_eq!(meta.real_user_count, 1, "带前导空白的消息必须被计数");
    }

    /// message.data 损坏也必须在逐会话回退中被检出（判未知，不得静默过滤）
    #[test]
    fn session_message_counts_unknown_when_message_data_corrupt() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencode.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE message (id TEXT, session_id TEXT, time_created INTEGER, data TEXT);
             CREATE TABLE part (id TEXT, message_id TEXT, time_created INTEGER, data TEXT);",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO message VALUES ('mc', 'ses_c', 1, 'not-json')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO part VALUES ('pc', 'mc', 1, '{\"type\":\"text\",\"text\":\"消息\"}')",
            [],
        )
        .unwrap();
        assert!(
            session_message_counts(&conn, "ses_c").is_none(),
            "message.data 损坏必须判计数未知"
        );
    }

    /// 批量降级时，map 里已有的条目也可能只是部分计数：
    /// role 类型异常（整数 42）让该分组行读取失败（user 计数丢失），
    /// assistant 分组成功 → map 留下 (0,1)。catalog 不得仅凭条目存在
    /// 就信任它，必须逐会话重查；重查因转换错误判未知 → 保底展示 + 告警
    #[test]
    fn catalog_requeries_partial_batch_entries_when_degraded() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencode.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE session (id TEXT, title TEXT, directory TEXT, version TEXT,
                time_created INTEGER, time_updated INTEGER, parent_id TEXT, time_archived INTEGER);
             CREATE TABLE message (id TEXT, session_id TEXT, time_created INTEGER, data TEXT);
             CREATE TABLE part (id TEXT, message_id TEXT, time_created INTEGER, data TEXT);",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session VALUES ('ses_p', NULL, '/tmp/x', NULL, 1, 1, NULL, NULL)",
            [],
        )
        .unwrap();
        // role 是整数 42：批量查询该分组行转换失败 → Degraded，
        // 但 assistant 分组成功 → map 里留下部分计数 (0,1)
        conn.execute(
            "INSERT INTO message VALUES ('m1', 'ses_p', 1, '{\"role\":42}')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO part VALUES ('p1', 'm1', 1, '{\"type\":\"text\",\"text\":\"异常角色消息\"}')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO message VALUES ('m2', 'ses_p', 2, '{\"role\":\"assistant\"}')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO part VALUES ('p2', 'm2', 2, '{\"type\":\"text\",\"text\":\"正常回复\"}')",
            [],
        )
        .unwrap();
        drop(conn);

        let catalog = catalog_at(&path, false).unwrap();
        let ids: Vec<_> = catalog
            .sessions
            .iter()
            .map(|s| s.session_id.clone())
            .collect();
        assert!(
            ids.iter().any(|id| id == "ses_p"),
            "部分计数条目不得被当空会话隐藏: {ids:?}"
        );
        assert!(
            catalog.warnings.iter().any(|w| w.contains("ses_p")),
            "计数未知必须告警: {:?}",
            catalog.warnings
        );
    }

    /// 逐会话查询必须传播字段转换错误：role 类型异常 → None（未知），
    /// 绝不能 unwrap_or_default 吞掉后返回 Some 伪装完整
    #[test]
    fn session_message_counts_propagates_field_conversion_errors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencode.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE message (id TEXT, session_id TEXT, time_created INTEGER, data TEXT);
             CREATE TABLE part (id TEXT, message_id TEXT, time_created INTEGER, data TEXT);",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO message VALUES ('m1', 'ses_r', 1, '{\"role\":42}')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO part VALUES ('p1', 'm1', 1, '{\"type\":\"text\",\"text\":\"消息\"}')",
            [],
        )
        .unwrap();
        assert!(
            session_message_counts(&conn, "ses_r").is_none(),
            "role 字段类型异常必须判计数未知"
        );
    }

    /// 用临时 SQLite 库验证会话发现与对话重组
    #[test]
    fn reads_sessions_from_fixture_db() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencode.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE session (id TEXT, title TEXT, directory TEXT, version TEXT,
                time_created INTEGER, time_updated INTEGER, parent_id TEXT, time_archived INTEGER);
             CREATE TABLE message (id TEXT, session_id TEXT, time_created INTEGER, data TEXT);
             CREATE TABLE part (id TEXT, message_id TEXT, time_created INTEGER, data TEXT);",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session VALUES ('ses_a1', 'New session - 2026-09-03T04:51:43.382Z', '/tmp/oc', '1.0', 1788417000000, 1788417600000, NULL, NULL)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO message VALUES ('m1', 'ses_a1', 1788417000000, '{\"role\":\"user\"}')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO part VALUES ('p1', 'm1', 1788417000000, '{\"type\":\"text\",\"text\":\"OpenCode 第一条消息\"}')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO message VALUES ('m2', 'ses_a1', 1788417100000, '{\"role\":\"assistant\"}')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO part VALUES ('p2', 'm2', 1788417100000, '{\"type\":\"text\",\"text\":\"收到\",\"synthetic\":false}')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO part VALUES ('p3', 'm2', 1788417100001, '{\"type\":\"text\",\"text\":\"工具管道文本\",\"synthetic\":true}')",
            [],
        )
        .unwrap();
        drop(conn);

        let conn = open_db(&path).unwrap();
        let (rows, ws) = load_session_rows(&conn).split();
        assert!(ws.is_empty(), "正常库不应产生降级告警: {ws:?}");
        let rows = rows.expect("fixture 库应可读");
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert_eq!(row.id, "ses_a1");

        let (u, a) = session_message_counts(&conn, "ses_a1").expect("fixture 库计数应可查");
        assert_eq!((u, a), (1, 1)); // synthetic 不计入

        let mut title_warnings = Vec::new();
        let title = first_user_text(&conn, "ses_a1", &mut title_warnings);
        assert_eq!(title.as_deref(), Some("OpenCode 第一条消息"));
        assert!(title_warnings.is_empty(), "正常数据不应产生告警");

        let mut warnings = Vec::new();
        let meta = build_meta(&conn, row, (u, a), false, &mut warnings).unwrap();
        // 占位标题被替换为首条用户消息
        assert_eq!(meta.title.as_deref(), Some("OpenCode 第一条消息"));
        assert_eq!(meta.project_dir, "-tmp-oc");
        assert_eq!(meta.first_ts.as_deref(), Some("2026-09-03T06:30:00Z"));
    }
}
