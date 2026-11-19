use crate::usage::ingest;
use super::{
    fetch::{build_curl_args, fetch_public_catalog, LITELLM_URL, MODELS_DEV_URL},
    *,
};
use crate::usage::{events, request_parse::{RequestMeta, RequestOutcome}, summary, types::*, Store};
use serde_json::json;
use std::{
    fs,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Barrier,
    },
    thread,
};

fn sample_models_dev() -> Vec<u8> {
    serde_json::to_vec(&json!({
        "anthropic": {
            "id": "anthropic",
            "models": {
                "claude-sonnet-4-5": {
                    "id": "claude-sonnet-4-5",
                    "cost": {
                        "input": 3.0,
                        "output": 15.0,
                        "cache_read": 0.3,
                        "cache_write": 3.75,
                        "context_over_200k": {
                            "input": 6.0,
                            "output": 22.5,
                            "cache_read": 0.6,
                            "cache_write": 7.5
                        }
                    }
                },
                "claude-nominal-zero": {
                    "id": "claude-nominal-zero",
                    "cost": { "input": 0.0, "output": 0.0 }
                },
                "claude-audio-special": {
                    "id": "claude-audio-special",
                    "cost": { "input": 3.0, "output": 15.0, "input_audio": 10.0 }
                }
            }
        },
        "openai": {
            "id": "openai",
            "models": {
                "gpt-5-codex": {
                    "id": "gpt-5-codex",
                    "cost": {
                        "input": 2.0,
                        "output": 8.0,
                        "cache_read": 0.2
                    }
                }
            }
        },
        "github-copilot": {
            "id": "github-copilot",
            "models": {
                "claude-sonnet-4-5": {
                    "id": "claude-sonnet-4-5",
                    "cost": { "input": 0.0, "output": 0.0 }
                }
            }
        },
        "openrouter": {
            "id": "openrouter",
            "models": {
                "anthropic/claude-sonnet-4-5": {
                    "id": "anthropic/claude-sonnet-4-5",
                    "cost": { "input": 3.5, "output": 16.0 }
                }
            }
        },
        "vendor-a": {
            "id": "vendor-a",
            "models": {
                "conflict-model": {
                    "id": "conflict-model",
                    "cost": { "input": 1.0, "output": 4.0 }
                },
                "agreed-model": {
                    "id": "agreed-model",
                    "cost": { "input": 1.5, "output": 6.0 }
                }
            }
        },
        "vendor-b": {
            "id": "vendor-b",
            "models": {
                "conflict-model": {
                    "id": "conflict-model",
                    "cost": { "input": 2.0, "output": 8.0 }
                },
                "agreed-model": {
                    "id": "agreed-model",
                    "cost": { "input": 1.5, "output": 6.0 }
                }
            }
        }
    }))
    .unwrap()
}

fn sample_litellm() -> Vec<u8> {
    serde_json::to_vec(&json!({
        "sample_spec": {
            "input_cost_per_token": 0.0,
            "output_cost_per_token": 0.0,
            "mode": "chat"
        },
        "deepseek/deepseek-chat": {
            "litellm_provider": "deepseek",
            "mode": "chat",
            "input_cost_per_token": 0.00000027,
            "output_cost_per_token": 0.0000011,
            "cache_read_input_token_cost": 0.00000007
        },
        "deepseek/deepseek-flash": {
            "litellm_provider": "deepseek",
            "mode": "chat",
            "input_cost_per_token": 0.0000002,
            "output_cost_per_token": 0.0000008
        },
        "gemini/gemini-2.5-pro": {
            "litellm_provider": "gemini",
            "mode": "chat",
            "input_cost_per_token": 0.00000125,
            "output_cost_per_token": 0.00001,
            "cache_read_input_token_cost": 0.00000031,
            "input_cost_per_token_above_200k_tokens": 0.0000025,
            "output_cost_per_token_above_200k_tokens": 0.000015
        },
        "text-embedding-3-large": {
            "litellm_provider": "openai",
            "mode": "embedding",
            "input_cost_per_token": 0.00000013,
            "output_cost_per_token": 0.0
        },
        "char-billed-model": {
            "litellm_provider": "vertex_ai",
            "mode": "chat",
            "input_cost_per_token": 0.000001,
            "output_cost_per_token": 0.000002,
            "input_cost_per_character": 0.0000005
        },
        "reasoning-diff-model": {
            "litellm_provider": "openai",
            "mode": "chat",
            "input_cost_per_token": 0.000001,
            "output_cost_per_token": 0.000002,
            "output_cost_per_reasoning_token": 0.000005
        },
        "missing-output-model": {
            "litellm_provider": "openai",
            "mode": "chat",
            "input_cost_per_token": 0.000001
        }
    }))
    .unwrap()
}

fn pricer_with_fixtures(dir: &tempfile::TempDir) -> Pricer {
    let pricer = Pricer::open(
        Some(dir.path().join("data")),
        Some(dir.path().join("cache")),
    );
    let md = sample_models_dev();
    let ll = sample_litellm();
    let status = pricer.refresh_with_fetcher(|url| {
        if url == MODELS_DEV_URL {
            Ok(md.clone())
        } else if url == LITELLM_URL {
            Ok(ll.clone())
        } else {
            Err("unexpected url".into())
        }
    });
    assert!(status.sources.iter().all(|s| s.error.is_none()));
    pricer
}

fn make_event(agent: &str, model: &str, tokens: Tokens) -> events::RequestEvent {
    events::RequestEvent {
        id: "evt-1".into(),
        agent: agent.into(),
        time: 1_790_000_000_000,
        session: "ses-1".into(),
        model: model.into(),
        status: RequestOutcome::Success,
        partial: false,
        tokens,
        details: RecordedFields {
            input_recorded: true,
            output_recorded: true,
            availability_known: true,
            cache_read_recorded: true,
            cache_write_recorded: true,
            reasoning_recorded: true,
            ..Default::default()
        },
        request: RequestMeta {
            scope: Some("request".into()),
            ..Default::default()
        },
        output_tps: Some(50.0),
        cost: None,
        pricing_issue: None,
    }
}

#[test]
fn normalizes_units_and_prevents_cross_provider_or_invalid_matches() {
    let dir = tempfile::tempdir().unwrap();
    let pricer = pricer_with_fixtures(&dir);

    // Models.dev 直接以每百万 Token 计价，原厂优先于 github-copilot ($0) 与 openrouter
    let claude = pricer.resolve("claude-sonnet-4-5", None, 0).unwrap();
    assert_eq!(claude.source, "models.dev");
    assert_eq!(claude.provider.as_deref(), Some("anthropic"));
    assert_eq!(claude.rates.input, 3.0);
    assert_eq!(claude.rates.output, 15.0);
    assert_eq!(claude.rates.cache_read, Some(0.3));
    assert_eq!(claude.rates.cache_write, Some(3.75));
    assert_eq!(
        claude.rates.long_context.as_ref().unwrap().threshold,
        200_000
    );

    // 显式 OpenRouter 前缀只命中 OpenRouter，不跨商回落到 Anthropic
    let or = pricer
        .resolve("openrouter/anthropic/claude-sonnet-4-5", None, 0)
        .unwrap();
    assert_eq!(or.provider.as_deref(), Some("openrouter"));
    assert_eq!(or.rates.input, 3.5);

    // LiteLLM USD/Token 规范换算到 USD/1M tokens，无浮点精度噪声
    let ds = pricer.resolve("deepseek-chat", None, 0).unwrap();
    assert_eq!(ds.source, "litellm");
    assert_eq!(ds.rates.input, 0.27);
    assert_eq!(ds.rates.output, 1.1);
    assert_eq!(ds.rates.cache_read, Some(0.07));

    for model in ["deepseek-v4.1-flash", "deepseek-v4.1-flash-1017", "gateway/DeepSeek-V4.1-Flash-1017"] {
        let price = pricer.resolve(model, None, 0).unwrap();
        assert_eq!(price.matched_as.as_deref(), Some("deepseek-flash"));
        assert_eq!(price.rates.input, 0.2);
    }

    // google/ 与 LiteLLM gemini/ 规范为同一原厂，可安全匹配
    let gem = pricer.resolve("google/gemini-2.5-pro", None, 0).unwrap();
    assert_eq!(gem.source, "litellm");
    assert_eq!(gem.rates.input, 1.25);
    assert_eq!(gem.rates.output, 10.0);
    assert_eq!(gem.rates.long_context.as_ref().unwrap().input, 2.5);

    let fallback = pricer
        .resolve("custom-gw/anthropic/claude-sonnet-4-5-xhigh", None, 0)
        .unwrap();
    assert_eq!(fallback.provider.as_deref(), Some("anthropic"));
    assert_eq!(fallback.rates.input, 3.0);
    assert_eq!(
        pricer
            .resolve("aliyun/deepseek-chat", None, 0)
            .unwrap()
            .rates
            .input,
        0.27
    );
    assert!(pricer.resolve("gpt-5-codex", Some("ollama"), 0).is_none());
    for model in ["ollama/gpt-5-codex", "lmstudio/anthropic/claude-sonnet-4-5", "openrouter/gpt-5-codex"] {
        assert!(pricer.resolve(model, None, 0).is_none(), "{model}");
    }

    // 冲突/挂名零/非文本单位/缺输出价全部拒绝
    assert!(pricer.resolve("conflict-model", None, 0).is_none());
    assert!(pricer.resolve("claude-nominal-zero", None, 0).is_none());
    assert!(pricer.resolve("claude-audio-special", None, 0).is_none());
    assert!(pricer.resolve("text-embedding-3-large", None, 0).is_none());
    assert!(pricer.resolve("char-billed-model", None, 0).is_none());
    assert!(pricer.resolve("reasoning-diff-model", None, 0).is_none());
    assert!(pricer.resolve("missing-output-model", None, 0).is_none());

    // 多提供商一致时可匹配
    let agreed = pricer.resolve("agreed-model", None, 0).unwrap();
    assert_eq!(agreed.rates.input, 1.5);

    // 已知变体后缀兜底
    let high = pricer.resolve("claude-sonnet-4-5-high", None, 0).unwrap();
    assert_eq!(high.matched_as.as_deref(), Some("claude-sonnet-4-5"));
    assert_eq!(high.rates.input, 3.0);

    // 仅允许固定公开 URL
    assert!(fetch_public_catalog("https://example.com/prices.json").is_err());
}

#[test]
fn custom_cny_usd_persistence_override_revert_and_corruption_safety() {
    let dir = tempfile::tempdir().unwrap();
    let pricer = pricer_with_fixtures(&dir);

    // 用 CNY 自定义价覆盖 Models.dev 参考价（1 USD = 7 CNY）
    pricer
        .set_custom_price(CustomPriceInput {
            model: "claude-sonnet-4-5".into(),
            currency: Currency::Cny,
            input: 14.0,
            output: 70.0,
            cache_read: Some(1.4),
            cache_write: None,
            long_context: Some(LongContextRates {
                threshold: 200_000,
                input: 28.0,
                output: 140.0,
                cache_read: Some(2.8),
                cache_write: None,
            }),
        })
        .unwrap();

    let resolved = pricer.resolve("claude-sonnet-4-5", None, 0).unwrap();
    assert_eq!(resolved.source, "custom");
    assert_eq!(resolved.currency, Currency::Cny);
    assert_eq!(resolved.original_rates.as_ref().unwrap().input, 14.0);
    assert_eq!(resolved.original_rates.as_ref().unwrap().output, 70.0);
    assert_eq!(
        resolved.original_rates.as_ref().unwrap().cache_read,
        Some(1.4)
    );
    assert_eq!(resolved.original_rates.as_ref().unwrap().cache_write, None);
    assert!((resolved.rates.input - 2.0).abs() < 1e-9);
    assert!((resolved.rates.output - 10.0).abs() < 1e-9);
    assert!((resolved.rates.cache_read.unwrap() - 0.2).abs() < 1e-9);
    assert_eq!(
        resolved.overrides_reference.as_ref().unwrap().source,
        "models.dev"
    );

    // 重载后保留币种与原始金额
    let reloaded = Pricer::open(
        Some(dir.path().join("data")),
        Some(dir.path().join("cache")),
    );
    let again = reloaded.resolve("claude-sonnet-4-5", None, 0).unwrap();
    assert_eq!(again.source, "custom");
    assert_eq!(again.currency, Currency::Cny);
    assert_eq!(again.original_rates.as_ref().unwrap().input, 14.0);

    assert_eq!(
        again
            .original_rates
            .as_ref()
            .unwrap()
            .long_context
            .as_ref()
            .unwrap()
            .input,
        28.0
    );
    assert_eq!(again.rates.long_context.as_ref().unwrap().input, 4.0);
    let event = |input_tokens| {
        make_event(
            "claude",
            "claude-sonnet-4-5",
            Tokens {
                input_tokens,
                output_tokens: 1,
                ..Default::default()
            },
        )
    };
    assert_eq!(
        reloaded
            .evaluate_event_cost(&event(200_000))
            .unwrap()
            .input_rate,
        2.0
    );
    assert_eq!(
        reloaded
            .evaluate_event_cost(&event(200_001))
            .unwrap()
            .input_rate,
        4.0
    );

    // 允许用户显式设置 0 费用（本地/免费模型）
    reloaded
        .set_custom_price(CustomPriceInput {
            model: "local/qwen3".into(),
            currency: Currency::Usd,
            input: 0.0,
            output: 0.0,
            cache_read: Some(0.0),
            cache_write: Some(0.0),
            long_context: None,
        })
        .unwrap();
    let free = reloaded.resolve("local/qwen3", None, 0).unwrap();
    assert_eq!(free.rates.input, 0.0);

    // 删除自定义覆盖后恢复为原公开参考价
    reloaded.delete_custom_price("claude-sonnet-4-5").unwrap();
    let reverted = reloaded.resolve("claude-sonnet-4-5", None, 0).unwrap();
    assert_eq!(reverted.source, "models.dev");
    assert_eq!(reverted.rates.input, 3.0);

    // 非法输入被拒绝
    assert!(reloaded
        .set_custom_price(CustomPriceInput {
            model: " ".into(),
            currency: Currency::Usd,
            input: 1.0,
            output: 1.0,
            cache_read: None,
            cache_write: None,
            long_context: None,
        })
        .is_err());
    assert!(reloaded
        .set_custom_price(CustomPriceInput {
            model: "m".into(),
            currency: Currency::Usd,
            input: -1.0,
            output: 1.0,
            cache_read: None,
            cache_write: None,
            long_context: None,
        })
        .is_err());

    // 自定义文件损坏时保留原文件，不静默清空或盲目覆盖
    let custom_file = dir.path().join("data/custom-prices.json");
    fs::write(&custom_file, b"{corrupt-json").unwrap();
    let broken = Pricer::open(
        Some(dir.path().join("data")),
        Some(dir.path().join("cache")),
    );
    assert!(broken.status().custom_error.is_some());
    assert!(broken
        .set_custom_price(CustomPriceInput {
            model: "m".into(),
            currency: Currency::Usd,
            input: 1.0,
            output: 2.0,
            cache_read: None,
            cache_write: None,
            long_context: None,
        })
        .is_err());
    assert_eq!(fs::read(&custom_file).unwrap(), b"{corrupt-json");
}

#[test]
fn free_model_ids_use_builtin_zero_rates_until_customized() {
    let pricer = Pricer::open(None, None);
    for model in ["local/demo-free", "openrouter/demo:free"] {
        let price = pricer.resolve(model, Some("gateway"), 0).unwrap();
        assert_eq!(price.source, BUILTIN_SOURCE);
        assert_eq!(price.rates.input, 0.0);
        assert_eq!(price.rates.output, 0.0);
        assert_eq!(price.rates.cache_read, Some(0.0));
        assert_eq!(price.rates.cache_write, Some(0.0));
        let mut event = make_event("opencode", model, Tokens::default());
        event.details = RecordedFields::default();
        event.request.tier = Some("flex".into());
        let cost = pricer.snapshot().price_event(&event).unwrap();
        assert_eq!(cost.source, BUILTIN_SOURCE);
        assert_eq!(cost.total_usd, 0.0);
        pricer.set_custom_price(CustomPriceInput {
            model: model.into(),
            currency: Currency::Usd,
            input: 1.0,
            output: 1.0,
            cache_read: None,
            cache_write: None,
            long_context: None,
        }).unwrap();
        assert_eq!(pricer.resolve(model, None, 0).unwrap().source, "custom");
        let paid = make_event("opencode", model, Tokens {
            input_tokens: 1_000_000,
            ..Default::default()
        });
        let cost = pricer.snapshot().price_event(&paid).unwrap();
        assert_eq!(cost.source, "custom");
        assert_eq!(cost.total_usd, 1.0);
        pricer.delete_custom_price(model).unwrap();
        assert_eq!(pricer.resolve(model, None, 0).unwrap().source, BUILTIN_SOURCE);
    }
    assert!(pricer.resolve("local/demo-free-preview", None, 0).is_none());
}

#[test]
fn cost_accounting_matches_agent_token_rules() {
    let dir = tempfile::tempdir().unwrap();
    let pricer = pricer_with_fixtures(&dir);

    // Claude：input_tokens=1_000_000 已包含 cache_read=600_000 与 cache_write=100_000，
    // 未缓存输入应为 300_000；output_tokens=200_000 已包含 reasoning=50_000，不重复计费。
    let claude = make_event(
        "claude",
        "claude-sonnet-4-5",
        Tokens {
            input_tokens: 1_000_000,
            output_tokens: 200_000,
            cache_read_tokens: 600_000,
            cache_write_tokens: 100_000,
            reasoning_tokens: 50_000,
            unclassified_tokens: 0,
            ..Default::default()
        },
    );
    // 1_000_000 > 200_000 触发长上下文档：input=6, output=22.5, read=0.6, write=7.5
    let cost = pricer.evaluate_event_cost(&claude).unwrap();
    assert_eq!(cost.source, "models.dev");
    assert!(cost.long_context);
    let expected = 0.3 * 6.0 + 0.6 * 0.6 + 0.1 * 7.5 + 0.2 * 22.5;
    assert!((cost.total_usd - expected).abs() < 1e-9);

    // Codex priority 档位按 ×2 计费；未设置 cache_write 时回落到 input 价
    let mut codex = make_event(
        "codex",
        "gpt-5-codex",
        Tokens {
            input_tokens: 100_000,
            output_tokens: 10_000,
            cache_read_tokens: 80_000,
            cache_write_tokens: 0,
            reasoning_tokens: 4_000,
            unclassified_tokens: 0,
            ..Default::default()
        },
    );
    codex.request.tier = Some("priority".into());
    let codex_cost = pricer.evaluate_event_cost(&codex).unwrap();
    assert_eq!(codex_cost.multiplier, Some(2.0));
    let expected_codex = (0.02 * 4.0) + (0.08 * 0.4) + (0.01 * 16.0);
    assert!((codex_cost.total_usd - expected_codex).abs() < 1e-9);

    // Qoder 缓存包含关系未确认：有缓存时不虚构估算；无缓存时可正常估算
    let qoder_cached = make_event(
        "qoder",
        "claude-sonnet-4-5",
        Tokens {
            input_tokens: 1000,
            output_tokens: 100,
            cache_read_tokens: 500,
            cache_write_tokens: 0,
            reasoning_tokens: 0,
            unclassified_tokens: 0,
            ..Default::default()
        },
    );
    assert!(pricer.evaluate_event_cost(&qoder_cached).is_none());
    let qoder_plain = make_event(
        "qoder",
        "claude-sonnet-4-5",
        Tokens {
            input_tokens: 100_000,
            output_tokens: 10_000,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            reasoning_tokens: 0,
            unclassified_tokens: 0,
            ..Default::default()
        },
    );
    let qoder_cost = pricer.evaluate_event_cost(&qoder_plain).unwrap();
    assert!((qoder_cost.total_usd - (0.1 * 3.0 + 0.01 * 15.0)).abs() < 1e-9);

    // 无计量的失败请求为未知（None），绝不写成 0
    let mut failed = make_event("claude", "claude-sonnet-4-5", Tokens::default());
    failed.status = RequestOutcome::Failed;
    failed.details.input_recorded = false;
    failed.details.output_recorded = false;
    assert!(pricer.evaluate_event_cost(&failed).is_none());
}

#[test]
fn summary_and_events_share_exact_cost_and_reprice_history_immediately() {
    let dir = tempfile::tempdir().unwrap();
    let logs = dir.path().join("logs");
    let t = 1_790_000_000_000i64;
    fs::create_dir_all(&logs).unwrap();
    fs::write(
        logs.join("a.jsonl"),
        format!(
            "{}\n{}\n",
            json!({"type":"assistant","sessionId":"s1","timestamp":t,"cwd":"/proj/a","message":{"id":"msg_1","model":"claude-sonnet-4-5","stop_reason":"end_turn","usage":{"input_tokens":200000,"output_tokens":10000,"cache_read_input_tokens":800000,"cache_creation_input_tokens":0}}}),
            json!({"type":"assistant","sessionId":"s1","timestamp":t+1000,"cwd":"/proj/a","message":{"id":"msg_2","model":"unpriced-model","stop_reason":"end_turn","usage":{"input_tokens":1000,"output_tokens":100,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}),
        ),
    )
    .unwrap();
    let mut store = Store::open(None);
    ingest::sync_at("claude", &[logs], &mut store, &|| false).unwrap();

    let pricer = pricer_with_fixtures(&dir);
    let db = &mut store.ledger().unwrap().db;
    let old_cost = serde_json::to_string(&EventCost {
        total_usd: 4321.0,
        ..Default::default()
    })
    .unwrap();
    assert_eq!(
        db.execute(
            "UPDATE usage_request_events
             SET data=json_set(data, '$.request.costUsd', 1234, '$.cost', json(?1))",
            [old_cost],
        )
        .unwrap(),
        2
    );
    let sum1 = summary::read_at_with_pricer(db, Default::default(), &pricer, &|| false).unwrap();
    let page1 = events::page_with_pricer(db, Default::default(), &pricer, &|| false).unwrap();

    let unpriced = page1
        .events
        .iter()
        .find(|event| event.model == "unpriced-model")
        .unwrap();
    assert!(unpriced.cost.is_none());
    let summary_cost: f64 = sum1.rows.iter().filter_map(|r| r.metrics.cost_usd).sum();
    let events_cost: f64 = page1
        .events
        .iter()
        .filter_map(|e| e.cost.as_ref().map(|c| c.total_usd))
        .sum();
    assert!((summary_cost - events_cost).abs() < 1e-12);
    assert!((summary_cost - 1.905).abs() < 1e-12);
    assert_eq!(
        sum1.rows
            .iter()
            .map(|r| r.metrics.unpriced_count)
            .sum::<u64>(),
        1
    );
    assert_eq!(sum1.projects.len(), 2);

    // 添加自定义价后，无需重新扫描日志即可立即作用于已有历史
    pricer
        .set_custom_price(CustomPriceInput {
            model: "unpriced-model".into(),
            currency: Currency::Cny,
            input: 7.0,
            output: 14.0,
            cache_read: None,
            cache_write: None,
            long_context: None,
        })
        .unwrap();
    let sum2 = summary::read_at_with_pricer(db, Default::default(), &pricer, &|| false).unwrap();
    let page2 = events::page_with_pricer(db, Default::default(), &pricer, &|| false).unwrap();
    assert_eq!(
        sum2.rows
            .iter()
            .map(|r| r.metrics.unpriced_count)
            .sum::<u64>(),
        0
    );
    let summary_cost2: f64 = sum2.rows.iter().filter_map(|r| r.metrics.cost_usd).sum();
    let events_cost2: f64 = page2
        .events
        .iter()
        .filter_map(|e| e.cost.as_ref().map(|c| c.total_usd))
        .sum();
    assert!((summary_cost2 - events_cost2).abs() < 1e-12);
    assert!((summary_cost2 - summary_cost - 0.0012).abs() < 1e-12);
}

#[test]
fn partial_refresh_failure_offline_stale_cache_and_concurrent_reads_keep_last_valid_prices() {
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("cache");
    let data = dir.path().join("data");
    let pricer = Arc::new(Pricer::open(Some(data.clone()), Some(cache.clone())));

    let md = sample_models_dev();
    let ll = sample_litellm();
    pricer.refresh_with_fetcher(|url| {
        if url == MODELS_DEV_URL {
            Ok(md.clone())
        } else {
            Ok(ll.clone())
        }
    });
    assert_eq!(
        pricer
            .resolve("deepseek-chat", None, 0)
            .unwrap()
            .rates
            .input,
        0.27
    );

    // 部分失败：Models.dev 更新为新价，LiteLLM 网络失败 -> LiteLLM 必须保留上一次有效参考价！
    let updated_md = serde_json::to_vec(&json!({
        "anthropic": {
            "id": "anthropic",
            "models": {
                "claude-sonnet-4-5": {
                    "id": "claude-sonnet-4-5",
                    "cost": { "input": 4.0, "output": 20.0 }
                }
            }
        }
    }))
    .unwrap();
    let partial = pricer.refresh_with_fetcher(|url| {
        if url == MODELS_DEV_URL {
            Ok(updated_md.clone())
        } else {
            Err("network timeout".into())
        }
    });
    assert_eq!(partial.sources[0].error, None);
    assert!(partial.sources[1]
        .error
        .as_deref()
        .is_some_and(|e| e.contains("timeout")));
    assert_eq!(
        pricer
            .resolve("claude-sonnet-4-5", None, 0)
            .unwrap()
            .rates
            .input,
        4.0
    );
    assert_eq!(
        pricer
            .resolve("deepseek-chat", None, 0)
            .unwrap()
            .rates
            .input,
        0.27
    );

    // 模拟过期缓存（超过 24 小时）在离线重启后仍可继续使用
    fs::write(
        cache.join("models-dev.meta.json"),
        serde_json::to_vec(&json!({"fetchedAt": 1_000})).unwrap(),
    )
    .unwrap();
    let offline = Pricer::open(Some(data), Some(cache));
    assert!(offline.is_stale());
    assert_eq!(
        offline
            .resolve("claude-sonnet-4-5", None, 0)
            .unwrap()
            .rates
            .input,
        4.0
    );
    assert_eq!(
        offline
            .resolve("deepseek-chat", None, 0)
            .unwrap()
            .rates
            .input,
        0.27
    );

    // 并发刷新与查询：刷新期间读线程始终拿到合法价格，不阻塞、不崩溃
    let done = Arc::new(AtomicBool::new(false));
    let barrier = Arc::new(Barrier::new(3));
    let mut handles = Vec::new();
    for _ in 0..2 {
        let p = Arc::clone(&pricer);
        let d = Arc::clone(&done);
        let b = Arc::clone(&barrier);
        handles.push(thread::spawn(move || {
            b.wait();
            while !d.load(Ordering::Relaxed) {
                let price = p.resolve("claude-sonnet-4-5", None, 0).unwrap();
                assert!(price.rates.input == 4.0 || price.rates.input == 3.0);
            }
        }));
    }
    barrier.wait();
    for _ in 0..5 {
        let _ = pricer.refresh_with_fetcher(|url| {
            if url == MODELS_DEV_URL {
                Ok(md.clone())
            } else {
                Ok(ll.clone())
            }
        });
    }
    done.store(true, Ordering::Relaxed);
    for handle in handles {
        handle.join().unwrap();
    }
}

#[test]
fn query_uses_single_immutable_pricing_snapshot_across_mid_scan_updates() {
    let dir = tempfile::tempdir().unwrap();
    let logs = dir.path().join("logs");
    let t = 1_790_000_000_000i64;
    fs::create_dir_all(&logs).unwrap();
    fs::write(
        logs.join("a.jsonl"),
        format!(
            "{}\n{}\n",
            json!({"type":"assistant","sessionId":"s1","timestamp":t,"message":{"id":"msg_1","model":"snap-model","stop_reason":"end_turn","usage":{"input_tokens":1000000,"output_tokens":0,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}),
            json!({"type":"assistant","sessionId":"s1","timestamp":t+1000,"message":{"id":"msg_2","model":"snap-model","stop_reason":"end_turn","usage":{"input_tokens":1000000,"output_tokens":0,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}),
        ),
    )
    .unwrap();
    let mut store = Store::open(None);
    ingest::sync_at("claude", &[logs], &mut store, &|| false).unwrap();

    let pricer = Pricer::open(None, None);
    let initial = pricer
        .set_custom_price(CustomPriceInput {
            model: "snap-model".into(),
            currency: Currency::Usd,
            input: 2.0,
            output: 0.0,
            cache_read: None,
            cache_write: None,
            long_context: None,
        })
        .unwrap();

    // 在第二条记录扫描前更新自定义价为 $7/M；本次查询仍须完整使用 $2/M 快照（总计 $4）及旧 revision。
    let db = &mut store.ledger().unwrap().db;
    let calls = std::cell::Cell::new(0usize);
    let mut_triggered = std::cell::Cell::new(false);
    let summary_res = summary::read_at_with_pricer(db, Default::default(), &pricer, &|| {
        let n = calls.get() + 1;
        calls.set(n);
        if n == 3 && !mut_triggered.get() {
            mut_triggered.set(true);
            pricer
                .set_custom_price(CustomPriceInput {
                    model: "snap-model".into(),
                    currency: Currency::Usd,
                    input: 7.0,
                    output: 0.0,
                    cache_read: None,
                    cache_write: None,
                    long_context: None,
                })
                .unwrap();
        }
        false
    })
    .unwrap();
    assert!(mut_triggered.get());
    assert_eq!(summary_res.pricing_revision, initial.revision);
    let total_cost: f64 = summary_res
        .rows
        .iter()
        .filter_map(|r| r.metrics.cost_usd)
        .sum();
    assert!((total_cost - 4.0).abs() < 1e-9);

    // 同一分页中途变更价格为 $11/M，分页内两条记录仍保持 $7/M 与一致 revision。
    let before_page = pricer.status().revision;
    let page_calls = std::cell::Cell::new(0usize);
    let page_mut = std::cell::Cell::new(false);
    let page_res = events::page_with_pricer(db, Default::default(), &pricer, &|| {
        let n = page_calls.get() + 1;
        page_calls.set(n);
        if n == 3 && !page_mut.get() {
            page_mut.set(true);
            pricer
                .set_custom_price(CustomPriceInput {
                    model: "snap-model".into(),
                    currency: Currency::Usd,
                    input: 11.0,
                    output: 0.0,
                    cache_read: None,
                    cache_write: None,
                    long_context: None,
                })
                .unwrap();
        }
        false
    })
    .unwrap();
    assert!(page_mut.get());
    assert_eq!(page_res.pricing_revision, before_page);
    let page_cost: f64 = page_res
        .events
        .iter()
        .filter_map(|e| e.cost.as_ref().map(|c| c.total_usd))
        .sum();
    assert!((page_cost - 14.0).abs() < 1e-9);
}

#[test]
fn litellm_supports_272k_threshold_and_rejects_unparseable_special_tiers() {
    let raw = serde_json::to_vec(&json!({
        "azure_ai/gpt-6.1-sol": {
            "litellm_provider": "azure_ai",
            "mode": "chat",
            "input_cost_per_token": 0.000002,
            "output_cost_per_token": 0.00001,
            "input_cost_per_token_above_272k_tokens": 0.000004,
            "output_cost_per_token_above_272k_tokens": 0.00002
        },
        "openai/weird-tier-model": {
            "litellm_provider": "openai",
            "mode": "chat",
            "input_cost_per_token": 0.000002,
            "output_cost_per_token": 0.00001,
            "input_cost_per_token_above_custom_window": 0.000004
        },
        "openai/multi-tier-model": {
            "litellm_provider": "openai",
            "mode": "chat",
            "input_cost_per_token": 0.000002,
            "output_cost_per_token": 0.00001,
            "input_cost_per_token_above_128k_tokens": 0.000003,
            "output_cost_per_token_above_128k_tokens": 0.000015,
            "input_cost_per_token_above_272k_tokens": 0.000004,
            "output_cost_per_token_above_272k_tokens": 0.00002
        },
        "openai/tiered-array-model": {
            "litellm_provider": "openai",
            "mode": "chat",
            "input_cost_per_token": 0.000002,
            "output_cost_per_token": 0.00001,
            "tiered_pricing": [{"range": [0, 200000], "input_cost_per_token": 0.000002}]
        }
    }))
    .unwrap();
    let pricer = Pricer::open(None, None);
    pricer.refresh_with_fetcher(|url| {
        if url == LITELLM_URL {
            Ok(raw.clone())
        } else {
            Err("skip".into())
        }
    });

    // 恰好等于 272,000：命中标准档位 $2/$10 -> 0.544 + 0.001 = $0.545
    let at_threshold = make_event(
        "codex",
        "azure_ai/gpt-6.1-sol",
        Tokens {
            input_tokens: 272_000,
            output_tokens: 100,
            ..Default::default()
        },
    );
    let c1 = pricer.evaluate_event_cost(&at_threshold).unwrap();
    assert!(!c1.long_context);
    assert!((c1.total_usd - 0.545).abs() < 1e-9);

    // 刚超过 272,000（272,001）：命中长上下文档 $4/$20 -> 1.088004 + 0.002 = $1.090004
    let above_threshold = make_event(
        "codex",
        "azure_ai/gpt-6.1-sol",
        Tokens {
            input_tokens: 272_001,
            output_tokens: 100,
            ..Default::default()
        },
    );
    let c2 = pricer.evaluate_event_cost(&above_threshold).unwrap();
    assert!(c2.long_context);
    assert!((c2.total_usd - 1.090004).abs() < 1e-9);

    // 无法准确解析的特殊/多档费率必须按未定价处理，不可降级为普通费率
    assert!(pricer.resolve("openai/weird-tier-model", None, 0).is_none());
    assert!(pricer.resolve("openai/multi-tier-model", None, 0).is_none());
    assert!(pricer
        .resolve("openai/tiered-array-model", None, 0)
        .is_none());
}

fn gpt_56_sol_event(tier: Option<&str>, tokens: Tokens) -> events::RequestEvent {
    let mut event = make_event("codex", "gpt-5.6-sol", tokens);
    event.request.provider = Some("openai".into());
    event.request.tier = tier.map(str::to_owned);
    event
}

fn sol_tokens(input: u64, read: u64, write: u64, output: u64) -> Tokens {
    Tokens {
        input_tokens: input,
        cache_read_tokens: read,
        cache_write_tokens: write,
        output_tokens: output,
        ..Default::default()
    }
}

/// Models.dev：OpenAI gpt-5.6-sol 标准 4/20/0.4/5，>272000 上下文档 8/30/0.8/10。
/// 同一提供商下另一个模型带有非上下文档位（缺 size），旧解析会让整个 openai 提供商被跳过。
#[test]
fn models_dev_gpt_56_sol_context_tier_survives_unusual_sibling_tiers() {
    let raw = serde_json::to_vec(&json!({
        "openai": {
            "id": "openai",
            "models": {
                "gpt-5.6-sol": {
                    "id": "gpt-5.6-sol",
                    "cost": {
                        "input": 4, "output": 20, "cache_read": 0.4, "cache_write": 5,
                        "tiers": [
                            {"input": 8, "output": 30, "cache_read": 0.8, "cache_write": 10,
                             "tier": {"type": "context", "size": 272000}},
                            {"input": 8, "output": 40, "tier": {"type": "service", "name": "priority"}}
                        ]
                    }
                },
                "gpt-odd-tier": {
                    "id": "gpt-odd-tier",
                    "cost": {"input": 1, "output": 2, "tiers": [{"input": 2, "tier": {"type": "batch"}}]}
                }
            }
        }
    }))
    .unwrap();
    let litellm = json!({
        "openai/gpt-5.6-sol": {
            "litellm_provider": "openai", "mode": "chat",
            "input_cost_per_token": 4e-6, "output_cost_per_token": 2e-5,
            "input_cost_per_token_priority": 7.2e-6, "output_cost_per_token_priority": 3.6e-5
        }
    }).to_string();
    let pricer = Pricer::open(None, None);
    pricer.refresh_with_fetcher(|url| {
        if url == MODELS_DEV_URL {
            Ok(raw.clone())
        } else {
            Ok(litellm.as_bytes().to_vec())
        }
    });

    let price = pricer
        .resolve("gpt-5.6-sol", Some("private-gateway"), 0)
        .expect("gateway can use the first-party reference price");
    assert_eq!(price.source, "models.dev");
    assert_eq!(price.provider.as_deref(), Some("openai"));
    let long = price.rates.long_context.as_ref().unwrap();
    assert_eq!(long.threshold, 272_000);
    assert!(approx_f64(long.input, 8.0) && approx_f64(long.output, 30.0));

    let mut event = gpt_56_sol_event(None, sol_tokens(100_000, 60_000, 10_000, 5_000));
    event.request.provider = Some("private-gateway".into());
    let standard = pricer.evaluate_event_cost(&event).unwrap();
    assert!(
        (standard.total_usd - (0.03 * 4.0 + 0.06 * 0.4 + 0.01 * 5.0 + 0.005 * 20.0)).abs() < 1e-9
    );
    let long_cost = pricer
        .evaluate_event_cost(&gpt_56_sol_event(
            None,
            sol_tokens(300_000, 200_000, 0, 1_000),
        ))
        .unwrap();
    assert!(long_cost.long_context);
    assert!((long_cost.total_usd - (0.1 * 8.0 + 0.2 * 0.8 + 0.001 * 30.0)).abs() < 1e-9);
    let fast = gpt_56_sol_event(Some("fast"), sol_tokens(100_000, 0, 0, 0));
    let priority = pricer.evaluate_event_cost(&fast).unwrap();
    assert_eq!(priority.source, "litellm");
    assert_eq!(priority.service_tier.as_deref(), Some("priority"));
    assert!(approx_f64(priority.total_usd, 0.72));
    // 其他模型的非上下文档位只被忽略，不影响其标准价。
    assert!(pricer.resolve("gpt-odd-tier", Some("openai"), 0).is_some());
}

/// LiteLLM：12 小时缓存写入、priority、flex、batch 都是可选变体，不能让标准价消失，
/// 也不能被当作标准 Token 的价格。未定价的档位或分项明确报告为未知，而不是 0。
#[test]
fn litellm_variants_keep_standard_price_and_apply_only_to_matching_tokens() {
    let raw = serde_json::to_vec(&json!({
        "gpt-5.6-sol": {
            "litellm_provider": "openai",
            "mode": "responses",
            "input_cost_per_token": 4e-6,
            "output_cost_per_token": 2e-5,
            "cache_read_input_token_cost": 4e-7,
            "cache_creation_input_token_cost": 5e-6,
            "cache_creation_input_token_cost_above_12hr": 8e-6,
            "input_cost_per_token_above_272k_tokens": 8e-6,
            "output_cost_per_token_above_272k_tokens": 3e-5,
            "cache_read_input_token_cost_above_272k_tokens": 8e-7,
            "cache_creation_input_token_cost_above_272k_tokens": 1e-5,
            "input_cost_per_token_priority": 8e-6,
            "output_cost_per_token_priority": 4e-5,
            "cache_read_input_token_cost_priority": 8e-7,
            "input_cost_per_token_flex": 2e-6,
            "output_cost_per_token_flex": 1e-5,
            "cache_read_input_token_cost_flex": 2e-7,
            "input_cost_per_token_batches": 2e-6
        },
        "claude-sonnet-9": {
            "litellm_provider": "anthropic",
            "mode": "chat",
            "input_cost_per_token": 3e-6,
            "output_cost_per_token": 1.5e-5,
            "cache_read_input_token_cost": 3e-7,
            "cache_creation_input_token_cost": 3.75e-6,
            "cache_creation_input_token_cost_above_1hr": 6e-6
        }
    }))
    .unwrap();
    let pricer = Pricer::open(None, None);
    pricer.refresh_with_fetcher(|url| {
        if url == LITELLM_URL {
            Ok(raw.clone())
        } else {
            Err("skip".into())
        }
    });
    let snapshot = pricer.snapshot();
    let price = snapshot
        .resolve("gpt-5.6-sol", Some("openai"), 0)
        .expect("standard price kept");
    assert_eq!(price.source, "litellm");
    assert!(
        approx_opt_f64(price.rates.cache_write, Some(5.0)),
        "12h write rate must not replace the standard rate"
    );
    assert!(price.rates.cache_write_1h.is_none());
    assert_eq!(
        price.rates.long_context.as_ref().unwrap().threshold,
        272_000
    );
    // batch 只有输入价，无法计价，只丢弃该档位。
    assert!(
        price.rates.service_tiers.contains_key("priority")
            && price.rates.service_tiers.contains_key("flex")
    );
    assert!(!price.rates.service_tiers.contains_key("batch"));

    let tokens = sol_tokens(100_000, 60_000, 10_000, 5_000);
    let standard = snapshot
        .price_event(&gpt_56_sol_event(Some("default"), tokens.clone()))
        .unwrap();
    assert!((standard.total_usd - 0.294).abs() < 1e-9);
    assert_eq!(standard.service_tier, None);

    let priority = snapshot
        .price_event(&gpt_56_sol_event(Some("priority"), tokens.clone()))
        .unwrap();
    assert_eq!(priority.service_tier.as_deref(), Some("priority"));
    assert_eq!(
        priority.multiplier, None,
        "explicit priority rates replace the ×2 fallback"
    );
    // priority 未单列缓存写入价，按该档输入价计。
    assert!(
        (priority.total_usd - (0.03 * 8.0 + 0.06 * 0.8 + 0.01 * 8.0 + 0.005 * 40.0)).abs() < 1e-9
    );

    let flex = snapshot
        .price_event(&gpt_56_sol_event(Some("flex"), tokens.clone()))
        .unwrap();
    assert!((flex.total_usd - (0.03 * 2.0 + 0.06 * 0.2 + 0.01 * 2.0 + 0.005 * 10.0)).abs() < 1e-9);

    // batch 无完整价格、priority 超过长上下文阈值却无对应档位价、未知档位：均为未知而非 0。
    let batch = snapshot.price_event(&gpt_56_sol_event(Some("batch"), tokens.clone()));
    assert_eq!(batch.unwrap_err(), PricingIssue::UnsupportedTier);
    let long_priority = snapshot.price_event(&gpt_56_sol_event(
        Some("priority"),
        sol_tokens(300_000, 0, 0, 10),
    ));
    assert_eq!(long_priority.unwrap_err(), PricingIssue::UnsupportedTier);
    let mut fast = make_event("claude", "claude-sonnet-9", tokens.clone());
    fast.request.tier = Some("fast".into());
    assert_eq!(
        snapshot.price_event(&fast).unwrap_err(),
        PricingIssue::UnsupportedTier
    );

    // Claude 1 小时缓存写入使用单列的 1h 价，其余写入仍按 5 分钟价。
    let mut claude = make_event(
        "claude",
        "claude-sonnet-9",
        sol_tokens(100_000, 50_000, 20_000, 1_000),
    );
    claude.tokens.cache_write_1h_tokens = 8_000;
    let cost = snapshot.price_event(&claude).unwrap();
    assert!(approx_opt_f64(cost.cache_write_1h_rate, Some(6.0)));
    assert!(
        (cost.total_usd - (0.03 * 3.0 + 0.05 * 0.3 + 0.012 * 3.75 + 0.008 * 6.0 + 0.001 * 15.0))
            .abs()
            < 1e-9
    );
    let mut gateway = gpt_56_sol_event(None, sol_tokens(100_000, 0, 10_000, 10));
    gateway.agent = "claude".into();
    gateway.tokens.cache_write_1h_tokens = 1_000;
    let gateway_cost = snapshot.price_event(&gateway).unwrap();
    assert_eq!(gateway_cost.cache_write_1h_rate, Some(gateway_cost.input_rate * 2.0));
    assert_eq!(
        gateway_cost.cache_write_1h_usd,
        gateway_cost.cache_write_1h_rate.map(|rate| rate / 1_000.0)
    );
    // 非 Anthropic 模型出现 1 小时写入且目录无价：该分项无法计价。
    let mut odd = gpt_56_sol_event(None, sol_tokens(100_000, 0, 10_000, 10));
    odd.tokens.cache_write_1h_tokens = 1_000;
    assert_eq!(
        snapshot.price_event(&odd).unwrap_err(),
        PricingIssue::UnsupportedComponent
    );
    let mut unknown = make_event("codex", "no-such-model", tokens);
    unknown.request.provider = Some("openai".into());
    assert_eq!(
        snapshot.price_event(&unknown).unwrap_err(),
        PricingIssue::Unpriced
    );
}

/// 同名条目（裸 ID 与带前缀 ID）标准价一致、仅特殊档位不同：标准价仍可用，
/// 只保留各条目一致的特殊档位。
#[test]
fn duplicate_listings_differing_only_in_variants_keep_standard_price() {
    let raw = serde_json::to_vec(&json!({
        "gpt-5.6-sol": {
            "litellm_provider": "openai", "mode": "chat",
            "input_cost_per_token": 4e-6, "output_cost_per_token": 2e-5,
            "input_cost_per_token_priority": 8e-6, "output_cost_per_token_priority": 4e-5
        },
        "openai/gpt-5.6-sol": {
            "litellm_provider": "openai", "mode": "chat",
            "input_cost_per_token": 4e-6, "output_cost_per_token": 2e-5
        }
    }))
    .unwrap();
    let pricer = Pricer::open(None, None);
    pricer.refresh_with_fetcher(|url| {
        if url == LITELLM_URL {
            Ok(raw.clone())
        } else {
            Err("skip".into())
        }
    });
    // 无提供商提示时两个原厂条目都参与一致性检查。
    let price = pricer
        .resolve("gpt-5.6-sol", None, 0)
        .expect("standard price kept");
    assert!(approx_f64(price.rates.input, 4.0));
    assert!(price.rates.service_tiers.is_empty());
    // 没有一致的 priority 价时，Codex 回落到已公开的 ×2 规则。
    let mut event = gpt_56_sol_event(Some("priority"), sol_tokens(1_000_000, 0, 0, 0));
    event.request.provider = None;
    let cost = pricer.evaluate_event_cost(&event).unwrap();
    assert_eq!(cost.multiplier, Some(2.0));
    assert!((cost.total_usd - 8.0).abs() < 1e-9);
}

#[test]
fn request_event_rejects_corrupt_missing_id_and_curl_disables_curlrc() {
    let dir = tempfile::tempdir().unwrap();
    let logs = dir.path().join("logs");
    let t = 1_790_000_000_000i64;
    fs::create_dir_all(&logs).unwrap();
    fs::write(
        logs.join("a.jsonl"),
        format!(
            "{}\n",
            json!({"type":"assistant","sessionId":"s1","timestamp":t,"message":{"id":"msg_1","model":"claude-sonnet-4-5","stop_reason":"end_turn","usage":{"input_tokens":100,"output_tokens":10,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}),
        ),
    )
    .unwrap();
    let mut store = Store::open(None);
    ingest::sync_at("claude", &[logs], &mut store, &|| false).unwrap();

    // 删除必填 id 字段后，请求分页必须报错而不是解析成空 ID 正常记录。
    let db = &mut store.ledger().unwrap().db;
    db.execute(
        "UPDATE usage_request_events SET data=json_remove(data,'$.id')",
        [],
    )
    .unwrap();
    let err =
        events::page_with_pricer(db, Default::default(), &Pricer::open(None, None), &|| false)
            .err()
            .unwrap();
    assert!(err.contains("损坏"));

    // curl 参数首项必须是 --disable 且仅限固定公开端点。
    for url in [MODELS_DEV_URL, LITELLM_URL] {
        let args = build_curl_args(url).unwrap();
        assert_eq!(args.first().copied(), Some("--disable"));
        assert!(args.windows(2).any(|w| w == ["--proto", "=https"]));
        assert!(args.windows(2).any(|w| w == ["--max-redirs", "0"]));
        assert_eq!(args.last().copied(), Some(url));
    }
    assert!(build_curl_args("https://evil.invalid/api.json").is_err());
}

#[test]
fn model_prices_list_respects_agent_scope_and_separates_differing_providers() {
    let dir = tempfile::tempdir().unwrap();
    let pricer = pricer_with_fixtures(&dir);
    let mut store = Store::open(None);
    let t = 1_790_000_000_000i64;

    // 同一模型名 gpt-5-codex 在 Codex 下走 openai（2 次），在 Claude/Pi 下走 ollama（1 次）。
    fs::create_dir_all(dir.path().join("codex")).unwrap();
    fs::write(
        dir.path().join("codex/a.jsonl"),
        format!(
            "{}\n{}\n{}\n{}\n",
            json!({"timestamp":t,"type":"session_meta","payload":{"id":"c1","model_provider":"openai"}}),
            json!({"timestamp":t,"type":"turn_context","payload":{"model":"gpt-5-codex"}}),
            json!({"timestamp":t+1000,"type":"token_usage_record","payload":{"response_id":"r1","usage":{"input_tokens":100,"output_tokens":10}}}),
            json!({"timestamp":t+2000,"type":"token_usage_record","payload":{"response_id":"r2","usage":{"input_tokens":200,"output_tokens":20}}}),
        ),
    )
    .unwrap();
    fs::create_dir_all(dir.path().join("claude")).unwrap();
    fs::write(
        dir.path().join("claude/a.jsonl"),
        format!(
            "{}\n",
            json!({"type":"assistant","sessionId":"s1","timestamp":t,"message":{"id":"msg_o1","provider":"ollama","model":"gpt-5-codex","stop_reason":"end_turn","usage":{"input_tokens":500,"output_tokens":50,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}),
        ),
    )
    .unwrap();
    ingest::sync_at("codex", &[dir.path().join("codex")], &mut store, &|| false).unwrap();
    ingest::sync_at("claude", &[dir.path().join("claude")], &mut store, &|| {
        false
    })
    .unwrap();

    let db = &mut store.ledger().unwrap().db;
    let all = pricer.list_model_prices_from_db(db, None).unwrap();
    assert_eq!(
        all.models.len(),
        2,
        "differing providers must not share one price row"
    );
    let openai_row = all
        .models
        .iter()
        .find(|r| r.provider.as_deref() == Some("openai"))
        .unwrap();
    let ollama_row = all
        .models
        .iter()
        .find(|r| r.provider.as_deref() == Some("ollama"))
        .unwrap();
    assert_eq!(openai_row.requests, 2);
    assert_eq!(openai_row.total_tokens, 330);
    assert!(openai_row.price.is_some());
    assert_eq!(ollama_row.requests, 1);
    assert_eq!(ollama_row.total_tokens, 550);
    assert!(ollama_row.price.is_none());

    // 按 Agent 筛选时次数与 Token 严格匹配所选 Agent。
    let codex_only = pricer.list_model_prices_from_db(db, Some("codex")).unwrap();
    assert_eq!(codex_only.models.len(), 1);
    assert_eq!(codex_only.models[0].requests, 2);
    assert_eq!(codex_only.models[0].total_tokens, 330);
}
