const MAX_WINDOW: usize = 64;

pub fn dominant_prefix_len(samples: &[&[u8]], threshold: f64) -> usize {
    let total = samples.len();
    let required = ((threshold * total as f64).ceil() as usize).clamp(total / 2 + 1, total.max(1));
    let mut members: Vec<&[u8]> = samples.to_vec();
    let mut depth = 0;
    loop {
        let mut counts = [0usize; 256];
        for sample in &members {
            if let Some(&byte) = sample.get(depth) {
                counts[usize::from(byte)] += 1;
            }
        }
        let Some(dominant) = counts.iter().position(|&c| c >= required) else {
            return depth;
        };
        members.retain(|sample| sample.get(depth).map(|&b| usize::from(b)) == Some(dominant));
        depth += 1;
    }
}

pub fn agreement(outcomes: &[usize]) -> f64 {
    let n = outcomes.len();
    if n < 2 {
        return 1.0;
    }
    let mut sorted = outcomes.to_vec();
    sorted.sort_unstable();
    let mut pairs = 0usize;
    let mut run = 0usize;
    for (i, value) in sorted.iter().enumerate() {
        run = if i > 0 && sorted[i - 1] == *value { run + 1 } else { 1 };
        pairs += run - 1;
    }
    (2 * pairs) as f64 / (n * (n - 1)) as f64
}

#[derive(Debug, Clone)]
pub struct PrefixLearner {
    threshold: f64,
    fingerprint: Option<String>,
    window: Vec<Vec<u8>>,
}

impl PrefixLearner {
    pub fn new(threshold: f64) -> Self {
        Self {
            threshold,
            fingerprint: None,
            window: Vec::new(),
        }
    }

    pub fn observe(&mut self, fingerprint: &str, bytes: &[u8]) -> bool {
        let reopened = self.fingerprint.as_deref().is_some_and(|f| f != fingerprint);
        if reopened {
            self.window.clear();
        }
        self.fingerprint = Some(fingerprint.to_string());
        if self.window.len() == MAX_WINDOW {
            self.window.remove(0);
        }
        self.window.push(bytes.to_vec());
        reopened
    }

    pub fn stable_len(&self) -> usize {
        let refs: Vec<&[u8]> = self.window.iter().map(Vec::as_slice).collect();
        dominant_prefix_len(&refs, self.threshold)
    }

    pub fn len(&self) -> usize {
        self.window.len()
    }

    pub fn is_empty(&self) -> bool {
        self.window.is_empty()
    }
}
