import os
import re

import uriel_client

BUDGET_TOKENS = 1 << 20
SIGNATURE_CHARS = 200
DEFINITION = re.compile(r"\b(fn|struct|enum|trait|impl|mod|class|def|function|interface|type)\s+([A-Za-z_][A-Za-z0-9_]*)")

_client = None


def _require():
    if _client is None:
        raise RuntimeError("init_clara has not been called")
    return _client


def close_clara():
    global _client
    if _client is not None:
        _client.close()
    _client = None


def init_clara(db_path):
    global _client
    close_clara()
    try:
        _client = uriel_client.Client(db_path)
    except (OSError, uriel_client.HostError):
        return False
    return True


def index_workspace(root):
    report = _require().index(root)
    return {"files_indexed": report["files_indexed"], "ast_nodes": report["chunks_added"]}


def _describe(hit, query):
    lines = [line.strip() for line in hit["memory"].splitlines() if line.strip()]
    found = []
    for line in lines:
        match = DEFINITION.search(line)
        if match:
            found.append((match.group(1), match.group(2), line))
    if found:
        wanted = query.lower()
        kind, name, line = next((f for f in found if f[1].lower() in wanted), found[0])
    else:
        kind, name, line = "block", os.path.basename(hit["file"]), lines[0] if lines else ""
    return {
        "file": hit["file"],
        "type": kind,
        "name": name,
        "signature": line[:SIGNATURE_CHARS],
        "start_line": hit["start_line"],
        "end_line": hit["end_line"],
    }


def query_context(q, limit=20):
    if limit <= 0:
        return []
    pack = _require().search(q, k=limit, budget_tokens=BUDGET_TOKENS)
    return [_describe(hit, q) for hit in pack["hits"][:limit]]


def get_stats():
    return _require().stats()
