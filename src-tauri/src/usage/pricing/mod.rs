//! 每次费用查询使用同一版本的定价快照。价格更新时可继续读取已取得的快照。
mod catalog;
mod fetch;
mod litellm;
mod models;
mod models_dev;
mod resolve;
mod store;
#[cfg(test)]
mod tests;
pub mod types;

#[cfg(test)]
use super::events::RequestEvent;
use catalog::{model_key, SourceCatalog};
use chrono::Utc;
pub use resolve::PricingSnapshot;
pub(super) use resolve::{BUILTIN_SOURCE, NO_USAGE_SOURCE};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock, RwLock},
};
pub use types::*;

const CATALOG_MAX_AGE_MS: i64 = 24 * 60 * 60 * 1000;
struct SourceSlot {
    id: &'static str,
    name: &'static str,
    slug: &'static str,
    url: &'static str,
    catalog: Option<Arc<SourceCatalog>>,
    fetched_at: Option<i64>,
    error: Option<String>,
}

impl SourceSlot {
    fn new(id: &'static str, name: &'static str, slug: &'static str, url: &'static str) -> Self {
        Self {
            id,
            name,
            slug,
            url,
            catalog: None,
            fetched_at: None,
            error: None,
        }
    }

    fn is_stale(&self, now: i64) -> bool {
        match (self.catalog.as_ref(), self.fetched_at) {
            (Some(_), Some(at)) if at > 0 => now.saturating_sub(at) > CATALOG_MAX_AGE_MS,
            _ => true,
        }
    }

    fn status(&self, now: i64) -> SourceStatus {
        SourceStatus {
            id: self.id.to_owned(),
            name: self.name.to_owned(),
            url: self.url.to_owned(),
            fetched_at: self.fetched_at,
            model_count: self.catalog.as_ref().map_or(0, |c| c.listings.len()),
            priced_count: self.catalog.as_ref().map_or(0, |c| c.priced_count),
            stale: self.is_stale(now),
            error: self.error.clone(),
        }
    }
}

struct PricingState {
    revision: u64,
    custom: Arc<BTreeMap<String, CustomPriceRecord>>,
    custom_error: Option<String>,
    models_dev: SourceSlot,
    litellm: SourceSlot,
    snapshot: Arc<PricingSnapshot>,
}

impl PricingState {
    fn new() -> Self {
        let custom = Arc::new(BTreeMap::new());
        let snapshot = Arc::new(PricingSnapshot {
            revision: 1,
            custom: Arc::clone(&custom),
            models_dev: None,
            litellm: None,
        });
        Self {
            revision: 1,
            custom,
            custom_error: None,
            models_dev: SourceSlot::new(
                models_dev::SOURCE_ID,
                models_dev::SOURCE_NAME,
                "models-dev",
                fetch::MODELS_DEV_URL,
            ),
            litellm: SourceSlot::new(
                litellm::SOURCE_ID,
                litellm::SOURCE_NAME,
                "litellm",
                fetch::LITELLM_URL,
            ),
            snapshot,
        }
    }

    /// 原子发布新快照，保证 revision 与 custom/models_dev/litellm 引用完全同步。
    fn publish(&mut self, bump_revision: bool) {
        if bump_revision {
            self.revision += 1;
        }
        self.snapshot = Arc::new(PricingSnapshot {
            revision: self.revision,
            custom: Arc::clone(&self.custom),
            models_dev: self.models_dev.catalog.clone(),
            litellm: self.litellm.catalog.clone(),
        });
    }

    fn status(&self, now: i64) -> PricingCatalogStatus {
        PricingCatalogStatus {
            revision: self.revision,
            sources: vec![self.models_dev.status(now), self.litellm.status(now)],
            custom_count: self.custom.len(),
            custom_error: self.custom_error.clone(),
            exchange_rate_cny_per_usd: CNY_PER_USD,
        }
    }
}

pub struct Pricer {
    data_dir: Option<PathBuf>,
    cache_dir: Option<PathBuf>,
    state: RwLock<PricingState>,
    refresh_lock: Mutex<()>,
    custom_lock: Mutex<()>,
}

impl Pricer {
    pub fn open(data_dir: Option<PathBuf>, cache_dir: Option<PathBuf>) -> Self {
        let mut state = PricingState::new();
        if let Some(dir) = &data_dir {
            match store::load_custom_prices(dir) {
                Ok(prices) => state.custom = Arc::new(prices),
                Err(error) => state.custom_error = Some(error),
            }
        }
        if let Some(dir) = &cache_dir {
            load_slot_from_cache(dir, &mut state.models_dev, models_dev::parse);
            load_slot_from_cache(dir, &mut state.litellm, litellm::parse);
        }
        state.publish(false);
        Self {
            data_dir,
            cache_dir,
            state: RwLock::new(state),
            refresh_lock: Mutex::new(()),
            custom_lock: Mutex::new(()),
        }
    }

    pub fn snapshot(&self) -> Arc<PricingSnapshot> {
        Arc::clone(
            &self
                .state
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .snapshot,
        )
    }

    pub fn is_stale(&self) -> bool {
        let now = Utc::now().timestamp_millis();
        let state = self.state.read().unwrap_or_else(|e| e.into_inner());
        state.models_dev.is_stale(now) || state.litellm.is_stale(now)
    }

    #[cfg(test)]
    pub fn status(&self) -> PricingCatalogStatus {
        let now = Utc::now().timestamp_millis();
        self.state
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .status(now)
    }

    #[cfg(test)]
    pub fn resolve(
        &self,
        model: &str,
        provider_hint: Option<&str>,
        at_ms: i64,
    ) -> Option<ResolvedPrice> {
        self.snapshot().resolve(model, provider_hint, at_ms)
    }

    #[cfg(test)]
    pub fn evaluate_event_cost(&self, event: &RequestEvent) -> Option<EventCost> {
        self.snapshot().evaluate_event_cost(event)
    }

    pub fn set_custom_price(
        &self,
        input: CustomPriceInput,
    ) -> Result<PricingCatalogStatus, String> {
        let now = Utc::now().timestamp_millis();
        let record = CustomPriceRecord::from_input(input, now)?;
        let key = model_key(&record.model);
        let _guard = self.custom_lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut next = if let Some(dir) = &self.data_dir {
            store::load_custom_prices(dir)?
        } else {
            (*self.state.read().unwrap_or_else(|e| e.into_inner()).custom).clone()
        };
        next.insert(key, record);
        if let Some(dir) = &self.data_dir {
            store::save_custom_prices(dir, &next)?;
        }
        let mut state = self.state.write().unwrap_or_else(|e| e.into_inner());
        state.custom = Arc::new(next);
        state.custom_error = None;
        state.publish(true);
        Ok(state.status(now))
    }

    pub fn delete_custom_price(&self, model: &str) -> Result<PricingCatalogStatus, String> {
        let key = model_key(model);
        if key.is_empty() {
            return Err("请输入模型 ID".into());
        }
        let now = Utc::now().timestamp_millis();
        let _guard = self.custom_lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut next = if let Some(dir) = &self.data_dir {
            store::load_custom_prices(dir)?
        } else {
            (*self.state.read().unwrap_or_else(|e| e.into_inner()).custom).clone()
        };
        next.remove(&key);
        if let Some(dir) = &self.data_dir {
            store::save_custom_prices(dir, &next)?;
        }
        let mut state = self.state.write().unwrap_or_else(|e| e.into_inner());
        state.custom = Arc::new(next);
        state.custom_error = None;
        state.publish(true);
        Ok(state.status(now))
    }

    pub fn refresh(&self) -> PricingCatalogStatus {
        self.refresh_with_fetcher(fetch::fetch_public_catalog)
    }

    pub fn refresh_if_stale(&self) -> Option<PricingCatalogStatus> {
        if !self.is_stale() {
            return None;
        }
        Some(self.refresh())
    }

    pub fn refresh_with_fetcher(
        &self,
        fetcher: impl Fn(&str) -> Result<Vec<u8>, String>,
    ) -> PricingCatalogStatus {
        let _guard = self.refresh_lock.lock().unwrap_or_else(|e| e.into_inner());
        // 下载、解析和写盘全部在状态锁之外执行；单源失败不丢弃该源的上一次有效缓存。
        let md_result = fetch_and_parse(
            fetch::MODELS_DEV_URL,
            "models-dev",
            self.cache_dir.as_deref(),
            models_dev::parse,
            &fetcher,
        );
        let ll_result = fetch_and_parse(
            fetch::LITELLM_URL,
            "litellm",
            self.cache_dir.as_deref(),
            litellm::parse,
            &fetcher,
        );
        let now = Utc::now().timestamp_millis();
        let mut state = self.state.write().unwrap_or_else(|e| e.into_inner());
        let mut updated = false;
        updated |= apply_slot_update(&mut state.models_dev, md_result);
        updated |= apply_slot_update(&mut state.litellm, ll_result);
        if updated {
            state.publish(true);
        }
        state.status(now)
    }

    pub fn search_reference(&self, query: &str, limit: usize) -> Vec<ReferenceSearchItem> {
        let limit = limit.clamp(1, 100);
        let snapshot = self.snapshot();
        let mut items = Vec::new();
        if let Some(catalog) = snapshot.models_dev.as_deref() {
            items.extend(catalog.search(query, limit));
        }
        if let Some(catalog) = snapshot.litellm.as_deref() {
            items.extend(catalog.search(query, limit));
        }
        let exact = model_key(query);
        items.sort_by(|a, b| {
            let score = |item: &ReferenceSearchItem| {
                let mut rank = 0;
                if catalog::bare_model_key(&item.model_id) == exact {
                    rank -= 4;
                }
                if item.source == models_dev::SOURCE_ID {
                    rank -= 1;
                }
                rank
            };
            score(a)
                .cmp(&score(b))
                .then_with(|| a.provider.cmp(&b.provider))
                .then_with(|| a.model_id.cmp(&b.model_id))
        });
        items.truncate(limit);
        items
    }
}

fn fetch_and_parse(
    url: &str,
    slug: &str,
    cache_dir: Option<&std::path::Path>,
    parser: fn(&[u8]) -> Result<SourceCatalog, String>,
    fetcher: &impl Fn(&str) -> Result<Vec<u8>, String>,
) -> Result<(Arc<SourceCatalog>, i64), String> {
    let raw = fetcher(url)?;
    let parsed = Arc::new(parser(&raw)?);
    let now = Utc::now().timestamp_millis();
    if let Some(dir) = cache_dir {
        store::save_cached_source(dir, slug, &raw, now)?;
    }
    Ok((parsed, now))
}

fn apply_slot_update(
    slot: &mut SourceSlot,
    outcome: Result<(Arc<SourceCatalog>, i64), String>,
) -> bool {
    match outcome {
        Ok((catalog, now)) => {
            slot.catalog = Some(catalog);
            slot.fetched_at = Some(now);
            slot.error = None;
            true
        }
        Err(error) => {
            slot.error = Some(error);
            false
        }
    }
}

fn load_slot_from_cache(
    dir: &std::path::Path,
    slot: &mut SourceSlot,
    parser: fn(&[u8]) -> Result<SourceCatalog, String>,
) {
    match store::load_cached_source(dir, slot.slug) {
        Ok(Some((raw, fetched_at))) => match parser(&raw) {
            Ok(catalog) => {
                slot.catalog = Some(Arc::new(catalog));
                slot.fetched_at = (fetched_at > 0).then_some(fetched_at);
                slot.error = None;
            }
            Err(error) => {
                slot.error = Some(format!("本地参考价缓存损坏：{error}"));
            }
        },
        Ok(None) => {}
        Err(error) => {
            slot.error = Some(error);
        }
    }
}

pub fn global() -> &'static Pricer {
    static PRICER: OnceLock<Pricer> = OnceLock::new();
    PRICER.get_or_init(|| {
        if cfg!(test) {
            Pricer::open(None, None)
        } else {
            Pricer::open(store::default_data_dir(), store::default_cache_dir())
        }
    })
}

pub fn list_model_prices(agent: Option<&str>) -> Result<ModelPricesResponse, String> {
    let db = super::ledger::reader(&super::ledger::path()?)?;
    global().list_model_prices_from_db(&db, agent)
}

pub fn set_custom_price(input: CustomPriceInput) -> Result<PricingCatalogStatus, String> {
    global().set_custom_price(input)
}

pub fn delete_custom_price(model: &str) -> Result<PricingCatalogStatus, String> {
    global().delete_custom_price(model)
}

pub fn refresh_reference_prices() -> PricingCatalogStatus {
    global().refresh()
}

pub fn refresh_if_stale() -> Option<PricingCatalogStatus> {
    global().refresh_if_stale()
}

pub fn search_reference_prices(query: &str, limit: usize) -> Vec<ReferenceSearchItem> {
    global().search_reference(query, limit)
}
