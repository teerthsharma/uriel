use crate::error::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Metric {
    Euclidean,
    Cosine,
}

#[derive(Debug, Clone)]
pub struct PersistenceConfig {
    pub metric: Metric,
    pub dense_limit: usize,
    pub knn: usize,
    pub essential_cap: Option<f64>,
}

impl Default for PersistenceConfig {
    fn default() -> Self {
        Self {
            metric: Metric::Euclidean,
            dense_limit: 4096,
            knn: 16,
            essential_cap: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bar {
    pub birth: f64,
    pub death: f64,
    pub id: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Diagram {
    pub finite: Vec<Bar>,
    pub essential_id: Option<u64>,
    pub essential_death: f64,
    pub ids: Vec<u64>,
    pub salience: Vec<f64>,
    pub approximate: bool,
}

impl Diagram {
    pub fn salience_of(&self, id: u64) -> Option<f64> {
        self.ids
            .iter()
            .position(|&x| x == id)
            .map(|i| self.salience[i])
    }
}

pub fn persistence_f64(
    data: &[f64],
    dim: usize,
    ids: Option<&[u64]>,
    cfg: &PersistenceConfig,
) -> Result<Diagram> {
    let n = validate(data.len(), dim, ids, cfg)?;
    if let Some(i) = data.iter().position(|x| !x.is_finite()) {
        return Err(Error::NonFinite(format!("coordinate {i} is {}", data[i])));
    }
    build(data, dim, n, ids, cfg)
}

pub fn persistence_f32(
    data: &[f32],
    dim: usize,
    ids: Option<&[u64]>,
    cfg: &PersistenceConfig,
) -> Result<Diagram> {
    let n = validate(data.len(), dim, ids, cfg)?;
    if let Some(i) = data.iter().position(|x| !x.is_finite()) {
        return Err(Error::NonFinite(format!("coordinate {i} is {}", data[i])));
    }
    let wide: Vec<f64> = data.iter().map(|&x| f64::from(x)).collect();
    build(&wide, dim, n, ids, cfg)
}

fn validate(len: usize, dim: usize, ids: Option<&[u64]>, cfg: &PersistenceConfig) -> Result<usize> {
    if let Some(c) = cfg.essential_cap {
        if !c.is_finite() || c < 0.0 {
            return Err(Error::InvalidParameter(format!(
                "essential_cap {c} must be finite and >= 0"
            )));
        }
    }
    if cfg.knn == 0 {
        return Err(Error::InvalidParameter("knn must be at least 1".into()));
    }
    if dim == 0 {
        return Err(Error::Shape("dim must be at least 1".into()));
    }
    if !len.is_multiple_of(dim) {
        return Err(Error::Shape(format!(
            "{len} values are not a multiple of dim {dim}"
        )));
    }
    let n = len / dim;
    if let Some(ids) = ids {
        if ids.len() != n {
            return Err(Error::Shape(format!("{} ids for {n} points", ids.len())));
        }
    }
    Ok(n)
}

pub(crate) struct Cloud {
    data: Vec<f64>,
    dim: usize,
    n: usize,
    metric: Metric,
}

impl Cloud {
    pub(crate) fn new(data: &[f64], dim: usize, metric: Metric) -> Self {
        let n = data.len() / dim;
        let mut data = data.to_vec();
        if metric == Metric::Cosine {
            for row in data.chunks_mut(dim) {
                let m = row.iter().fold(0.0f64, |a, x| a.max(x.abs()));
                if m > 0.0 {
                    for x in row.iter_mut() {
                        *x /= m;
                    }
                    let norm = row.iter().map(|x| x * x).sum::<f64>().sqrt();
                    for x in row.iter_mut() {
                        *x /= norm;
                    }
                }
            }
        }
        Self {
            data,
            dim,
            n,
            metric,
        }
    }

    #[inline]
    pub(crate) fn dist(&self, i: usize, j: usize) -> f64 {
        let a = &self.data[i * self.dim..(i + 1) * self.dim];
        let b = &self.data[j * self.dim..(j + 1) * self.dim];
        let mut s = 0.0;
        for k in 0..self.dim {
            let t = a[k] - b[k];
            s += t * t;
        }
        match self.metric {
            Metric::Euclidean => s.sqrt(),
            Metric::Cosine => {
                let za = a.iter().all(|&x| x == 0.0);
                let zb = b.iter().all(|&x| x == 0.0);
                match (za, zb) {
                    (true, true) => 0.0,
                    (true, false) | (false, true) => 1.0,
                    (false, false) => 0.5 * s,
                }
            }
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.n
    }
}

pub(crate) type Edge = (usize, usize, f64);

fn build(
    data: &[f64],
    dim: usize,
    n: usize,
    ids: Option<&[u64]>,
    cfg: &PersistenceConfig,
) -> Result<Diagram> {
    let ids: Vec<u64> = match ids {
        Some(v) => v.to_vec(),
        None => (0..n as u64).collect(),
    };
    check_unique(&ids)?;
    let cloud = Cloud::new(data, dim, cfg.metric);
    let approximate = n > cfg.dense_limit;
    let tree = if approximate {
        knn_tree(&cloud, cfg.knn)
    } else {
        prim_tree(&cloud)
    };
    if tree.iter().any(|e| !e.2.is_finite()) {
        return Err(Error::Overflow);
    }
    Ok(diagram_from_tree(
        &ids,
        &tree,
        cfg.essential_cap,
        approximate,
    ))
}

pub(crate) fn check_unique(ids: &[u64]) -> Result<()> {
    let mut s = ids.to_vec();
    s.sort_unstable();
    if let Some(w) = s.windows(2).find(|w| w[0] == w[1]) {
        return Err(Error::DuplicateId(w[0]));
    }
    Ok(())
}

pub(crate) fn prim_tree(c: &Cloud) -> Vec<Edge> {
    let n = c.len();
    if n < 2 {
        return Vec::new();
    }
    let mut in_tree = vec![false; n];
    let mut best = vec![f64::INFINITY; n];
    let mut from = vec![0usize; n];
    in_tree[0] = true;
    for (j, b) in best.iter_mut().enumerate().skip(1) {
        *b = c.dist(0, j);
    }
    let mut edges = Vec::with_capacity(n - 1);
    for _ in 1..n {
        let mut pick = usize::MAX;
        for j in 0..n {
            if !in_tree[j] && (pick == usize::MAX || best[j].total_cmp(&best[pick]).is_lt()) {
                pick = j;
            }
        }
        in_tree[pick] = true;
        edges.push((from[pick], pick, best[pick]));
        for j in 0..n {
            if !in_tree[j] {
                let d = c.dist(pick, j);
                if d.total_cmp(&best[j]).is_lt() {
                    best[j] = d;
                    from[j] = pick;
                }
            }
        }
    }
    edges
}

pub(crate) struct Dsu {
    pub(crate) parent: Vec<usize>,
}

impl Dsu {
    pub(crate) fn new(n: usize) -> Self {
        Self {
            parent: (0..n).collect(),
        }
    }
    pub(crate) fn find(&mut self, mut x: usize) -> usize {
        while self.parent[x] != x {
            self.parent[x] = self.parent[self.parent[x]];
            x = self.parent[x];
        }
        x
    }
    pub(crate) fn union(&mut self, a: usize, b: usize) -> bool {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra == rb {
            return false;
        }
        let (lo, hi) = (ra.min(rb), ra.max(rb));
        self.parent[hi] = lo;
        true
    }
}

pub(crate) fn edge_order(a: &Edge, b: &Edge) -> std::cmp::Ordering {
    a.2.total_cmp(&b.2)
        .then(a.0.min(a.1).cmp(&b.0.min(b.1)))
        .then(a.0.max(a.1).cmp(&b.0.max(b.1)))
}

fn knn_tree(c: &Cloud, k: usize) -> Vec<Edge> {
    let n = c.len();
    if n < 2 {
        return Vec::new();
    }
    let k = k.min(n - 1);
    let mut cand: Vec<Edge> = Vec::with_capacity(n * k);
    let mut buf: Vec<(f64, usize)> = Vec::with_capacity(n);
    for i in 0..n {
        buf.clear();
        for j in 0..n {
            if j != i {
                buf.push((c.dist(i, j), j));
            }
        }
        buf.select_nth_unstable_by(k - 1, |a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        for &(d, j) in &buf[..k] {
            cand.push((i.min(j), i.max(j), d));
        }
    }
    cand.sort_by(edge_order);
    cand.dedup_by(|a, b| a.0 == b.0 && a.1 == b.1);
    let mut dsu = Dsu::new(n);
    let mut tree: Vec<Edge> = Vec::with_capacity(n - 1);
    for e in cand {
        if dsu.union(e.0, e.1) {
            tree.push(e);
        }
    }
    while tree.len() < n - 1 {
        let roots: Vec<usize> = (0..n).map(|i| dsu.find(i)).collect();
        let mut best: Vec<Option<Edge>> = vec![None; n];
        for i in 0..n {
            for j in 0..n {
                if roots[i] == roots[j] {
                    continue;
                }
                let e = (i.min(j), i.max(j), c.dist(i, j));
                let slot = &mut best[roots[i]];
                if slot.is_none_or(|b| edge_order(&e, &b).is_lt()) {
                    *slot = Some(e);
                }
            }
        }
        let mut chosen: Vec<Edge> = best.into_iter().flatten().collect();
        chosen.sort_by(edge_order);
        for e in chosen {
            if dsu.union(e.0, e.1) {
                tree.push(e);
            }
        }
    }
    tree
}

pub(crate) fn diagram_from_tree(
    ids: &[u64],
    tree: &[Edge],
    cap: Option<f64>,
    approximate: bool,
) -> Diagram {
    let n = ids.len();
    let mut order: Vec<Edge> = tree.to_vec();
    order.sort_by(|a, b| {
        a.2.total_cmp(&b.2)
            .then_with(|| {
                let (ia, ib) = (ids[a.0].min(ids[a.1]), ids[b.0].min(ids[b.1]));
                ia.cmp(&ib)
            })
            .then_with(|| {
                let (ia, ib) = (ids[a.0].max(ids[a.1]), ids[b.0].max(ids[b.1]));
                ia.cmp(&ib)
            })
    });
    let mut dsu = Dsu::new(n);
    let mut rep: Vec<u64> = ids.to_vec();
    let mut finite: Vec<Bar> = Vec::with_capacity(n.saturating_sub(1));
    let mut salience = vec![0.0; n];
    let pos_of: std::collections::BTreeMap<u64, usize> =
        ids.iter().enumerate().map(|(i, &id)| (id, i)).collect();
    for &(a, b, w) in &order {
        let (ra, rb) = (dsu.find(a), dsu.find(b));
        if ra == rb {
            continue;
        }
        let (dead, live) = if rep[ra] > rep[rb] {
            (ra, rb)
        } else {
            (rb, ra)
        };
        let dead_id = rep[dead];
        finite.push(Bar {
            birth: 0.0,
            death: w,
            id: dead_id,
        });
        salience[pos_of[&dead_id]] = w;
        let keep = rep[live];
        let root = {
            dsu.parent[dead] = live;
            live
        };
        rep[root] = keep;
    }
    finite.sort_by(|a, b| a.death.total_cmp(&b.death).then(a.id.cmp(&b.id)));
    let max_death = finite.iter().fold(0.0f64, |m, b| m.max(b.death));
    let essential_death = cap.unwrap_or(max_death);
    let essential_id = ids.iter().copied().min();
    if let Some(e) = essential_id {
        salience[pos_of[&e]] = essential_death;
    }
    Diagram {
        finite,
        essential_id,
        essential_death,
        ids: ids.to_vec(),
        salience,
        approximate,
    }
}

pub(crate) fn mst_insert(tree: &mut Vec<Edge>, c: &Cloud, p: usize) {
    let mut cand = std::mem::take(tree);
    for j in 0..p {
        cand.push((j, p, c.dist(j, p)));
    }
    cand.sort_by(edge_order);
    let mut dsu = Dsu::new(p + 1);
    for e in cand {
        if dsu.union(e.0, e.1) {
            tree.push(e);
        }
    }
}
