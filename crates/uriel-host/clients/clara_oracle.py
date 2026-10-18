import os

import uriel_client

PREVIEW_CHARS = 300
FETCH_FACTOR = 3
BUDGET_TOKENS = 1 << 20


class ClaraOracle:
    def __init__(self, pipe_path=None, max_frame=1 << 20):
        self.client = uriel_client.Client(pipe_path or os.environ["URIEL_PIPE"], max_frame=max_frame)

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        self.close()

    def close(self):
        self.client.close()

    def search(self, query, k=3):
        if k <= 0:
            return []
        pack = self.client.search(query, k=k * FETCH_FACTOR, budget_tokens=BUDGET_TOKENS)
        seen = set()
        results = []
        for hit in pack["hits"]:
            if hit["file"] in seen:
                continue
            seen.add(hit["file"])
            score = 1.0 / (1 + len(results))
            results.append(
                {
                    "path": hit["file"],
                    "preview": hit["memory"][:PREVIEW_CHARS],
                    "score": score,
                    "distance": 1.0 - score,
                }
            )
            if len(results) == k:
                break
        return results

    def get_context_for_prompt(self, query, k=3, max_chars=600):
        text = "\n".join("%s: %s" % (item["path"], item["preview"]) for item in self.search(query, k))
        return text[: max(max_chars, 0)]

    def crawl(self, dir):
        report = self.client.index(dir)
        return report["files_indexed"] + report["files_unchanged"]

    def index_file(self, path, content):
        return self.client.index_file(path, content)["files_skipped"] == 0

    def stats(self):
        host = self.client.stats()
        return {
            "documents_indexed": host["files"],
            "vocabulary_size": host["embed_dim"],
            "vector_dimensions": host["embed_dim"],
        }
