#!/usr/bin/env python3
"""Audit every tool of the uriel MCP server (stdio, JSON-RPC 2.0) and the CLI.

Spawns `uriel mcp` in normal, house, read-only and low-memory configurations,
calls every listed tool with realistic and with deliberately bad arguments,
and writes <outdir>/audit.json and <outdir>/AUDIT.md. Stdlib only, Python 3.9+.
Exit code 1 if any non-expected failure was recorded.
"""
import argparse, json, os, queue, re, shutil, subprocess, sys, tempfile, threading, time
from pathlib import Path

TIMEOUT = 20.0
HOUSE_PREFIX = ("house_", "research_", "genius_", "dispatch_")
VERTS = ["abduce", "analogize", "question", "refute", "eliminate", "monitor"]
RECORDS, EXTRA = [], {"lock": [], "cli": [], "list_sizes": [], "protocol_notes": []}
CORPUS = {
    "math.rs": "/// Compute the nth fibonacci number iteratively.\npub fn fibonacci(n: u32) -> u64 {\n    let (mut a, mut b) = (0u64, 1u64);\n    for _ in 0..n {\n        let t = a + b;\n        a = b;\n        b = t;\n    }\n    a\n}\n\n/// Greatest common divisor by the euclidean algorithm.\npub fn gcd(a: u64, b: u64) -> u64 {\n    if b == 0 { a } else { gcd(b, a % b) }\n}\n",
    "tokenizer.rs": "//! A tiny tokenizer used by the parser.\npub struct Tokenizer { pos: usize, src: String }\nimpl Tokenizer {\n    pub fn new(src: &str) -> Self { Tokenizer { pos: 0, src: src.to_string() } }\n    pub fn next_token(&mut self) -> Option<&str> {\n        let rest = &self.src[self.pos..];\n        let rest = rest.trim_start();\n        if rest.is_empty() { return None; }\n        let end = rest.find(' ').unwrap_or(rest.len());\n        self.pos = self.src.len() - rest.len() + end;\n        Some(&rest[..end])\n    }\n}\n",
    "notes.md": "# Design notes\n\nThe ledger keeps dependent goals in topological order.\n\n## Caching\n\nThe query cache is invalidated on every write transaction. Reads hit the cache first.\n\n## Retrieval\n\nHybrid BM25 plus vector search ranks compressed memories; a budget packs the hits.\n",
    "README.txt": "Audit corpus. This directory holds a few small files used to exercise rag_index, rag_search and rag_expand.\nIt mentions fibonacci, tokenizer, ledger and cache so that queries find something.\n",
    "util.py": "def slugify(text):\n    \"\"\"Lower-case a title and join words with dashes.\"\"\"\n    return '-'.join(w for w in text.lower().split() if w)\n\n\ndef chunked(items, size):\n    \"\"\"Yield consecutive slices of the list.\"\"\"\n    for i in range(0, len(items), size):\n        yield items[i:i + size]\n",
    "config.md": "## Configuration\n\nThe server reads a JSON config with model slots. Memory budget defaults to 50 MiB split between the page cache and the RAG index.\n",
}


def jload(text):
    try:
        return json.loads(text)
    except (TypeError, ValueError):
        return None


def short(x, n=160):
    s = x if isinstance(x, str) else json.dumps(x, ensure_ascii=False)
    return s if len(s) <= n else s[:n] + "..."


def peak_rss(proc, hist):
    try:
        if os.name == "nt":
            import ctypes
            from ctypes import wintypes

            class PMC(ctypes.Structure):
                _fields_ = [("cb", wintypes.DWORD), ("pf", wintypes.DWORD)] + [(n, ctypes.c_size_t) for n in (
                    "peak_ws", "ws", "qpp", "qp", "qpnp", "qnp", "pagefile", "peak_pagefile")]
            pmc = PMC()
            pmc.cb = ctypes.sizeof(pmc)
            fn = ctypes.windll.psapi.GetProcessMemoryInfo
            fn.argtypes = [wintypes.HANDLE, ctypes.POINTER(PMC), wintypes.DWORD]
            if fn(wintypes.HANDLE(int(proc._handle)), ctypes.byref(pmc), pmc.cb):
                return pmc.peak_ws / 1048576
        elif Path(f"/proc/{proc.pid}/status").exists():
            m = re.search(r"VmHWM:\s+(\d+) kB", Path(f"/proc/{proc.pid}/status").read_text())
            return int(m.group(1)) / 1024
        else:
            out = subprocess.run(["ps", "-o", "rss=", "-p", str(proc.pid)], capture_output=True, text=True).stdout
            hist[0] = max(hist[0], int(out.strip()) / 1024)
            return hist[0]
    except Exception:
        pass
    return None


class Server:
    def __init__(self, binary, argv, cwd, env):
        self.cmd, self.cwd, self.env = [str(binary), "mcp", *map(str, argv)], cwd, env
        self.restarts, self.hist = 0, [0.0]
        self.start()

    def start(self):
        self.proc = subprocess.Popen(self.cmd, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                     stderr=subprocess.PIPE, cwd=self.cwd, env=self.env)
        self.q, self.err, self.notes, self.next_id = queue.Queue(), [], [], 1
        threading.Thread(target=self._pump, daemon=True).start()
        threading.Thread(target=lambda: [self.err.append(l.decode("utf-8", "replace")) for l in self.proc.stderr], daemon=True).start()

    def _pump(self):
        for line in iter(self.proc.stdout.readline, b""):
            self.q.put(line.decode("utf-8", "replace").strip())
        self.q.put(None)

    def stderr(self):
        return "".join(self.err)[-600:]

    def notify(self, method):
        self.proc.stdin.write((json.dumps({"jsonrpc": "2.0", "method": method}) + "\n").encode())
        self.proc.stdin.flush()

    def rpc(self, method, params=None, timeout=TIMEOUT, raw=None):
        """Returns (status, response_obj, latency_ms, response_bytes, request_text)."""
        rid = self.next_id
        self.next_id += 1
        req = raw if raw is not None else json.dumps({"jsonrpc": "2.0", "id": rid, "method": method, "params": params if params is not None else {}})
        t0 = time.perf_counter()
        ms = lambda: round((time.perf_counter() - t0) * 1000, 1)
        try:
            self.proc.stdin.write(req.encode("utf-8") + b"\n")
            self.proc.stdin.flush()
        except OSError:
            return "server-died", None, ms(), 0, req
        while True:
            left = timeout - (time.perf_counter() - t0)
            try:
                line = self.q.get(timeout=max(left, 0.001))
            except queue.Empty:
                return "timeout", None, ms(), 0, req
            if line is None:
                return "server-died", None, ms(), 0, req
            obj = jload(line)
            if isinstance(obj, dict) and "method" in obj:
                self.notes.append(obj["method"])
                continue
            if raw is not None or isinstance(obj, list) or (isinstance(obj, dict) and obj.get("id") == rid):
                return classify(obj), obj, ms(), len(line.encode()), req

    def rss(self):
        return peak_rss(self.proc, self.hist)

    def init(self):
        r = self.rpc("initialize", {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "audit", "version": "1"}})
        if r[0] == "ok":
            self.notify("notifications/initialized")
        return r

    def close(self):
        try:
            self.proc.stdin.close()
            self.proc.wait(timeout=10)
        except Exception:
            self.proc.kill()

    def restart(self):
        self.proc.kill()
        self.restarts += 1
        self.start()
        self.init()


def classify(obj):
    if not isinstance(obj, dict):
        return "protocol-error"
    if "error" in obj:
        return "protocol-error"
    if isinstance(obj.get("result"), dict) and obj["result"].get("isError"):
        return "typed-error"
    return "ok"


def text_of(obj):
    try:
        return obj["result"]["content"][0]["text"]
    except (KeyError, IndexError, TypeError):
        return json.dumps(obj.get("error", obj)) if isinstance(obj, dict) else str(obj)


def passes(expect, status):
    return {"ok": status == "ok", "error": status in ("typed-error", "protocol-error"),
            "either": status in ("ok", "typed-error")}[expect]


class Session:
    def __init__(self, phase, binary, argv, cwd, env):
        self.phase, self.state = phase, {"ledger": f"audit-{phase}"}
        self.srv = Server(binary, argv, cwd, env)
        self.listed, self.writes = set(), set()
        self.mode = argv[argv.index("--mode") + 1] if "--mode" in argv else "normal"
        self.ro = "--read-only" in argv
        self.called_ok = set()
        self.record("(protocol)", "initialize", "protocol", "ok", self.srv.init(), {})
        r = self.srv.rpc("tools/list")
        self.record("(protocol)", "tools/list", "protocol", "ok", r, {})
        for t in (r[1] or {}).get("result", {}).get("tools", []):
            self.listed.add(t["name"])
            self.tools_schema = getattr(self, "tools_schema", {})
            self.tools_schema[t["name"]] = t["inputSchema"]
        EXTRA["list_sizes"].append({"phase": phase, "mode": self.mode, "read_only": self.ro, "tools": len(self.listed), "bytes": r[3]})

    def record(self, tool, label, kind, expect, r, args, note="", must=None):
        status, obj, ms, nbytes, req = r
        text = text_of(obj) if obj is not None else ""
        ok = passes(expect, status)
        if ok and must and must not in text:
            ok, note = False, f"{note} expected text containing '{must}'".strip()
        odd = (kind == "auto" and status == "typed-error") or (expect == "either" and status == "typed-error") or ms > 5000
        RECORDS.append({"phase": self.phase, "mode": self.mode, "read_only": self.ro, "tool": tool, "label": label, "kind": kind,
                        "expect": expect, "result": status, "pass": ok, "odd": odd and ok, "latency_ms": ms, "resp_bytes": nbytes,
                        "peak_rss_mb": round(self.srv.rss() or 0, 1) or None, "args": short(args), "note": note,
                        "request": req if not ok or odd else short(req, 400), "response": text[:3000] if (not ok or odd) else text[:200]})
        if status in ("timeout", "server-died"):
            RECORDS[-1]["note"] += f" stderr: {self.srv.stderr()}"
            self.srv.restart()
        return RECORDS[-1]

    def call(self, tool, args, kind="good", expect="ok", cap=None, must=None, label=None, note=""):
        if tool not in self.listed and kind != "bad" and not (self.ro or self.mode != "house" and tool.startswith(HOUSE_PREFIX)):
            note = "tool missing from tools/list"
            expect = "ok"
        r = self.srv.rpc("tools/call", {"name": tool, "arguments": args})
        rec = self.record(tool, label or tool, kind, expect, r, args, note, must)
        if rec["pass"] and kind in ("good", "auto"):
            self.called_ok.add(tool)
        if cap and r[1] is not None and rec["result"] == "ok":
            try:
                cap(self.state, jload(text_of(r[1])), text_of(r[1]))
            except Exception as e:
                rec["note"] += f" capture failed: {e!r}"
                rec["pass"] = False
        return rec

    def step(self, s):
        tool, args, cap, expect, must = s
        args = args(self.state) if callable(args) else args
        hidden = tool not in self.listed
        if self.mode != "house" and tool.startswith(HOUSE_PREFIX):
            return self.call(tool, args, "probe", "error", must="wrong_mode", note="house tool in normal mode")
        if self.ro and hidden:
            return self.call(tool, args, "probe", "error", must="disabled", note="write tool in --read-only")
        return self.call(tool, args, "good", expect, cap, must)

    def bad_calls(self):
        for name in sorted(self.listed):
            sch = self.tools_schema[name]
            self.call(name, "not-an-object", "bad", "error", label=f"{name} (non-object args)")
            req = sch.get("required", [])
            if req:
                self.call(name, {}, "bad", "error", label=f"{name} (missing required)")
                wrong = {k: {"string": 12345, "integer": "x", "number": "x", "boolean": "x", "array": "x", "object": "x"}.get(
                    sch["properties"][k].get("type"), 1) for k in req}
                self.call(name, wrong, "bad", "error", label=f"{name} (wrong types)")

    def sweep(self):
        for name in sorted(self.listed - self.called_ok):
            if name in ("dispatch_close",):
                continue
            self.call(name, sample(self.tools_schema[name]), "auto", "either", note="auto-generated args")

    def protocol_probes(self):
        for label, kw, exp in [
            ("malformed json", dict(raw="{not json"), "error"),
            ("unknown method", dict(method="no/such/method"), "error"),
            ("ping", dict(method="ping"), "ok"),
            ("prompts/list", dict(method="prompts/list"), "ok"),
            ("tools/call unknown tool", dict(method="tools/call", params={"name": "no_such_tool", "arguments": {}}), "error"),
            ("tools/call missing name", dict(method="tools/call", params={}), "error"),
            ("empty batch", dict(raw="[]"), "error"),
            ("oversize line (9 MB)", dict(raw='{"jsonrpc":"2.0","id":1,"method":"ping","params":{"p":"' + "a" * 9_000_000 + '"}}', timeout=60), "error"),
            ("prompts/get genius", dict(method="prompts/get", params={"name": "genius", "arguments": {"idea": "why is it slow"}}), "ok" if self.mode == "house" else "error"),
        ]:
            raw = kw.pop("raw", None)
            r = self.srv.rpc(kw.pop("method", ""), kw.pop("params", None), raw=raw, **kw)
            if label.startswith("oversize"):
                r = (r[0], r[1], r[2], r[3], "<9 MB line>")
            self.record("(protocol)", label, "protocol", exp, r, {})

    def finish(self):
        self.srv.close()


def sample(sch):
    t = sch.get("type")
    if "enum" in sch:
        return sch["enum"][0]
    if t == "object":
        return {k: sample(sch["properties"][k]) for k in sch.get("required", []) if k in sch.get("properties", {})}
    if t == "array":
        return [sample(sch["items"])] if "items" in sch else []
    return {"string": "audit", "integer": max(sch.get("minimum", 0), 1), "number": 0.5, "boolean": False}.get(t, "audit")


def S(tool, args=None, cap=None, expect="ok", must=None):
    return (tool, args if args is not None else {}, cap, expect, must)


def first_id(x):
    return x[0]["id"] if isinstance(x[0], dict) else x[0]


def vertex_claims(v, rnd):
    if rnd == 1:
        return [{"id": f"{v}.c1", "kind": "conjecture", "text": f"{v} conjecture", "falsifier": "f", "test": "t", "cost": "S", "confidence": 0.4},
                {"id": f"{v}.c2", "kind": "wild", "text": f"{v} wild", "falsifier": "f", "test": "t", "cost": "M", "confidence": 0.3},
                {"id": f"{v}.c3", "kind": "observation", "text": f"{v} saw", "evidence": ["a.rs:1"]}]
    prev = VERTS[VERTS.index(v) - 1]
    return [{"id": f"{v}.d1", "kind": "observation", "text": "reply", "evidence": ["b.rs:2"], "targets": [f"{prev}.c1"], "stance": "support"}]


def main_steps(corpus):
    P = lambda st: st.get("plan", "missing-plan")
    ch = lambda st: st.get("chunk", 1)
    orbit = {"relation": [{"entity": "a", "answer": "1"}, {"entity": "b", "answer": "1"}, {"entity": "c", "answer": "2"}], "injective": True, "claimed_errors": 0}
    leaf = lambda i, par: {"id": i, "parent": par, "text": f"do {i}", "leaf": True, "tier": "sonnet", "class": "code",
                           "test": {"name": f"t_{i}", "cmd": f"cargo test -p x t_{i}", "class": "test_unit"}, "est": {"ram_mb": 800, "secs": 60.0, "tokens": 10000}}
    foreman = {"fellow": "Foreman", "chart": {"goals": [
        {"id": "G1", "parent": "G0", "text": "part one", "leaf": False}, leaf("G1.1", "G1"), leaf("G1.2", "G1"),
        {"id": "G2", "parent": "G0", "text": "part two", "leaf": False}, leaf("G2.1", "G2"), leaf("G2.2", "G2")]}}
    s = [
        S("get_neo4j_schema"),
        S("write_neo4j_cypher", {"query": "CREATE (a:Person {name:'Ann'})-[:KNOWS]->(b:Person {name:'Bob'})"}),
        S("read_neo4j_cypher", {"query": "MATCH (p:Person) RETURN p.name AS name ORDER BY name"}),
        S("read_neo4j_cypher", {"query": "MATCH (p:Person {name:$n}) RETURN p.name AS name", "params": {"n": "Ann"}, "format": "compact"}),
        S("create_entities", {"entities": [{"name": "AuditEnt", "type": "Thing", "observations": ["alpha obs"]}, {"name": "AuditEnt2", "type": "Thing"}]}),
        S("create_relations", {"relations": [{"source": "AuditEnt", "target": "AuditEnt2", "relationType": "LINKS"}]}),
        S("add_observations", {"observations": [{"entityName": "AuditEnt", "observations": ["beta obs"]}]}),
        S("read_graph"), S("search_memories", {"query": "alpha"}), S("find_memories_by_name", {"names": ["AuditEnt"]}),
        S("delete_observations", {"deletions": [{"entityName": "AuditEnt", "observations": ["beta obs"]}]}),
        S("delete_relations", {"relations": [{"source": "AuditEnt", "target": "AuditEnt2", "relationType": "LINKS"}]}),
        S("delete_entities", {"entityNames": ["AuditEnt", "AuditEnt2"]}),
        S("rag_index", {"path": str(corpus)}),
        S("rag_search", {"query": "fibonacci number", "k": 5}, lambda st, j, t: st.update(chunk=j["hits"][0]["chunk_id"], chunks=[h["chunk_id"] for h in j["hits"]])),
        S("rag_search", {"query": "ledger cache", "format": "compact", "fresh": True}),
        S("rag_expand", lambda st: {"chunk_id": ch(st), "level": 0}),
        S("rag_expand", lambda st: {"chunk_id": ch(st), "level": 3}),
        S("rag_feedback", lambda st: {"used": [ch(st)], "unused": st.get("chunks", [ch(st)])[1:2]}),
        S("seal_remember", lambda st: {"text": "audit finding: fibonacci lives in math.rs", "tags": ["audit"], "sources": [ch(st)]}),
        S("seal_recall", {"query": "audit finding fibonacci"}),
        S("session_reset"), S("soul", {"budget_tokens": 800}),
        S("view", {"question": "how is fibonacci computed", "budget_tokens": 600}),
        S("view", {"question": "tokenizer and cache", "budget_tokens": 400, "islands": True}),
        S("calibrate", {"budget_secs": 0.5}), S("tier_stats"), S("why_hot", {"node": 0}), S("pin", {"node": 0}), S("unpin", {"node": 0}),
        S("replay", {"events": [{"op": "read", "region": 0}, {"op": "write", "region": 1}, {"op": "scan", "region": 2}, {"op": "pin", "region": 0}]}),
        S("fleet_admit", {"run": "audit-run", "count": 2, "role": "lens", "tier": "small"}, lambda st, j, t: st.update(agent=first_id(j["granted"]))),
        S("fleet_event", lambda st: {"run": "audit-run", "agent": st.get("agent", "a1"), "kind": "llm", "name": "haiku", "tokens_in": 100, "tokens_out": 50, "attrs": {"k": "v"}}),
        S("fleet_event", lambda st: {"run": "audit-run", "agent": st.get("agent", "a1"), "kind": "end", "status": "done", "note": "ok"}),
        S("fleet_event", {"run": "audit-run", "kind": "reap"}),
        S("fleet_trace", {"run": "audit-run"}), S("fleet_trace", {"run": "audit-run", "format": "jsonl"}), S("fleet_trace", {"run": "audit-run", "format": "deck"}),
        S("ledger_add", lambda st: {"ledger": st["ledger"], "goals": [{"id": "a", "title": "first"}, {"id": "b", "title": "second", "needs": ["a"]}]}),
        S("ledger_next", lambda st: {"ledger": st["ledger"], "limit": 1}),
        S("ledger_done", lambda st: {"ledger": st["ledger"], "id": "a", "outcome": "done", "result": "ok"}),
        S("ledger_next", lambda st: {"ledger": st["ledger"]}),
        S("ledger_done", lambda st: {"ledger": st["ledger"], "id": "b", "outcome": "failed", "result": "no"}),
        S("usage"), S("tools_enable", {"group": "rag"}), S("more", {"h": 999999}, expect="error", must="no stored remainder"),
        S("house_config"),
        S("house_need", {"project": "audit", "goal": "audit the server", "acceptance": ["every tool called"]}),
        S("house_chart", foreman), S("house_merge"), S("house_status"), S("house_next"),
        S("house_test_run", {"test": "t_G1.1", "status": "red", "secs": 1.5, "peak_ram_mb": 100}),
        S("house_test_run", {"test": "t_G1.1", "status": "green", "secs": 1.0, "peak_ram_mb": 100}),
        S("house_report", {"goal": "G1.1", "status": "reached", "evidence": {"test": "t_G1.1", "status": "green", "output": "ok"}}),
        S("house_test_run", {"test": "t_G2.1", "status": "red", "secs": 1.0, "peak_ram_mb": 50, "covered_files": ["src/a.rs"]}),
        S("house_select_tests", {"files": ["src/a.rs"]}),
        S("house_fact", {"fact": {"source": "audit:1", "text": "audit fact"}}),
        S("house_leap", {"goal": "G1.2", "cause": "audit"}, expect="either"),
        S("research_orbit_audit", orbit),
        S("research_pooling_audit", {"relation": orbit["relation"], "recoveries": [{"entity": "a", "recovery": 0.9}]}),
        S("research_record", {"tool": "research_orbit_audit", "arguments": orbit, "goal": "G0"}),
        S("genius_plan", {"idea": "why is the audit slow"}, lambda st, j, t: st.update(plan=j["plan"])),
        S("genius_brief", lambda st: {"plan": P(st), "vertex": "abduce"}),
    ]
    s += [S("genius_submit", (lambda v: lambda st: {"plan": P(st), "round": 1, "vertex": v, "claims": vertex_claims(v, 1), "unknowns": ["u"]})(v)) for v in VERTS]
    s += [S("genius_submit", (lambda v: lambda st: {"plan": P(st), "round": 2, "vertex": v, "claims": vertex_claims(v, 2)})(v)) for v in VERTS]
    s += [S("genius_submit", lambda st: {"plan": P(st), "experiment": [{"claim": "abduce.c1", "outcome": "survived", "evidence": ["x"]}]}, expect="either"),
          S("dispatch_open", {"duration": "30m", "workers": 2}),
          S("dispatch_plan", {"plan": {"departments": [{"name": "build", "goal": "g", "subgoals": [{"goal": "s", "leaves": [{"goal": "l", "checklist": ["a", "b"]}]}]}]}}),
          S("dispatch_check", {"leaf": "build.1.1", "index": 0}),
          S("dispatch_add", {"leaf": "build.1.1"}, lambda st, j, t: st.update(ticket=j["ticket"])),
          S("dispatch_add", {"scope": "audit scope: one line change and its test"}),
          S("dispatch_status"),
          S("dispatch_report", lambda st: {"ticket": st.get("ticket", 1), "kind": "ready", "note": "branch ok"}),
          S("dispatch_close")]
    return s


def extra_steps(corpus):
    more_h = lambda st, j, t: st.update(h=int(re.search(r"more\(h=(\d+)\)", t).group(1)))
    return [S("rag_index", {"path": str(corpus)}), S("terse_stats"),
            S("rag_search", {"query": "fibonacci tokenizer ledger cache", "k": 30, "budget_tokens": 6000, "format": "compact"}, more_h),
            S("more", lambda st: {"h": st.get("h", 1)}, expect="either"), S("usage"), S("tools_enable", {"group": "place"}), S("calibrate", {"budget_secs": 0.5}),
            S("tools_enable", {"group": "bogus"}, expect="error")]


def make_env(tmp):
    env = {k: v for k, v in os.environ.items() if not k.startswith("URIEL_")}
    home = tmp / "home"
    home.mkdir(exist_ok=True)
    env.update(USERPROFILE=str(home), HOME=str(home), RUST_BACKTRACE="1")
    return env


def run_phase(name, binary, tmp, env, argv, steps, bad=False, protocol=False, db="audit.uriel"):
    root = tmp / "root"
    root.mkdir(exist_ok=True)
    s = Session(name, binary, ["--db", tmp / db, "--root", root, "--tools", "full", "--format", "json", *argv], root, env)
    print(f"[{name}] {len(s.listed)} tools listed", flush=True)
    for st in steps:
        s.step(st)
    if protocol:
        s.protocol_probes()
    s.sweep()
    if bad:
        s.bad_calls()
    s.finish()
    return s


def reopen_house_db(binary, tmp, env):
    root = tmp / "root"
    s = Session("reopen-house-db", binary, ["--db", tmp / "audit-house.uriel", "--root", root, "--tools", "full", "--format", "json", "--mode", "house"], root, env)
    for st in [S("house_status"), S("rag_search", {"query": "fibonacci number"}), S("rag_index", {"path": str(tmp / "corpus")}), S("soul", {"budget_tokens": 400})]:
        s.step(st)
    s.finish()


def lock_test(binary, tmp, env):
    root = tmp / "root"
    out = EXTRA["lock"]
    for label, idle, wait in [("holder keeps db (--idle-release-secs 0)", "0", 0), ("holder idle-releases after 2 s, second starts after 3.5 s", "2", 3.5)]:
        db = tmp / f"lock-{idle}.uriel"
        argv = ["--db", db, "--root", root, "--tools", "full", "--format", "json"]
        a = Server(binary, argv + ["--idle-release-secs", idle], root, env)
        a.init()
        r1 = a.rpc("tools/call", {"name": "get_neo4j_schema", "arguments": {}})
        time.sleep(wait)
        b = Server(binary, argv, root, env)
        b.init()
        r2 = b.rpc("tools/call", {"name": "get_neo4j_schema", "arguments": {}})
        r3 = a.rpc("tools/call", {"name": "get_neo4j_schema", "arguments": {}})
        held = idle == "0"
        out.append({"scenario": label, "pass": (r2[0] == "typed-error" and "locked" in text_of(r2[1]) and r3[0] == "ok") if held else (r2[0] == "ok" and r3[0] == "typed-error"),
                    "expected": "second server refused with a 'locked' error, first unaffected" if held else "second server opens the released db; first is refused until it is released", "first_call": r1[0], "second_server_call": r2[0], "second_ms": r2[2],
                    "second_text": short(text_of(r2[1]) if r2[1] else "", 300), "first_after": r3[0], "first_after_ms": r3[2],
                    "second_stderr": b.stderr()[-200:]})
        print(f"[lock] {label}: second={r2[0]} in {r2[2]} ms", flush=True)
        a.close()
        b.close()


def mem16_test(binary, tmp, env, corpus):
    root = tmp / "root"
    s = Session("mem16", binary, ["--db", tmp / "mem16.uriel", "--root", root, "--tools", "full", "--format", "json", "--memory-mib", "16"], root, env)
    for st in main_steps(corpus)[:20]:
        if st[0] in ("rag_index", "rag_search", "write_neo4j_cypher", "read_neo4j_cypher", "rag_expand"):
            s.step(st)
    s.finish()


def list_sizes(binary, tmp, env):
    for mode in ("normal", "house"):
        for tools in ("adaptive", "full"):
            for ro in (False, True):
                argv = ["--db", tmp / "listsize.uriel", "--mode", mode, "--tools", tools] + (["--read-only"] if ro else [])
                srv = Server(binary, argv, tmp, env)
                srv.init()
                r = srv.rpc("tools/list")
                names = [t["name"] for t in (r[1] or {}).get("result", {}).get("tools", [])]
                EXTRA["list_sizes"].append({"phase": "listsize", "mode": mode, "tools_flag": tools, "read_only": ro, "tools": len(names), "bytes": r[3], "names": names})
                srv.close()


def cli_audit(binary, tmp, env, corpus):
    mcp_bin = Path(binary).with_name(Path(binary).name.replace("uriel", "uriel-mcp"))
    db = tmp / "cli.uriel"
    cases = [("--help", [], 0), ("--version", ["--version"], 0), ("banner", ["banner"], 0), ("(no args)", [], 0),
             ("doctor", ["doctor", "--db", db], 0), ("index", ["index", corpus, "--db", db], 0), ("index (again, incremental)", ["index", corpus, "--db", db], 0),
             ("recall", ["recall", "--db", db, "fibonacci", "number"], 0), ("recall --expand 1", ["recall", "--db", db, "--expand", "1"], None),
             ("calibrate --secs 1", ["calibrate", "--secs", "1"], 0), ("genius --json", ["genius", "why is the audit slow", "--json"], 0),
             ("genius --print-command", ["genius", "--print-command"], 0),
             ("afk --help", ["afk", "--help"], 0), ("page --help", ["page", "--help"], 0), ("capture --help", ["capture", "--help"], 0),
             ("page demo", ["page", "demo", "--out", tmp / "demo.html"], 0), ("afk status (no session)", ["afk", "status", "--file", tmp / "none.jsonl"], None),
             ("bogus subcommand", ["bogus"], "nonzero"), ("index missing dir", ["index", tmp / "nope", "--db", db], "nonzero"),
             ("genius without idea", ["genius"], "nonzero"), ("mcp --mode warp", ["mcp", "--mode", "warp"], "nonzero"),
             ("recall without query", ["recall", "--db", db], "nonzero"), ("afk without subcommand", ["afk"], "nonzero"),
             ("capture hook, empty stdin", ["capture", "--db", db], 0), ("recall-context hook, {} stdin", ["recall-context", "--db", db], 0)]
    jobs = [(n, [str(binary), *map(str, a)], e) for n, a, e in cases]
    if mcp_bin.exists():
        jobs += [("uriel-mcp --version", [str(mcp_bin), "--version"], 0), ("uriel-mcp --help", [str(mcp_bin), "--help"], 0), ("uriel-mcp --bogus", [str(mcp_bin), "--bogus"], "nonzero")]
    for name, cmd, exp in jobs:
        t0 = time.perf_counter()
        try:
            p = subprocess.run(cmd, input=b"{}" if "recall-context" in name else b"", capture_output=True, cwd=tmp, env=env, timeout=120)
            rc, out, err = p.returncode, p.stdout.decode("utf-8", "replace"), p.stderr.decode("utf-8", "replace")
        except subprocess.TimeoutExpired:
            rc, out, err = "timeout", "", ""
        ok = True if exp is None else (rc != 0 and rc != "timeout" if exp == "nonzero" else rc == exp)
        EXTRA["cli"].append({"cmd": name, "argv": short(" ".join(cmd[1:]), 200), "rc": rc, "expected": "any" if exp is None else exp, "pass": ok,
                             "secs": round(time.perf_counter() - t0, 2), "stdout": short(out.strip().splitlines()[-1] if out.strip() else "", 160), "stderr": short(err.strip(), 300)})


def esc(x):
    return str(x).replace("|", "\\|").replace("\n", " ")


def write_report(outdir, binary, version):
    fails = [r for r in RECORDS if not r["pass"]]
    cli_fail = [c for c in EXTRA["cli"] if not c["pass"]]
    lock_note = [l for l in EXTRA["lock"]]
    (outdir / "audit.json").write_text(json.dumps({"binary": str(binary), "version": version, "records": RECORDS, **EXTRA}, indent=1, ensure_ascii=False), encoding="utf-8")
    L = ["# uriel MCP audit", "", f"binary `{binary}` ({version}), {time.strftime('%Y-%m-%d %H:%M:%S')}", ""]
    counts = {}
    for r in RECORDS:
        counts[r["result"]] = counts.get(r["result"], 0) + 1
    L += ["## Summary", "", f"- calls: {len(RECORDS)}, failures: {len(fails)}, odd (passed but suspicious): {sum(r['odd'] for r in RECORDS)}",
          "- results: " + ", ".join(f"{k} {v}" for k, v in sorted(counts.items())), f"- CLI cases: {len(EXTRA['cli'])}, failures: {len(cli_fail)}", ""]
    L += ["## Failures", ""]
    if not fails and not cli_fail:
        L += ["none", ""]
    for r in fails:
        L += [f"### [{r['phase']}] {r['label']} -> {r['result']} (expected {r['expect']})", "", f"note: {r['note']}", "", "request:", "```", r["request"], "```", "response:", "```", r["response"], "```", ""]
    for c in cli_fail:
        L += [f"### CLI `{c['cmd']}` rc={c['rc']} (expected {c['expected']})", "", "```", c["argv"], c["stderr"], "```", ""]
    odd = [r for r in RECORDS if r["odd"]]
    L += ["## Odd (passed, but worth a look)", ""]
    L += [f"- [{r['phase']}] {r['label']}: {r['result']}, {r['latency_ms']} ms. {r['note']} response: `{short(r['response'], 220)}`" for r in odd] or ["none"]
    L += ["", "## Lock behaviour (second server on a held db)", "", "| scenario | expected | pass | first call | second call | second ms | first after | second text |", "|---|---|---|---|---|---|---|---|"]
    L += [f"| {esc(l['scenario'])} | {esc(l['expected'])} | {'yes' if l['pass'] else 'NO'} | {l['first_call']} | {l['second_server_call']} | {l['second_ms']} | {l['first_after']} | {esc(l['second_text'])} |" for l in lock_note]
    L += ["", "## tools/list size", "", "| phase | mode | tools flag | read-only | tools | bytes |", "|---|---|---|---|---|---|"]
    L += [f"| {s['phase']} | {s['mode']} | {s.get('tools_flag', 'full')} | {s['read_only']} | {s['tools']} | {s['bytes']} |" for s in EXTRA["list_sizes"]]
    L += ["", "## CLI", "", "| case | rc | expected | secs | pass | stderr / last stdout line |", "|---|---|---|---|---|---|"]
    L += [f"| {esc(c['cmd'])} | {c['rc']} | {c['expected']} | {c['secs']} | {'yes' if c['pass'] else 'NO'} | {esc(c['stderr'] or c['stdout'])} |" for c in EXTRA["cli"]]
    for phase in dict.fromkeys(r["phase"] for r in RECORDS):
        rs = [r for r in RECORDS if r["phase"] == phase]
        L += ["", f"## Phase: {phase} (mode {rs[0]['mode']}, read-only {rs[0]['read_only']})", "", "| tool | kind | result | pass | ms | bytes | peak RSS MB | notes |", "|---|---|---|---|---|---|---|---|"]
        L += [f"| {esc(r['label'])} | {r['kind']} | {r['result']} | {'yes' if r['pass'] else 'NO'} | {r['latency_ms']} | {r['resp_bytes']} | {r['peak_rss_mb']} | {esc(r['note'] or short(r['response'], 90))} |" for r in rs]
    (outdir / "AUDIT.md").write_text("\n".join(L) + "\n", encoding="utf-8")
    return len(fails) + len(cli_fail) + sum(1 for l in lock_note if not l["pass"])


def main():
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(errors="replace")
    here = Path(__file__).resolve().parent.parent
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--binary", default=str(here / "target" / "release" / ("uriel.exe" if os.name == "nt" else "uriel")))
    ap.add_argument("--outdir", default=str(here / "audit-out"))
    ap.add_argument("--keep", action="store_true", help="keep the temp dir")
    ap.add_argument("--wait-minutes", type=float, default=0, help="poll for the binary this long")
    a = ap.parse_args()
    binary, deadline = Path(a.binary), time.time() + a.wait_minutes * 60
    while True:
        try:
            ver = subprocess.run([str(binary), "--version"], capture_output=True, timeout=30).stdout.decode("utf-8", "replace").strip().splitlines()[-1]
            break
        except (OSError, IndexError, subprocess.TimeoutExpired):
            if time.time() > deadline:
                sys.exit(f"binary not usable: {binary}")
            time.sleep(30)
    tmp = Path(tempfile.mkdtemp(prefix="uriel-audit-"))
    outdir = Path(a.outdir)
    outdir.mkdir(parents=True, exist_ok=True)
    for f in (binary, binary.with_name(binary.name.replace("uriel", "uriel-mcp"))):
        if f.exists():
            shutil.copy2(f, tmp / f.name)
    binary = tmp / binary.name
    corpus = tmp / "corpus"
    corpus.mkdir()
    for n, c in CORPUS.items():
        (corpus / n).write_text(c, encoding="utf-8")
    env = make_env(tmp)
    steps = main_steps(corpus)
    try:
        run_phase("normal", binary, tmp, env, ["--mode", "normal"], steps, bad=True, protocol=True)
        run_phase("house", binary, tmp, env, ["--mode", "house"], steps, bad=True, protocol=True, db="audit-house.uriel")
        reopen_house_db(binary, tmp, env)
        run_phase("ro-normal", binary, tmp, env, ["--mode", "normal", "--read-only"], steps)
        run_phase("ro-house", binary, tmp, env, ["--mode", "house", "--read-only"], steps)
        run_phase("terse+token-limit", binary, tmp, env, ["--mode", "normal", "--terse", "data", "--token-limit", "150"], extra_steps(corpus))
        mem16_test(binary, tmp, env, corpus)
        lock_test(binary, tmp, env)
        list_sizes(binary, tmp, env)
        cli_audit(binary, tmp, env, corpus)
    finally:
        bad = write_report(outdir, a.binary, ver)
        if not a.keep:
            shutil.rmtree(tmp, ignore_errors=True)
    print(f"calls {len(RECORDS)}, failures {bad}; report in {outdir}")
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
