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
        Self {
            entries: vec![],
        }
    }

    pub fn price_for(&self, model: &str) -> Option<&Price> {
        self.entries
            .iter()
            .find(|(k, _)| model.contains(k.as_str()))
            .map(|(_, p)| p)
    }
}

pub fn cost_usd(p: &Price, input: u64, cache_read: u64, cache_write: u64, output: u64) -> f64 {
    (input as f64 * p.input
        + cache_read as f64 * p.cache_read
        + cache_write as f64 * p.cache_write
        + output as f64 * p.output)
        / 1_000_000.0
}
