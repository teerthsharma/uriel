import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time

import tiktoken

ENC = tiktoken.get_encoding("cl100k_base")
LABEL = "cl100k proxy"
BAR = 3.0
HERE = os.path.dirname(os.path.abspath(__file__))


def toks(text):
    return len(ENC.encode(text, disallowed_special=()))


def nbytes(text):
    return len(text.encode("utf-8"))


class Server:
    def __init__(self, binary, root):
        self.proc = subprocess.Popen(
            [binary, "--in-memory", "--root", root, "--token-limit", "10000000"],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            text=True,
            encoding="utf-8",
            bufsize=1,
        )
        self.next_id = 1
        self.rpc(
            "initialize",
            {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "token-eval", "version": "1"}},
        )
        self.proc.stdin.write(json.dumps({"jsonrpc": "2.0", "method": "notifications/initialized"}) + "\n")
        self.proc.stdin.flush()

    def rpc(self, method, params):
        rid = self.next_id
        self.next_id += 1
        self.proc.stdin.write(json.dumps({"jsonrpc": "2.0", "id": rid, "method": method, "params": params}) + "\n")
        self.proc.stdin.flush()
        line = self.proc.stdout.readline()
        if not line:
            raise RuntimeError("server closed stdout")
        resp = json.loads(line)
        assert resp["id"] == rid, resp
        return resp

    def call(self, name, args):
        resp = self.rpc("tools/call", {"name": name, "arguments": args})
        res = resp["result"]
        text = res["content"][0]["text"]
        if res.get("isError"):
            raise RuntimeError(f"{name} failed: {text}")
        return text

    def close(self):
        try:
            self.proc.stdin.close()
            self.proc.wait(timeout=20)
        except Exception:
            self.proc.kill()



FN_RE = re.compile(r"^\s*(pub(?:\([^)]*\))?\s+)?(?:const\s+)?(?:async\s+)?(?:unsafe\s+)?fn\s+(\w+)")


def source_files(root):
    out = []
    for d, dirs, files in os.walk(root):
        dirs[:] = sorted(x for x in dirs if x not in ("target", ".git", "node_modules"))
        for f in sorted(files):
            if f.endswith(".rs"):
                out.append(os.path.join(d, f))
    return out


def load_graph(server, root):
    files = []
    fns = {}
    for path in source_files(root):
        rel = os.path.relpath(path, root).replace("\\", "/")
        with open(path, encoding="utf-8", errors="replace") as fh:
            lines = fh.read().split("\n")
        files.append({"path": rel, "crate": rel.split("/")[0], "lines": len(lines)})
        found = []
        for i, line in enumerate(lines, 1):
            m = FN_RE.match(line)
            if m:
                found.append({"name": m.group(2), "line": i, "vis": "pub" if m.group(1) else "priv"})
        fns[rel] = found
    for i in range(0, len(files), 100):
        server.call(
            "write_neo4j_cypher",
            {
                "query": "UNWIND $rows AS r CREATE (:File {path: r.path, crate: r.crate, lines: r.lines})",
                "params": {"rows": files[i : i + 100]},
            },
        )
    for rel, found in fns.items():
        if not found:
            continue
        server.call(
            "write_neo4j_cypher",
            {
                "query": "MATCH (f:File {path: $p}) UNWIND $fns AS x CREATE (:Fn {name: x.name, file: $p, line: x.line, vis: x.vis})-[:IN]->(f)",
                "params": {"p": rel, "fns": found},
            },
        )
    return len(files), sum(len(v) for v in fns.values())



UNESC = {"\\": "\\", "t": "\t", "n": "\n", "r": "\r"}


def unescape(s):
    out = []
    i = 0
    while i < len(s):
        c = s[i]
        if c == "\\" and i + 1 < len(s):
            out.append(UNESC.get(s[i + 1], s[i + 1]))
            i += 2
        else:
            out.append(c)
            i += 1
    return "".join(out)


def read_aliases(lines, pos):
    aliases = {}
    while pos < len(lines):
        m = re.match(r"^@([A-Za-z])=(.*)$", lines[pos])
        if not m:
            break
        aliases[m.group(1)] = unescape(m.group(2))
        pos += 1
    return aliases, pos


def expand_alias(field, aliases):
    if field.startswith("@"):
        rest = field[2:]
        return aliases[field[1]] + (unescape(rest[1:]) if rest.startswith(" ") else "")
    if field.startswith("'"):
        return unescape(field[1:])
    return unescape(field)


V1_CITE = re.compile(r"^// (.*):(\d+)-(\d+) #(\d+)$", re.M)
V1_CITE_INLINE = re.compile(r"^// (.*?):(\d+)-(\d+) #(\d+)(?: |$)", re.M)


def parse_v1_pack(text):
    obj = json.loads(text)
    cites = {}
    for m in V1_CITE_INLINE.finditer(obj["text"]):
        cites[int(m.group(4))] = (m.group(1), int(m.group(2)), int(m.group(3)), int(m.group(4)))
    return {"ids": [h["chunk_id"] for h in obj["hits"]], "cites": cites, "refs": set(), "omitted": obj["omitted"]}


PATH_UNESC = dict(UNESC, s=" ")


def unescape_path(s):
    out = []
    i = 0
    while i < len(s):
        if s[i] == "\\" and i + 1 < len(s):
            out.append(PATH_UNESC[s[i + 1]])
            i += 2
        else:
            out.append(s[i])
            i += 1
    return "".join(out)


def pack_file(field, aliases):
    if field.startswith("@"):
        rest = field[2:]
        assert rest == "" or rest.startswith(" "), f"bad alias use {field!r}"
        return aliases[field[1]] + unescape_path(rest[1:])
    if field.startswith("'"):
        return unescape_path(field[1:])
    return unescape_path(field)


def pack_range(text, prev_end):
    m = re.match(r"^([+-]?)(\d+)(?:([+-])(\d+))?$", text)
    assert m, f"bad range {text!r}"
    sign, n = m.group(1), int(m.group(2))
    start = n if not sign else (prev_end + n if sign == "+" else prev_end - n)
    if m.group(3) is None:
        return start, start
    return start, (int(m.group(4)) if m.group(3) == "-" else start + int(m.group(4)))


def split_header(line):
    skip = 3 if len(line) >= 3 and line[0] == "@" and line[2] == " " else 0
    j = line.find(" ", skip)
    return (line, None) if j < 0 else (line[:j], line[j + 1 :])


def parse_v2_pack(text):
    lines = text.split("\n")
    if lines and lines[-1] == "":
        lines.pop()
    head = re.match(r"^#(\d+)(?: omit=(\d+))?$", lines[0])
    assert head, f"bad pack header {lines[0]!r}"
    count = int(head.group(1))
    aliases, pos = read_aliases(lines, 1)
    cites, ids, refs, levels = {}, [], set(), []
    prev_file, last_end = None, {}
    for _ in range(count):
        line = lines[pos]
        pos += 1
        assert line.startswith(">"), f"expected a citation line, got {line!r}"
        header, inline = split_header(line[1:])
        loc, _, tail = header.rpartition("#")
        m = re.match(r"^(\d+)(.*)$", tail)
        assert m, f"bad citation {line!r}"
        cid, flags = int(m.group(1)), m.group(2)
        ids.append(cid)
        if loc == "":
            assert "r" in flags, f"bare citation without r {line!r}"
            refs.add(cid)
            continue
        if ":" in loc:
            ftext, _, rtext = loc.rpartition(":")
            f = pack_file(ftext, aliases)
        else:
            f, rtext = prev_file, loc
        s, e = pack_range(rtext, last_end.get(f))
        cites[cid] = (f, s, e, cid)
        prev_file, last_end[f] = f, e
        lm = re.search(r"L(\d)", flags)
        levels.append(int(lm.group(1)) if lm else 3)
        km = re.search(r"\+(\d+)", flags)
        if inline is None:
            assert km, f"hit without text {line!r}"
            pos += int(km.group(1))
    assert pos == len(lines), f"trailing lines in pack at {pos}/{len(lines)}"
    return {"ids": ids, "cites": cites, "refs": refs, "omitted": int(head.group(2) or 0), "levels": levels}


def read_dictionary(lines, pos, aliases):
    dict_values = []
    while pos < len(lines):
        m = re.match(r"^\$([A-Za-z])=(.*)$", lines[pos])
        if not m:
            break
        dict_values.append(decode_cell(m.group(2), aliases, []))
        pos += 1
    return dict_values, pos


def dict_index(name):
    return ord(name) - 97 if name.islower() else 26 + ord(name) - 65


def decode_cell(field, aliases, dict_values=None):
    if field == "":
        return None
    if field == "true":
        return True
    if field == "false":
        return False
    if dict_values is not None and field.startswith("$"):
        assert len(field) == 2, f"bad dictionary reference {field!r}"
        return dict_values[dict_index(field[1])]
    c = field[0]
    if c == "'":
        return unescape(field[1:])
    if c == "@":
        return expand_alias(field, aliases)
    if c == "~":
        return json.loads(field[1:])
    if c.isdigit() or c in "-+.":
        try:
            return int(field)
        except ValueError:
            return float(field)
    return unescape(field)


def decode_table(text):
    lines = text.split("\n")
    m = re.match(r"^#(\d+)( \+)?$", lines[0])
    assert m, f"bad table header {lines[0]!r}"
    n = int(m.group(1))
    flagged = m.group(2) is not None
    if n == 0 and len(lines) == 1:
        return []
    aliases, pos = read_aliases(lines, 1)
    dict_values = None
    if flagged:
        dict_values, pos = read_dictionary(lines, pos, aliases)
    cols = [expand_alias(f, {}) if not f.startswith("@") else f for f in lines[pos].split("\t")]
    pos += 1
    rows = []
    previous = None
    for i in range(n):
        cells = []
        if lines[pos + i]:
            for f in lines[pos + i].split("\t"):
                if flagged and f == "^":
                    cells.append(previous[len(cells)])
                else:
                    cells.append(decode_cell(f, aliases, dict_values))
        cells += [None] * (len(cols) - len(cells))
        previous = cells
        rows.append(dict(zip(cols, cells)))
    return rows




def ratio(a, b):
    return a / b if b else float("inf")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--bin", default=os.path.normpath(os.path.join(HERE, "../../../../target/release/uriel-mcp.exe")))
    ap.add_argument("--root", default=os.path.normpath(os.path.join(HERE, "../../..")))
    ap.add_argument("--out", default=os.path.join(HERE, "results.json"))
    args = ap.parse_args()
    queries = json.load(open(os.path.join(HERE, "queries.json"), encoding="utf-8"))

    stage = tempfile.mkdtemp(prefix="token-eval-")
    root = os.path.join(stage, "crates")
    shutil.copytree(args.root, root, ignore=shutil.ignore_patterns("target", ".git", "node_modules", "frozen_corpus"))
    srv = Server(args.bin, root)
    t0 = time.time()
    idx = json.loads(srv.call("rag_index", {}))
    nfiles, nfns = load_graph(srv, root)
    print(f"indexed {idx['files_indexed']} files / {idx['chunks_added']} chunks; graph {nfiles} File and {nfns} Fn nodes ({time.time() - t0:.1f}s)")

    problems = []
    rag_rows = []
    session_rows = []
    cyp_rows = []

    for q in queries["rag"]:
        t1 = srv.call("rag_search", {"query": q, "format": "json", "ladder": False})
        t2 = srv.call("rag_search", {"query": q, "format": "compact", "ladder": True, "fresh": True})
        p1, p2 = parse_v1_pack(t1), parse_v2_pack(t2)
        both = [i for i in p1["ids"] if i in p2["ids"]]
        missing = [p1["cites"][i] for i in both if p1["cites"].get(i) != p2["cites"].get(i)]
        rag_rows.append(
            {
                "q": q, "v1_tok": toks(t1), "v2_tok": toks(t2), "v1_b": nbytes(t1), "v2_b": nbytes(t2),
                "hits_v1": len(p1["ids"]), "hits_v2": len(p2["ids"]), "both": len(both),
                "only_v1": len([i for i in p1["ids"] if i not in p2["ids"]]),
                "only_v2": len([i for i in p2["ids"] if i not in p1["ids"]]),
                "cites_checked": len(both), "cites_missing": len(missing),
                "levels": p2.get("levels", []),
            }
        )
        if missing:
            problems.append(f"rag citation missing in v2 for: {q!r}: {missing}")

    srv.call("session_reset", {})
    seen = {}
    for q in queries["rag"]:
        t1 = srv.call("rag_search", {"query": q, "format": "json", "ladder": False})
        t3 = srv.call("rag_search", {"query": q, "format": "compact", "ladder": True})
        p1, p3 = parse_v1_pack(t1), parse_v2_pack(t3)
        bad = 0
        for i in p1["ids"]:
            if i not in p3["ids"]:
                continue
            if i in p3["refs"]:
                if seen.get(i) != p1["cites"][i]:
                    bad += 1
            elif p3["cites"].get(i) != p1["cites"][i]:
                bad += 1
        seen.update(p3["cites"])
        session_rows.append({"q": q, "v1_tok": toks(t1), "v3_tok": toks(t3), "v1_b": nbytes(t1), "v3_b": nbytes(t3),
                             "refs": len(p3["refs"]), "hits": len(p3["ids"]), "cites_missing": bad})
        if bad:
            problems.append(f"session-pass citation not preserved for: {q!r}")

    for q in queries["cypher"]:
        t1 = srv.call("read_neo4j_cypher", {"query": q, "format": "json"})
        t2 = srv.call("read_neo4j_cypher", {"query": q, "format": "compact"})
        rows1 = json.loads(t1)
        rows2 = decode_table(t2)
        same = rows1 == rows2
        if not same:
            problems.append(f"cypher v2 does not decode to the v1 rows: {q!r}")
        cyp_rows.append({"q": q, "rows": len(rows1), "v1_tok": toks(t1), "v2_tok": toks(t2), "v1_b": nbytes(t1),
                         "v2_b": nbytes(t2), "decodes_to_v1": same})

    exact_ok = exact_n = 0
    srv.call("session_reset", {})
    for q in queries["rag"]:
        p2 = parse_v2_pack(srv.call("rag_search", {"query": q, "format": "compact", "ladder": True, "fresh": True}))
        for cid in p2["ids"]:
            if cid not in p2["cites"]:
                continue
            f, s, e, _ = p2["cites"][cid]
            original = srv.call("rag_expand", {"chunk_id": cid})
            disk = open(os.path.join(root, f), encoding="utf-8", errors="replace").read().replace("\r\n", "\n").split("\n")
            want = "\n".join(disk[s - 1 : e]).rstrip("\n")
            exact_n += 1
            exact_ok += int(original.replace("\r\n", "\n").rstrip("\n") == want)
    srv.close()
    shutil.rmtree(stage, ignore_errors=True)

    def total(rows, a, b):
        ta, tb = sum(r[a] for r in rows), sum(r[b] for r in rows)
        return ta, tb, ratio(ta, tb)

    print(f"\nAll token counts are {LABEL} (tiktoken cl100k_base), not Claude tokens.\n")
    print(f"{'rag query':<64} {'v1 tok':>7} {'v2 tok':>7} {'ratio':>6} {'v1 B':>7} {'v2 B':>7}")
    for r in rag_rows:
        print(f"{r['q'][:63]:<64} {r['v1_tok']:>7} {r['v2_tok']:>7} {ratio(r['v1_tok'], r['v2_tok']):>6.2f} {r['v1_b']:>7} {r['v2_b']:>7}")
    a, b, rr = total(rag_rows, "v1_tok", "v2_tok")
    ab, bb, rb = total(rag_rows, "v1_b", "v2_b")
    print(f"{'RAG TOTAL (v2 = compact + ladder, no dedup)':<64} {a:>7} {b:>7} {rr:>6.2f} {ab:>7} {bb:>7}  bytes ratio {rb:.2f}")
    sa, sb, sr = total(session_rows, "v1_tok", "v3_tok")
    print(f"{'RAG TOTAL (v2 + session dedup, 10 questions in order)':<64} {sa:>7} {sb:>7} {sr:>6.2f}   refs {sum(r['refs'] for r in session_rows)}/{sum(r['hits'] for r in session_rows)} hits")

    print(f"\n{'cypher query (rows)':<64} {'v1 tok':>7} {'v2 tok':>7} {'ratio':>6} {'v1 B':>7} {'v2 B':>7}")
    for r in cyp_rows:
        label = f"{r['q'][:52]} ({r['rows']})"
        print(f"{label:<64} {r['v1_tok']:>7} {r['v2_tok']:>7} {ratio(r['v1_tok'], r['v2_tok']):>6.2f} {r['v1_b']:>7} {r['v2_b']:>7}")
    ca, cb, cr = total(cyp_rows, "v1_tok", "v2_tok")
    cab, cbb, crb = total(cyp_rows, "v1_b", "v2_b")
    print(f"{'CYPHER TOTAL':<64} {ca:>7} {cb:>7} {cr:>6.2f} {cab:>7} {cbb:>7}  bytes ratio {crb:.2f}")
    out_of_range = [r["rows"] for r in cyp_rows if not 20 <= r["rows"] <= 200]
    if out_of_range:
        print(f"NOTE: row counts outside 20..200 (not adjusted): {out_of_range}")

    allv1, allv2 = a + ca, b + cb
    overall = ratio(allv1, allv2)
    print(f"\nOVERALL v1 {allv1} -> v2 {allv2} {LABEL} tokens: {overall:.2f}x fewer; bytes {ab + cab} -> {bb + cbb}")
    checked = sum(r["cites_checked"] for r in rag_rows)
    missing = sum(r["cites_missing"] for r in rag_rows) + sum(r["cites_missing"] for r in session_rows)
    print(f"citations: {checked} v1 citations checked against v2 (chunks in both packs), {missing} missing incl. session pass")
    print(f"hits only in v1: {sum(r['only_v1'] for r in rag_rows)}, only in v2: {sum(r['only_v2'] for r in rag_rows)} (budget/ladder differences)")
    print(f"exact original via rag_expand(every cited hit): {exact_ok}/{exact_n} equal the file's own lines")
    print(f"cypher v2 decodes to the v1 rows: {sum(r['decodes_to_v1'] for r in cyp_rows)}/{len(cyp_rows)}")

    met = overall >= BAR and rr >= BAR and cr >= BAR and not problems
    verdict = "BAR MET" if met else "BAR MISSED"
    print(f"\n{verdict}: need >= {BAR}x in total and per part with every citation preserved "
          f"(rag {rr:.2f}x, cypher {cr:.2f}x, overall {overall:.2f}x)")
    for p in problems:
        print("PROBLEM:", p)
    json.dump(
        {"label": LABEL, "rag": rag_rows, "rag_session": session_rows, "cypher": cyp_rows,
         "totals": {"rag": [a, b, rr], "rag_session": [sa, sb, sr], "cypher": [ca, cb, cr], "overall": [allv1, allv2, overall]},
         "citations_checked": checked, "citations_missing": missing, "exact_original": [exact_ok, exact_n],
         "index": idx, "graph": [nfiles, nfns], "bar": BAR, "met": met},
        open(args.out, "w", encoding="utf-8"), indent=1,
    )
    return 0 if met else 1


if __name__ == "__main__":
    sys.exit(main())
