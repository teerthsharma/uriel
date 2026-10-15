use crate::tokenize::terms;

pub trait Embedder: Send + Sync {
    fn dim(&self) -> usize;
    fn embed(&self, text: &str) -> Vec<f32>;
}

#[derive(Debug, Clone)]
pub struct HashEmbedder {
    dim: usize,
}

impl HashEmbedder {
    pub fn new(dim: usize) -> HashEmbedder {
        HashEmbedder { dim: dim.max(8) }
    }
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h ^= h >> 33;
    h = h.wrapping_mul(0xff51_afd7_ed55_8ccd);
    h ^= h >> 33;
    h
}

impl HashEmbedder {
    fn add(&self, v: &mut [f32], feature: &[u8], weight: f32) {
        let h = fnv1a(feature);
        let idx = (h % self.dim as u64) as usize;
        let sign = if (h >> 63) & 1 == 1 { -1.0 } else { 1.0 };
        v[idx] += sign * weight;
    }
}

impl Embedder for HashEmbedder {
    fn dim(&self) -> usize {
        self.dim
    }

    fn embed(&self, text: &str) -> Vec<f32> {
        let mut v = vec![0.0f32; self.dim];
        for term in terms(text) {
            self.add(&mut v, term.as_bytes(), 1.0);
            let padded: Vec<char> = std::iter::once('<')
                .chain(term.chars())
                .chain(std::iter::once('>'))
                .collect();
            for n in [3usize, 4] {
                if padded.len() < n {
                    continue;
                }
                for w in padded.windows(n) {
                    let s: String = w.iter().collect();
                    self.add(&mut v, s.as_bytes(), 0.5);
                }
            }
        }
        let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 0.0 {
            for x in &mut v {
                *x /= norm;
            }
        }
        v
    }
}

pub(crate) fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_unit_norm_and_dimension() {
        let e = HashEmbedder::new(256);
        let a = e.embed("rolling checksum over a byte buffer");
        assert_eq!(a.len(), 256);
        assert_eq!(a, e.embed("rolling checksum over a byte buffer"));
        let norm: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-5);
        assert!(e.embed("").iter().all(|x| *x == 0.0));
    }

    #[test]
    fn related_text_is_closer_than_unrelated() {
        let e = HashEmbedder::new(256);
        let q = e.embed("compute checksum of bytes");
        let near = e.embed("computing the checksum over a byte buffer");
        let far = e.embed("prune the roses in late winter");
        assert!(dot(&q, &near) > dot(&q, &far) + 0.1);
    }
}
