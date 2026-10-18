import json
import struct
import sys
import time

MSG_SEARCH = 0x02
MSG_STATS = 0x03
MSG_PING = 0x04
MSG_INDEX = 0x05
MSG_INDEX_FILE = 0x06
MSG_SEARCH_OK = 0x82
MSG_STATS_OK = 0x83
MSG_PONG = 0x84
MSG_INDEX_OK = 0x85
MSG_ERROR = 0xE0
MSG_BUSY = 0xE1

ERROR_PIPE_BUSY = 231
STATS_FIELDS = (
    "arena_cap",
    "arena_used",
    "epoch",
    "requests_served",
    "requests_failed",
    "requests_busy",
    "in_flight",
    "queue_len",
    "connections",
    "chunks",
    "files",
    "embed_dim",
)
ERROR_CODES = {
    1: "query",
    2: "search",
    3: "malformed",
    4: "shutting_down",
    5: "too_large",
    6: "internal",
    7: "index",
}
REPORT_FIELDS = (
    "files_scanned",
    "files_indexed",
    "files_unchanged",
    "files_skipped",
    "files_removed",
    "chunks_added",
    "chunks_removed",
)


class HostError(Exception):
    pass


class RemoteError(HostError):
    def __init__(self, code, message):
        super().__init__("%s: %s" % (code, message))
        self.code = code
        self.message = message


class Busy(HostError):
    pass


class Reader:
    def __init__(self, data):
        self.data = data
        self.pos = 0

    def take(self, n):
        end = self.pos + n
        if end > len(self.data):
            raise HostError("truncated response")
        chunk = self.data[self.pos:end]
        self.pos = end
        return chunk

    def unpack(self, fmt):
        return struct.unpack("<" + fmt, self.take(struct.calcsize("<" + fmt)))[0]

    def string(self):
        return self.take(self.unpack("I")).decode("utf-8")

    def finish(self):
        if self.pos != len(self.data):
            raise HostError("trailing bytes in response")


def put_string(text):
    raw = text.encode("utf-8")
    return struct.pack("<I", len(raw)) + raw


class Client:
    def __init__(self, path, max_frame=1 << 20, connect_timeout=5.0):
        self.max_frame = max_frame
        self.nonce = 0
        deadline = time.monotonic() + connect_timeout
        while True:
            try:
                self.pipe = open(path, "r+b", buffering=0)
                break
            except OSError as e:
                busy = getattr(e, "winerror", None) == ERROR_PIPE_BUSY
                if not busy or time.monotonic() >= deadline:
                    raise
                time.sleep(0.01)
        try:
            self.ping()
        except BaseException:
            self.close()
            raise

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        self.close()

    def close(self):
        self.pipe.close()

    def read_exact(self, n):
        buf = bytearray(n)
        view = memoryview(buf)
        got = 0
        while got < n:
            read = self.pipe.readinto(view[got:])
            if not read:
                raise HostError("connection closed")
            got += read
        return bytes(buf)

    def call(self, kind, payload=b""):
        body = bytes([kind]) + payload
        if len(body) > self.max_frame:
            raise HostError("request of %d bytes exceeds the limit of %d" % (len(body), self.max_frame))
        frame = struct.pack("<I", len(body)) + body
        view = memoryview(frame)
        sent = 0
        while sent < len(frame):
            sent += self.pipe.write(view[sent:])
        (length,) = struct.unpack("<I", self.read_exact(4))
        if length == 0 or length > self.max_frame:
            raise HostError("response length %d outside 1..%d" % (length, self.max_frame))
        reply = self.read_exact(length)
        reader = Reader(reply[1:])
        kind = reply[0]
        if kind == MSG_ERROR:
            code = reader.unpack("B")
            raise RemoteError(ERROR_CODES.get(code, str(code)), reader.string())
        if kind == MSG_BUSY:
            raise Busy("server queue is full")
        return kind, reader

    def ping(self):
        self.nonce += 1
        kind, reader = self.call(MSG_PING, struct.pack("<Q", self.nonce))
        if kind != MSG_PONG:
            raise HostError("unexpected reply %#x to ping" % kind)
        echoed = reader.unpack("Q")
        reader.finish()
        if echoed != self.nonce:
            raise HostError("pong carried a different nonce")

    def stats(self):
        kind, reader = self.call(MSG_STATS)
        if kind != MSG_STATS_OK:
            raise HostError("unexpected reply %#x to stats" % kind)
        out = {name: reader.unpack("Q") for name in STATS_FIELDS}
        reader.finish()
        return out

    def read_report(self, kind, reader, what):
        if kind != MSG_INDEX_OK:
            raise HostError("unexpected reply %#x to %s" % (kind, what))
        out = {name: reader.unpack("Q") for name in REPORT_FIELDS}
        reader.finish()
        return out

    def index(self, root=""):
        kind, reader = self.call(MSG_INDEX, put_string(root))
        return self.read_report(kind, reader, "index")

    def index_file(self, path, content):
        kind, reader = self.call(MSG_INDEX_FILE, put_string(path) + put_string(content))
        return self.read_report(kind, reader, "index_file")

    def search(self, query, k=8, budget_tokens=2000, expand_hops=0, use_utility=True):
        payload = put_string(query) + struct.pack("<QQQB", k, budget_tokens, expand_hops, 1 if use_utility else 0)
        kind, reader = self.call(MSG_SEARCH, payload)
        if kind != MSG_SEARCH_OK:
            raise HostError("unexpected reply %#x to search" % kind)
        hits = []
        for _ in range(reader.unpack("I")):
            hits.append(
                {
                    "chunk_id": reader.unpack("Q"),
                    "file": reader.string(),
                    "start_line": reader.unpack("I"),
                    "end_line": reader.unpack("I"),
                    "memory": reader.string(),
                    "score": reader.unpack("f"),
                    "via": "graph_neighbour" if reader.unpack("B") else "direct",
                }
            )
        pack = {
            "hits": hits,
            "text": reader.string(),
            "tokens": reader.unpack("Q"),
            "budget_tokens": reader.unpack("Q"),
            "omitted": reader.unpack("Q"),
        }
        reader.finish()
        return pack


def main(argv):
    if len(argv) < 3:
        sys.stderr.write("usage: uriel_client.py PIPE_PATH QUERY\n")
        return 2
    with Client(argv[1]) as client:
        client.ping()
        pack = client.search(argv[2], k=2, budget_tokens=200)
        stats = client.stats()
    top = pack["hits"][0] if pack["hits"] else None
    print(
        json.dumps(
            {
                "files": stats["files"],
                "hits": len(pack["hits"]),
                "pong": True,
                "tokens": pack["tokens"],
                "top_file": top["file"] if top else None,
                "top_start_line": top["start_line"] if top else None,
            },
            sort_keys=True,
        )
    )
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
