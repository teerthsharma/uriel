import os
import struct
import sys
import threading
import traceback
import time
import unittest

sys.path.insert(0, os.environ["URIEL_CLIENTS"])

import clara_module
import uriel_client
from clara_oracle import ClaraOracle

PIPE = os.environ["URIEL_PIPE"]
ROOT = os.environ["URIEL_ROOT"]
OUTSIDE = os.environ["URIEL_OUTSIDE"]
QUERY = "rolling sum over a byte buffer"
ORACLE_KEYS = {"path", "preview", "score", "distance"}
MODULE_KEYS = {"file", "type", "name", "signature", "start_line", "end_line"}


def search_frame(query):
    body = bytes([uriel_client.MSG_SEARCH]) + uriel_client.put_string(query) + struct.pack("<QQQB", 3, 2000, 0, 1)
    return struct.pack("<I", len(body)) + body


class OracleShapes(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.oracle = ClaraOracle(PIPE)
        cls.crawled = cls.oracle.crawl(ROOT)

    @classmethod
    def tearDownClass(cls):
        cls.oracle.close()

    def test_crawl_returns_an_int_counting_the_documents_now_indexed(self):
        self.assertIs(type(self.crawled), int)
        self.assertEqual(self.crawled, len(os.listdir(ROOT)))
        self.assertEqual(self.oracle.crawl(ROOT), len(os.listdir(ROOT)))

    def test_crawl_refuses_a_directory_that_is_not_the_host_root(self):
        with self.assertRaises(uriel_client.RemoteError):
            self.oracle.crawl(OUTSIDE)

    def test_search_returns_a_list_of_dicts_with_exactly_the_four_clara_keys(self):
        results = self.oracle.search(QUERY)
        self.assertIs(type(results), list)
        self.assertTrue(results)
        self.assertLessEqual(len(results), 3)
        for item in results:
            self.assertIs(type(item), dict)
            self.assertEqual(set(item), ORACLE_KEYS)
            self.assertIs(type(item["path"]), str)
            self.assertIs(type(item["preview"]), str)
            self.assertIs(type(item["score"]), float)
            self.assertIs(type(item["distance"]), float)
        self.assertEqual(results[0]["path"], "a.rs")

    def test_preview_is_the_compressed_memory_truncated_to_300_chars(self):
        with uriel_client.Client(PIPE) as raw:
            pack = raw.search(QUERY, k=9, budget_tokens=100000)
        by_file = {}
        for hit in pack["hits"]:
            by_file.setdefault(hit["file"], hit["memory"])
        for item in self.oracle.search(QUERY, k=3):
            self.assertLessEqual(len(item["preview"]), 300)
            self.assertEqual(item["preview"], by_file[item["path"]][:300])

    def test_preview_is_cut_to_exactly_300_chars_when_the_memory_is_longer(self):
        class Stub:
            def search(self, query, k, budget_tokens):
                memory = "".join(chr(97 + i % 26) for i in range(1000))
                hit = {"file": "wide.txt", "memory": memory, "start_line": 1, "end_line": 2}
                return {"hits": [hit]}

        oracle = ClaraOracle.__new__(ClaraOracle)
        oracle.client = Stub()
        (item,) = oracle.search("anything", k=3)
        self.assertEqual(len(item["preview"]), 300)
        self.assertEqual(item["preview"], "".join(chr(97 + i % 26) for i in range(300)))

    def test_score_is_one_over_one_plus_the_fused_rank(self):
        results = self.oracle.search(QUERY, k=3)
        for rank, item in enumerate(results):
            self.assertAlmostEqual(item["score"], 1.0 / (1 + rank), places=12)

    def test_scores_are_strictly_decreasing_in_fused_rank_and_inside_zero_one(self):
        results = self.oracle.search("function", k=3)
        scores = [item["score"] for item in results]
        self.assertEqual(scores, sorted(scores, reverse=True))
        self.assertEqual(len(set(scores)), len(scores))
        for value in scores:
            self.assertTrue(0.0 < value <= 1.0)

    def test_distance_is_one_minus_score(self):
        for item in self.oracle.search(QUERY, k=3):
            self.assertAlmostEqual(item["distance"], 1.0 - item["score"], places=12)

    def test_search_returns_at_most_k_and_one_entry_per_path(self):
        for k in (1, 2, 3):
            results = self.oracle.search("function", k=k)
            self.assertLessEqual(len(results), k)
            paths = [item["path"] for item in results]
            self.assertEqual(len(paths), len(set(paths)))

    def test_get_context_for_prompt_returns_a_str_within_max_chars_naming_the_path(self):
        text = self.oracle.get_context_for_prompt(QUERY)
        self.assertIs(type(text), str)
        self.assertLessEqual(len(text), 600)
        self.assertIn("a.rs", text)
        for cap in (1, 50, 120):
            self.assertLessEqual(len(self.oracle.get_context_for_prompt(QUERY, k=3, max_chars=cap)), cap)
        self.assertEqual(self.oracle.get_context_for_prompt(QUERY, k=3, max_chars=0), "")

    def test_index_file_returns_true_and_the_content_becomes_searchable(self):
        result = self.oracle.index_file("scratch/ghost.rs", "pub fn quokka_pelican_shuffle() -> u32 { 41 }\n")
        self.assertIs(result, True)
        self.assertEqual(self.oracle.search("quokka_pelican_shuffle")[0]["path"], "scratch/ghost.rs")
        self.assertIs(self.oracle.index_file("scratch/ghost.rs", "pub fn quokka_pelican_shuffle() -> u32 { 41 }\n"), True)

    def test_index_file_returns_false_for_binary_content_and_raises_for_a_bad_path(self):
        self.assertIs(self.oracle.index_file("blob.bin", "abc\x00def"), False)
        with self.assertRaises(uriel_client.RemoteError):
            self.oracle.index_file("", "fn x() {}")

    def test_stats_has_exactly_the_three_clara_keys_as_ints_taken_from_the_host_stats(self):
        stats = self.oracle.stats()
        self.assertEqual(set(stats), {"documents_indexed", "vocabulary_size", "vector_dimensions"})
        for value in stats.values():
            self.assertIs(type(value), int)
        with uriel_client.Client(PIPE) as raw:
            host = raw.stats()
        self.assertEqual(stats["documents_indexed"], host["files"])
        self.assertEqual(stats["vector_dimensions"], host["embed_dim"])
        self.assertEqual(stats["vector_dimensions"], 256)

    def test_vocabulary_size_is_the_hash_bucket_count_of_the_embedder(self):
        stats = self.oracle.stats()
        self.assertEqual(stats["vocabulary_size"], stats["vector_dimensions"])


class ModuleShapes(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.initialised = clara_module.init_clara(PIPE)
        cls.workspace = clara_module.index_workspace(ROOT)

    @classmethod
    def tearDownClass(cls):
        clara_module.close_clara()

    def test_init_clara_returns_true_for_a_live_host_and_false_for_a_dead_path(self):
        self.assertIs(self.initialised, True)
        clara_module.close_clara()
        self.assertIs(clara_module.init_clara(PIPE + "-nobody-home"), False)
        self.assertIs(clara_module.init_clara(PIPE), True)

    def test_index_workspace_returns_files_indexed_and_ast_nodes_ints(self):
        self.assertEqual(set(self.workspace), {"files_indexed", "ast_nodes"})
        self.assertIs(type(self.workspace["files_indexed"]), int)
        self.assertIs(type(self.workspace["ast_nodes"]), int)

    def test_index_workspace_counts_new_files_as_files_indexed_and_their_chunks_as_ast_nodes(self):
        self.assertEqual(clara_module.index_workspace(ROOT), {"files_indexed": 0, "ast_nodes": 0})
        with open(os.path.join(ROOT, "fresh.rs"), "w") as handle:
            handle.write("pub fn walrus_lighthouse() -> u8 { 7 }\n")
        fresh = clara_module.index_workspace(ROOT)
        self.assertEqual(fresh["files_indexed"], 1)
        self.assertGreaterEqual(fresh["ast_nodes"], 1)
        with uriel_client.Client(PIPE) as raw:
            before = raw.stats()["chunks"]
        self.assertEqual(clara_module.index_workspace(ROOT), {"files_indexed": 0, "ast_nodes": 0})
        with uriel_client.Client(PIPE) as raw:
            self.assertEqual(raw.stats()["chunks"], before)

    def test_query_context_returns_dicts_with_exactly_the_six_clara_keys_and_exact_types(self):
        rows = clara_module.query_context(QUERY)
        self.assertIs(type(rows), list)
        self.assertTrue(rows)
        for row in rows:
            self.assertIs(type(row), dict)
            self.assertEqual(set(row), MODULE_KEYS)
            for key in ("file", "type", "name", "signature"):
                self.assertIs(type(row[key]), str)
            for key in ("start_line", "end_line"):
                self.assertIs(type(row[key]), int)
            self.assertGreaterEqual(row["start_line"], 1)
            self.assertLessEqual(row["start_line"], row["end_line"])
            self.assertTrue(row["type"])
            self.assertTrue(row["name"])
        self.assertEqual(rows[0]["file"], "a.rs")

    def test_query_context_names_the_symbol_when_the_chunk_starts_with_a_definition(self):
        rows = clara_module.query_context("parse_config")
        self.assertEqual(rows[0]["file"], "util.py")
        self.assertEqual(rows[0]["type"], "def")
        self.assertEqual(rows[0]["name"], "parse_config")
        self.assertIn("parse_config", rows[0]["signature"])

    def test_query_context_honours_limit(self):
        self.assertLessEqual(len(clara_module.query_context("function", limit=1)), 1)
        self.assertLessEqual(len(clara_module.query_context("function", limit=2)), 2)
        self.assertEqual(clara_module.query_context("function", limit=0), [])

    def test_get_stats_returns_the_host_stats_dict_of_ints(self):
        stats = clara_module.get_stats()
        self.assertIs(type(stats), dict)
        self.assertEqual(set(stats), set(uriel_client.STATS_FIELDS))
        for value in stats.values():
            self.assertIs(type(value), int)
        self.assertGreaterEqual(stats["files"], 3)

    def test_functions_raise_before_init_clara(self):
        clara_module.close_clara()
        with self.assertRaises(RuntimeError):
            clara_module.get_stats()
        self.assertIs(clara_module.init_clara(PIPE), True)


class Concurrency(unittest.TestCase):
    def test_sixteen_oracles_connecting_at_the_same_instant_all_get_a_connection(self):
        failures = []
        gate = threading.Barrier(16)

        def worker():
            try:
                gate.wait(10)
                with ClaraOracle(PIPE) as oracle:
                    oracle.stats()
            except BaseException:
                failures.append(traceback.format_exc())

        threads = [threading.Thread(target=worker) for _ in range(16)]
        for thread in threads:
            thread.start()
        for thread in threads:
            thread.join(60)
        self.assertEqual(failures, [])

    def test_two_oracles_query_at_once_and_a_disconnect_mid_request_does_not_wedge_the_server(self):
        failures = []
        started = threading.Barrier(3)

        def worker():
            try:
                with ClaraOracle(PIPE) as oracle:
                    oracle.crawl(ROOT)
                    started.wait(10)
                    for _ in range(40):
                        results = oracle.search(QUERY, k=3)
                        if not results or results[0]["path"] != "a.rs":
                            failures.append(results)
            except BaseException as error:
                failures.append(traceback.format_exc())
                started.abort()

        threads = [threading.Thread(target=worker) for _ in range(2)]
        for thread in threads:
            thread.start()
        try:
            started.wait(10)
        except threading.BrokenBarrierError:
            self.fail(failures)
        quitter = uriel_client.Client(PIPE)
        quitter.pipe.write(search_frame("slow-probe vanish before the reply"))
        time.sleep(0.05)
        quitter.close()
        for thread in threads:
            thread.join(60)
            self.assertFalse(thread.is_alive())
        self.assertEqual(failures, [])
        deadline = time.monotonic() + 10
        with ClaraOracle(PIPE) as survivor:
            self.assertEqual(survivor.search(QUERY)[0]["path"], "a.rs")
            with uriel_client.Client(PIPE) as raw:
                while raw.stats()["connections"] > 2 and time.monotonic() < deadline:
                    time.sleep(0.01)
                self.assertLessEqual(raw.stats()["connections"], 2)


if __name__ == "__main__":
    unittest.main(argv=[sys.argv[0], "-v"])
