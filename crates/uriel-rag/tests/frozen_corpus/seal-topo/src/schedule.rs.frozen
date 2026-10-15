use crate::error::{Error, Result};
use crate::persistence::{diagram_from_tree, mst_insert, prim_tree, Cloud, Metric};

#[derive(Debug, Clone)]
pub struct Blocks {
    pub ids: Vec<u64>,
    pub centroids: Vec<f64>,
    pub dim: usize,
    pub tokens: Vec<usize>,
}

impl Blocks {
    pub fn new(ids: Vec<u64>, centroids: Vec<f64>, dim: usize, tokens: Vec<usize>) -> Result<Self> {
        let b = Self {
            ids,
            centroids,
            dim,
            tokens,
        };
        b.validate()?;
        Ok(b)
    }

    pub fn from_f32(
        ids: Vec<u64>,
        centroids: &[f32],
        dim: usize,
        tokens: Vec<usize>,
    ) -> Result<Self> {
        if let Some(i) = centroids.iter().position(|x| !x.is_finite()) {
            return Err(Error::NonFinite(format!("centroid value {i}")));
        }
        Self::new(
            ids,
            centroids.iter().map(|&x| f64::from(x)).collect(),
            dim,
            tokens,
        )
    }

    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    pub fn validate(&self) -> Result<()> {
        let n = self.ids.len();
        if self.dim == 0 {
            return Err(Error::Shape("dim must be at least 1".into()));
        }
        if self.centroids.len() != n * self.dim {
            return Err(Error::Shape(format!(
                "{} centroid values for {n} blocks of dim {}",
                self.centroids.len(),
                self.dim
            )));
        }
        if self.tokens.len() != n {
            return Err(Error::Shape(format!(
                "{} token sizes for {n} blocks",
                self.tokens.len()
            )));
        }
        if let Some(i) = self.centroids.iter().position(|x| !x.is_finite()) {
            return Err(Error::NonFinite(format!("centroid value {i}")));
        }
        for w in self.ids.windows(2) {
            if w[0] == w[1] {
                return Err(Error::DuplicateId(w[0]));
            }
            if w[0] > w[1] {
                return Err(Error::InvalidParameter(format!(
                    "block ids must be increasing in causal order: {} before {}",
                    w[0], w[1]
                )));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct ScheduleParams {
    pub sink_blocks: usize,
    pub local_window: usize,
    pub salient_k: usize,
    pub relevant_m: usize,
    pub token_budget: Option<usize>,
    pub metric: Metric,
}

impl Default for ScheduleParams {
    fn default() -> Self {
        Self {
            sink_blocks: 1,
            local_window: 4,
            salient_k: 8,
            relevant_m: 0,
            token_budget: None,
            metric: Metric::Cosine,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Schedule {
    pub indptr: Vec<usize>,
    pub indices: Vec<usize>,
}

impl Schedule {
    pub fn rows(&self) -> usize {
        self.indptr.len().saturating_sub(1)
    }

    pub fn row(&self, i: usize) -> &[usize] {
        &self.indices[self.indptr[i]..self.indptr[i + 1]]
    }

    pub fn to_csr(&self) -> Result<(Vec<i32>, Vec<i32>)> {
        let conv = |v: &[usize]| -> Result<Vec<i32>> {
            v.iter()
                .map(|&x| {
                    i32::try_from(x)
                        .map_err(|_| Error::OutOfRange(format!("{x} does not fit in i32")))
                })
                .collect()
        };
        Ok((conv(&self.indptr)?, conv(&self.indices)?))
    }
}

pub fn dense_causal(n: usize) -> Schedule {
    let mut indptr = Vec::with_capacity(n + 1);
    let mut indices = Vec::with_capacity(n * (n + 1) / 2);
    indptr.push(0);
    for i in 0..n {
        indices.extend(0..=i);
        indptr.push(indices.len());
    }
    Schedule { indptr, indices }
}

fn check_row(b: &Blocks, row: usize) -> Result<()> {
    if row >= b.len() {
        return Err(Error::OutOfRange(format!(
            "row {row} of {} blocks",
            b.len()
        )));
    }
    Ok(())
}

fn check_query(b: &Blocks, q: Option<&[f64]>) -> Result<()> {
    if let Some(q) = q {
        if q.len() != b.dim {
            return Err(Error::Shape(format!(
                "query has {} values, blocks have dim {}",
                q.len(),
                b.dim
            )));
        }
        if let Some(i) = q.iter().position(|x| !x.is_finite()) {
            return Err(Error::NonFinite(format!("query value {i}")));
        }
    }
    Ok(())
}

pub fn prefix_salience(b: &Blocks, row: usize, metric: Metric) -> Result<Vec<f64>> {
    b.validate()?;
    check_row(b, row)?;
    let m = row + 1;
    let cloud = Cloud::new(&b.centroids[..m * b.dim], b.dim, metric);
    let tree = prim_tree(&cloud);
    if tree.iter().any(|e| !e.2.is_finite()) {
        return Err(Error::Overflow);
    }
    Ok(diagram_from_tree(&b.ids[..m], &tree, None, false).salience)
}

pub(crate) fn relevance_order(
    b: &Blocks,
    row: usize,
    metric: Metric,
    q: &[f64],
) -> Result<Vec<usize>> {
    let m = row + 1;
    let mut data = Vec::with_capacity((m + 1) * b.dim);
    data.extend_from_slice(&b.centroids[..m * b.dim]);
    data.extend_from_slice(q);
    let cloud = Cloud::new(&data, b.dim, metric);
    let mut v: Vec<(f64, usize)> = (0..m).map(|i| (cloud.dist(i, m), i)).collect();
    if v.iter().any(|x| !x.0.is_finite()) {
        return Err(Error::Overflow);
    }
    v.sort_by(|a, c| a.0.total_cmp(&c.0).then(a.1.cmp(&c.1)));
    Ok(v.into_iter().map(|x| x.1).collect())
}

pub fn row_priority(
    b: &Blocks,
    row: usize,
    p: &ScheduleParams,
    query: Option<&[f64]>,
    salience: &[f64],
) -> Result<Vec<usize>> {
    check_row(b, row)?;
    check_query(b, query)?;
    if salience.len() != row + 1 {
        return Err(Error::Shape(format!(
            "{} salience values for a prefix of {}",
            salience.len(),
            row + 1
        )));
    }
    let m = row + 1;
    let mut seen = vec![false; m];
    let mut out: Vec<usize> = Vec::new();
    let mut push = |i: usize, out: &mut Vec<usize>| {
        if !seen[i] {
            seen[i] = true;
            out.push(i);
        }
    };
    for i in 0..p.sink_blocks.min(m) {
        push(i, &mut out);
    }
    for k in 0..p.local_window.min(m) {
        push(row - k, &mut out);
    }
    if let (Some(q), true) = (query, p.relevant_m > 0) {
        for i in relevance_order(b, row, p.metric, q)?
            .into_iter()
            .take(p.relevant_m)
        {
            push(i, &mut out);
        }
    }
    if p.salient_k > 0 {
        let mut order: Vec<usize> = (0..m).collect();
        order.sort_by(|&a, &c| salience[c].total_cmp(&salience[a]).then(a.cmp(&c)));
        for i in order.into_iter().take(p.salient_k) {
            push(i, &mut out);
        }
    }
    Ok(out)
}

pub(crate) fn cut(b: &Blocks, prio: Vec<usize>, budget: Option<usize>) -> Vec<usize> {
    let mut keep = Vec::with_capacity(prio.len());
    match budget {
        None => keep = prio,
        Some(limit) => {
            let mut total = 0usize;
            for i in prio {
                match total.checked_add(b.tokens[i]) {
                    Some(t) if t <= limit => {
                        total = t;
                        keep.push(i);
                    }
                    _ => break,
                }
            }
        }
    }
    keep.sort_unstable();
    keep
}

pub fn select_row(
    b: &Blocks,
    row: usize,
    p: &ScheduleParams,
    query: Option<&[f64]>,
    salience: &[f64],
) -> Result<Vec<usize>> {
    let prio = row_priority(b, row, p, query, salience)?;
    Ok(cut(b, prio, p.token_budget))
}

pub fn schedule_row(
    b: &Blocks,
    row: usize,
    p: &ScheduleParams,
    query: Option<&[f64]>,
) -> Result<Vec<usize>> {
    b.validate()?;
    check_row(b, row)?;
    check_query(b, query)?;
    let sal = prefix_salience(b, row, p.metric)?;
    select_row(b, row, p, query, &sal)
}

pub fn schedule(b: &Blocks, p: &ScheduleParams, query: Option<&[f64]>) -> Result<Schedule> {
    b.validate()?;
    check_query(b, query)?;
    let n = b.len();
    let cloud = Cloud::new(&b.centroids, b.dim, p.metric);
    let mut tree = Vec::new();
    let mut indptr = Vec::with_capacity(n + 1);
    let mut indices = Vec::new();
    indptr.push(0);
    for row in 0..n {
        mst_insert(&mut tree, &cloud, row);
        if tree.iter().any(|e| !e.2.is_finite()) {
            return Err(Error::Overflow);
        }
        let sal = diagram_from_tree(&b.ids[..=row], &tree, None, false).salience;
        let q = if row + 1 == n { query } else { None };
        indices.extend(select_row(b, row, p, q, &sal)?);
        indptr.push(indices.len());
    }
    Ok(Schedule { indptr, indices })
}
