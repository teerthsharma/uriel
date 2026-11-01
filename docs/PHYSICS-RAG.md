# Future plan: particle-physics-shaped retrieval

Status: **not built**. Nothing here is a claim. Each item has a computable definition, a baseline and a condition under which it is deleted.

## Rule

A physics term is allowed in this plan only if it names something computable and a baseline to beat. If a mechanism turns out to be a known method under another name, the plan says so and keeps the known name.

## Step 0: a benchmark that can fail

The frozen evaluation is saturated (recall at 5 is 1.000 for hybrid and for BM25 over full text), so it cannot show any gain. Before anything below is built:

- **K0.** A harder query set where BM25 recall at 5 is under 0.6: paraphrases with no shared vocabulary, questions that need two files (multi-hop), and queries about a symbol that was renamed. Frozen the same way (a corpus digest), with a confidence interval on every number.

## Candidates

| # | Idea from collider physics | What it would be here | Baseline | Delete if |
|---|---|---|---|---|
| K1 | **Anti-kt jet clustering** (Cacciari, Salam, Soyez, arXiv:0802.1189): hard seeds absorb soft neighbours; the result is infrared and collinear safe | Cluster chunks around high-salience seeds with distance `min(1/s_i², 1/s_j²) · d_ij² / R²`, `s` = salience, `d` = graph or embedding distance. Replaces the union-find islands in `view` when grouping candidates | Island labels in `uriel-algo`; Louvain | It does not beat islands on K0 view recall at equal tokens |
| K2 | **Collinear and infrared safety as a test** | A metamorphic property for any retrieval change: splitting a chunk into two halves, or adding a near-duplicate, must not change which clusters are returned | None; this is a gate, not a feature | A change that breaks it cannot merge |
| K3 | **Unfolding** (iterative Bayesian unfolding, D'Agostini 1995): infer the true distribution from detector-smeared measurements using a response matrix | The resolution ladder is a lossy detector. Estimate a response matrix from the corpus (how often an L3 gist hides what L0 contains), then decide which hits to `rag_expand` by expected information gain instead of fixed rules | Current `pack_ladder` order; a fixed zoom-the-top-hit rule | It does not cut expands at equal answer accuracy |
| K4 | **Symmetries and conservation laws** | Invariances as property tests: renaming identifiers consistently, permuting file order and duplicating a chunk must leave ranks unchanged (up to ties) | None; a bug detector | Never deleted; it is a test suite |
| K5 | **Monte Carlo event generation plus detector simulation** | Sample synthetic questions from the graph (a symbol, a change, a failing test), run the pipeline, and label them for free. This is how K0 grows without hand labelling | Hand-written queries | Synthetic queries do not rank methods the same way hand-written ones do |
| K6 | **Systematic uncertainties and profile likelihood** | Treat BM25 and vector disagreement as a nuisance parameter and report, with each hit, whether its rank is stable under it, using the outward-rounded bounds already in `uriel-algo::certify` | Plain fused score | Certified ranks are not more often correct than uncertified ones |

## What is deliberately not on the list

- **A sum over paths with a propagator** is the Neumann series `(I − αA)⁻¹`, which is personalised PageRank or Katz centrality. The retrieval already has that. The physics name would add nothing, so it stays PageRank.
- **Calorimetry as token allocation** is a budgeted knapsack with a concave gain, which `pack_ladder` already is.
- Anything whose only support is that the vocabulary sounds right.

## Order and gates

1. K0 and K5 first, so there is something that can fail.
2. K2 and K4 next: they are tests, they cost nothing at query time, and they protect everything after.
3. K1, K3, K6 each as an ablation on K0 with a confidence interval. A result whose interval includes zero is a negative result and is recorded in `PLAN.md` like the topological-salience loss already is.
