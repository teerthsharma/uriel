//! `CALL` procedures: schema introspection and graph algorithms.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use uriel_algo::{
    betweenness_centrality, louvain, pagerank, weakly_connected_components, BetweennessConfig,
    LouvainConfig, PageRankConfig, Projection,
};
use uriel_core::NodeId;

use crate::ctx::Ctx;
use crate::error::{rt, sem, QResult, QueryError};
use crate::val::Val;

type ProcOut = (Vec<String>, Vec<Vec<Val>>);

fn algo_err(e: uriel_algo::Error) -> QueryError {
    QueryError::Runtime(format!("algorithm failed: {e}"))
}

impl Ctx<'_> {
    pub fn run_procedure(&mut self, ns: &[String], name: &str, args: &[Val]) -> QResult<ProcOut> {
        // procedures read arbitrary parts of the graph
        self.star = true;
        let full = if ns.is_empty() {
            name.to_string()
        } else {
            format!("{}.{name}", ns.join("."))
        };
        match full.to_ascii_lowercase().as_str() {
            "db.labels" => {
                let rows = self
                    .read
                    .labels()?
                    .into_iter()
                    .map(|l| vec![Val::Str(l)])
                    .collect();
                Ok((vec!["label".into()], rows))
            }
            "db.relationshiptypes" => {
                let rows = self
                    .read
                    .rel_types()?
                    .into_iter()
                    .map(|l| vec![Val::Str(l)])
                    .collect();
                Ok((vec!["relationshipType".into()], rows))
            }
            "db.propertykeys" => {
                let mut keys: BTreeSet<String> = BTreeSet::new();
                for n in self.read.all_nodes()? {
                    keys.extend(n?.props.into_keys());
                }
                for r in self.read.all_rels()? {
                    keys.extend(r?.props.into_keys());
                }
                Ok((
                    vec!["propertyKey".into()],
                    keys.into_iter().map(|k| vec![Val::Str(k)]).collect(),
                ))
            }
            "db.schema.visualization" => self.schema_visualization(),
            "seal.pagerank" => {
                let cfg = self.config_arg(args)?;
                let mut c = PageRankConfig::default();
                if let Some(d) = cfg.get("dampingFactor").and_then(Val::as_f64) {
                    c.damping = d;
                }
                if let Some(Val::Int(i)) = cfg.get("maxIterations") {
                    c.max_iterations = (*i).max(1) as usize;
                }
                let (ids, g) = self.project_all(true)?;
                let res = pagerank(&g, &c).map_err(algo_err)?;
                Ok(scored(
                    &ids,
                    "score",
                    res.scores.into_iter().map(Val::Float),
                ))
            }
            "seal.betweenness" => {
                let (ids, g) = self.project_all(true)?;
                let res =
                    betweenness_centrality(&g, &BetweennessConfig::default()).map_err(algo_err)?;
                Ok(scored(&ids, "score", res.into_iter().map(Val::Float)))
            }
            "seal.wcc" => {
                let (ids, g) = self.project_all(false)?;
                let labels = weakly_connected_components(&g);
                Ok(scored(
                    &ids,
                    "componentId",
                    labels.into_iter().map(|l| Val::Int(i64::from(l))),
                ))
            }
            "seal.louvain" => {
                let (ids, g) = self.project_all(false)?;
                let res = louvain(&g, &LouvainConfig::default()).map_err(algo_err)?;
                Ok(scored(
                    &ids,
                    "communityId",
                    res.communities.into_iter().map(|l| Val::Int(i64::from(l))),
                ))
            }
            _ => sem(format!(
                "There is no procedure with the name `{full}` registered"
            )),
        }
    }

    fn config_arg(&self, args: &[Val]) -> QResult<BTreeMap<String, Val>> {
        match args.first() {
            None | Some(Val::Null) => Ok(BTreeMap::new()),
            Some(Val::Map(m)) => Ok(m.clone()),
            Some(o) => rt(format!(
                "procedure configuration must be a Map, got {}",
                o.type_name()
            )),
        }
    }

    /// Projects every node and relationship of the snapshot into a CSR graph; node index
    /// `i` of the projection is `ids[i]`.
    fn project_all(&mut self, directed: bool) -> QResult<(Vec<NodeId>, Projection)> {
        let mut ids: Vec<NodeId> = Vec::new();
        for n in self.read.all_nodes()? {
            ids.push(n?.id);
        }
        ids.sort();
        let index: HashMap<NodeId, u32> = ids
            .iter()
            .enumerate()
            .map(|(i, id)| (*id, i as u32))
            .collect();
        let mut edges: Vec<(u32, u32)> = Vec::new();
        for r in self.read.all_rels()? {
            let r = r?;
            if let (Some(&a), Some(&b)) = (index.get(&r.src), index.get(&r.dst)) {
                edges.push((a, b));
            }
        }
        let g = Projection::from_edges(ids.len(), &edges, directed).map_err(algo_err)?;
        Ok((ids, g))
    }

    fn schema_visualization(&mut self) -> QResult<ProcOut> {
        let schema = self.read.schema()?;
        let mut nodes = Vec::new();
        for l in &schema.labels {
            let mut m = BTreeMap::new();
            m.insert("name".to_string(), Val::Str(l.name.clone()));
            m.insert("count".to_string(), Val::Int(l.count as i64));
            m.insert(
                "properties".to_string(),
                Val::List(l.properties.keys().cloned().map(Val::Str).collect()),
            );
            nodes.push(Val::Map(m));
        }
        let mut triples: BTreeSet<(String, String, String)> = BTreeSet::new();
        for r in self.read.all_rels()? {
            let r = r?;
            let (Some(a), Some(b)) = (self.read.get_node(r.src)?, self.read.get_node(r.dst)?)
            else {
                continue;
            };
            for la in &a.labels {
                for lb in &b.labels {
                    triples.insert((la.clone(), r.rel_type.clone(), lb.clone()));
                }
            }
        }
        let rels = triples
            .into_iter()
            .map(|(a, t, b)| {
                let mut m = BTreeMap::new();
                m.insert("type".to_string(), Val::Str(t));
                m.insert("from".to_string(), Val::Str(a));
                m.insert("to".to_string(), Val::Str(b));
                Val::Map(m)
            })
            .collect();
        Ok((
            vec!["nodes".into(), "relationships".into()],
            vec![vec![Val::List(nodes), Val::List(rels)]],
        ))
    }
}

fn scored(ids: &[NodeId], col: &str, vals: impl Iterator<Item = Val>) -> ProcOut {
    (
        vec!["nodeId".into(), col.into()],
        ids.iter()
            .zip(vals)
            .map(|(id, v)| vec![Val::Int(id.0 as i64), v])
            .collect(),
    )
}
