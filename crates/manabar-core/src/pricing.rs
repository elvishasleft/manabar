pub struct Price {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
}

pub struct PriceTable {
    pub entries: Vec<(String, Price)>,
}

impl PriceTable {
    pub fn empty() -> Self {
        Self { entries: vec![] }
    }

    pub fn price_for(&self, model: &str) -> Option<&Price> {
        self.entries
            .iter()
            .find(|(k, _)| model.contains(k.as_str()))
            .map(|(_, p)| p)
    }

    pub fn default_table() -> Self {
        let e = |k: &str, input: f64, output: f64, cache_read: f64, cache_write: f64| {
            (
                k.to_string(),
                Price {
                    input,
                    output,
                    cache_read,
                    cache_write,
                },
            )
        };
        Self {
            entries: vec![
                e("claude-opus", 15.0, 75.0, 1.5, 18.75),
                e("claude-sonnet", 3.0, 15.0, 0.3, 3.75),
                e("claude-haiku", 1.0, 5.0, 0.1, 1.25),
                e("claude", 3.0, 15.0, 0.3, 3.75),
                e("gpt-5", 1.25, 10.0, 0.125, 0.0),
                e("codex", 1.25, 10.0, 0.125, 0.0),
            ],
        }
    }
}

pub fn cost_usd(p: &Price, input: u64, cache_read: u64, cache_write: u64, output: u64) -> f64 {
    (input as f64 * p.input
        + cache_read as f64 * p.cache_read
        + cache_write as f64 * p.cache_write
        + output as f64 * p.output)
        / 1_000_000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn specific_entry_wins_over_fallback() {
        let t = PriceTable::default_table();
        let opus = t.price_for("claude-opus-4-8").unwrap();
        let generic = t.price_for("claude-fable-5").unwrap();
        assert!(
            opus.output > generic.output,
            "opus must not fall through to generic claude"
        );
    }

    #[test]
    fn unknown_model_has_no_price() {
        assert!(PriceTable::default_table().price_for("grok-4-5").is_none());
    }

    #[test]
    fn cost_is_per_million() {
        let p = Price {
            input: 3.0,
            output: 15.0,
            cache_read: 0.3,
            cache_write: 3.75,
        };
        let c = cost_usd(&p, 1_000_000, 0, 0, 0);
        assert!((c - 3.0).abs() < 1e-9);
        let c2 = cost_usd(&p, 0, 2_000_000, 0, 1_000_000);
        assert!((c2 - (0.6 + 15.0)).abs() < 1e-9);
    }
}
