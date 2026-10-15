use crate::rng::Rng;

#[derive(Debug, Clone)]
pub struct Zipf {
    cdf: Vec<f64>,
}

impl Zipf {
    pub fn new(n: usize, exponent: f64) -> Zipf {
        let n = n.max(1);
        let mut acc = 0.0;
        let mut cdf = Vec::with_capacity(n);
        for i in 0..n {
            acc += 1.0 / ((i + 1) as f64).powf(exponent);
            cdf.push(acc);
        }
        for c in &mut cdf {
            *c /= acc;
        }
        Zipf { cdf }
    }

    pub fn sample(&self, rng: &mut Rng) -> usize {
        let u = rng.unit();
        self.cdf
            .partition_point(|&c| c < u)
            .min(self.cdf.len() - 1)
    }
}

pub fn sequence(n: usize, exponent: f64, len: usize, seed: u64) -> Vec<usize> {
    let zipf = Zipf::new(n, exponent);
    let mut rng = Rng::new(seed);
    (0..len).map(|_| zipf.sample(&mut rng)).collect()
}
