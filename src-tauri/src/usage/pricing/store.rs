//! 保存自定义价格和参考价缓存。保留损坏的自定义价格文件以便恢复。
use super::types::CustomPriceRecord;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::Write,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomPriceFile {
    pub version: u32,
    pub prices: BTreeMap<String, CustomPriceRecord>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CachedSourceMeta {
    pub fetched_at: i64,
}

pub fn default_data_dir() -> Option<PathBuf> {
    dirs::data_dir().map(|dir| dir.join("Agent Session Manager/pricing"))
}

pub fn default_cache_dir() -> Option<PathBuf> {
    dirs::cache_dir().map(|dir| dir.join("com.liangyukun.agent-session-manager/pricing"))
}

fn prepare_dir(dir: &Path) -> Result<(), String> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .map_err(|_| "无法创建价格目录")?;
    if fs::symlink_metadata(dir)
        .map_err(|_| "无法读取价格目录")?
        .file_type()
        .is_symlink()
    {
        return Err("价格目录不能是符号链接".into());
    }
    Ok(())
}

fn atomic_write(dir: &Path, name: &str, bytes: &[u8]) -> Result<(), String> {
    prepare_dir(dir)?;
    let target = dir.join(name);
    if let Ok(meta) = fs::symlink_metadata(&target) {
        if meta.file_type().is_symlink() {
            return Err(format!("价格文件 {name} 不能是符号链接"));
        }
    }
    let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp = dir.join(format!(".{name}-{}-{seq}.tmp", std::process::id()));
    let result = (|| -> Result<(), String> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&tmp)
            .map_err(|_| "无法创建价格临时文件")?;
        file.write_all(bytes).map_err(|_| "无法写入价格数据")?;
        file.sync_all().map_err(|_| "无法同步价格文件到磁盘")?;
        fs::rename(&tmp, &target).map_err(|_| "无法保存价格文件")?;
        File::open(dir)
            .and_then(|f| f.sync_all())
            .map_err(|_| "无法同步价格目录到磁盘".into())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

fn read_regular(path: &Path) -> Result<Option<Vec<u8>>, String> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("无法读取价格文件：{error}")),
        Ok(meta) if !meta.is_file() => return Err("价格文件必须是普通文件".into()),
        Ok(_) => {}
    }
    let mut file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|error| format!("无法打开价格文件：{error}"))?;
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut file, &mut bytes)
        .map_err(|error| format!("无法读取价格文件：{error}"))?;
    Ok(Some(bytes))
}

pub fn load_custom_prices(dir: &Path) -> Result<BTreeMap<String, CustomPriceRecord>, String> {
    let path = dir.join("custom-prices.json");
    let Some(bytes) = read_regular(&path)? else {
        return Ok(BTreeMap::new());
    };
    let parsed: CustomPriceFile =
        serde_json::from_slice(&bytes).map_err(|_| "自定义价格文件损坏，已保留原文件")?;
    if parsed.version != 1 {
        return Err("不支持的自定义价格文件版本，已保留原文件".into());
    }
    for (key, record) in &parsed.prices {
        if key.trim().is_empty() {
            return Err("自定义价格文件包含空模型 ID，已保留原文件".into());
        }
        let _ = CustomPriceRecord::from_input(
            super::types::CustomPriceInput {
                model: record.model.clone(),
                currency: record.currency,
                input: record.input,
                output: record.output,
                cache_read: record.cache_read,
                cache_write: record.cache_write,
                long_context: record.long_context.clone(),
            },
            record.updated_at,
        )
        .map_err(|_| "自定义价格文件包含无效数值，已保留原文件")?;
    }
    Ok(parsed.prices)
}

pub fn save_custom_prices(
    dir: &Path,
    prices: &BTreeMap<String, CustomPriceRecord>,
) -> Result<(), String> {
    let payload = CustomPriceFile {
        version: 1,
        prices: prices.clone(),
    };
    let bytes = serde_json::to_vec_pretty(&payload).map_err(|_| "无法编码自定义价格")?;
    atomic_write(dir, "custom-prices.json", &bytes)
}

pub fn load_cached_source(dir: &Path, slug: &str) -> Result<Option<(Vec<u8>, i64)>, String> {
    let data_path = dir.join(format!("{slug}.json"));
    let Some(raw) = read_regular(&data_path)? else {
        return Ok(None);
    };
    let meta_path = dir.join(format!("{slug}.meta.json"));
    let fetched_at = match read_regular(&meta_path) {
        Ok(Some(meta_bytes)) => serde_json::from_slice::<CachedSourceMeta>(&meta_bytes)
            .map(|m| m.fetched_at)
            .unwrap_or(0),
        _ => fs::metadata(&data_path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0),
    };
    Ok(Some((raw, fetched_at)))
}

pub fn save_cached_source(
    dir: &Path,
    slug: &str,
    raw: &[u8],
    fetched_at: i64,
) -> Result<(), String> {
    atomic_write(dir, &format!("{slug}.json"), raw)?;
    let meta = serde_json::to_vec(&CachedSourceMeta { fetched_at })
        .map_err(|_| "无法编码参考价缓存元数据")?;
    atomic_write(dir, &format!("{slug}.meta.json"), &meta)
}
