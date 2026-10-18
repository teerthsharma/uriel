import collections
import glob
import os
import re
import sys

import tiktoken

ENCODINGS = ("o200k_base", "cl100k_base")
TOOLS = ("rag_search", "seal_recall", "view", "soul")
BUDGETS = (300, 800, 4000)
COMPONENTS = (
    "header", "alias_def", "toggle", "file_ref", "file_path", "colon", "range", "hash", "id",
    "flags", "linecount", "text", "newline", "legacy_marker", "legacy_prefix",
)
FLAGS = re.compile(r"^((?:L\d|[gmr~])*)(\+\d+)?$")


def label_v3(text):
    lab = ["text"] * len(text)
    pos = 0
    remaining = 0
    aliasing = True
    lines = text.split("\n")
    for index, line in enumerate(lines):
        start, end = pos, pos + len(line)

        def paint(a, b, name):
            for k in range(a, b):
                lab[k] = name

        if index == 0:
            paint(start, end, "header")
        elif remaining > 0:
            remaining -= 1
        elif aliasing and re.match(r"^@[A-Za-z]=", line):
            paint(start, end, "alias_def")
        elif line in ("=g", "="):
            aliasing = False
            paint(start, end, "toggle")
        else:
            aliasing = False
            skip = 3 if line.startswith("@") and len(line) > 2 and line[2] == " " else 0
            cut = line.find(" ", skip)
            head, rest_at = (line, None) if cut < 0 else (line[:cut], cut)
            loc, hash_at, tail = head.rpartition("#")
            m = re.match(r"^(\d+)(.*)$", tail)
            if hash_at and m and FLAGS.match(m.group(2)):
                o = start
                if ":" in loc:
                    file_part, _, range_part = loc.rpartition(":")
                    kind = "file_ref" if file_part.isdigit() else "file_path"
                    paint(o, o + len(file_part), kind)
                    o += len(file_part)
                    paint(o, o + 1, "colon")
                    o += 1
                else:
                    range_part = loc
                paint(o, o + len(range_part), "range")
                o += len(range_part)
                paint(o, o + 1, "hash")
                o += 1
                paint(o, o + len(m.group(1)), "id")
                o += len(m.group(1))
                flags = FLAGS.match(m.group(2))
                paint(o, o + len(flags.group(1)), "flags")
                o += len(flags.group(1))
                if flags.group(2):
                    paint(o, o + len(flags.group(2)), "linecount")
                    remaining = int(flags.group(2)[1:])
                if rest_at is not None:
                    paint(rest_at, rest_at + 1, "text")
        if end < len(text):
            lab[end] = "newline"
        pos = end + 1
    return lab


def label_v2(text):
    lab = ["text"] * len(text)
    pos = 0
    remaining = 0
    aliasing = True
    for index, line in enumerate(text.split("\n")):
        start, end = pos, pos + len(line)
        if index == 0:
            lab[start:end] = ["header"] * len(line)
        elif remaining > 0:
            remaining -= 1
        elif aliasing and re.match(r"^@[A-Za-z]=", line):
            lab[start:end] = ["alias_def"] * len(line)
        elif line.startswith(">"):
            aliasing = False
            lab[start] = "legacy_marker"
            skip = 4 if line[1:2] == "@" and line[3:4] == " " else 1
            cut = line.find(" ", skip)
            head = line if cut < 0 else line[:cut]
            loc, hash_at, tail = head.rpartition("#")
            m = re.match(r"^(\d+)(.*)$", tail)
            if hash_at and m:
                o = start + 1
                if ":" in loc:
                    file_part, _, range_part = loc[1:].rpartition(":")
                    lab[o:o + len(file_part)] = ["file_path"] * len(file_part)
                    o += len(file_part)
                    lab[o] = "colon"
                    o += 1
                else:
                    range_part = loc[1:]
                lab[o:o + len(range_part)] = ["range"] * len(range_part)
                o += len(range_part)
                lab[o] = "hash"
                o += 1
                lab[o:o + len(m.group(1))] = ["id"] * len(m.group(1))
                o += len(m.group(1))
                flags = m.group(2)
                lab[o:o + len(flags)] = ["flags"] * len(flags)
                lm = re.search(r"\+(\d+)", flags)
                if lm:
                    remaining = int(lm.group(1))
        if end < len(text):
            lab[end] = "newline"
        pos = end + 1
    return lab


def components(enc, text, lab):
    ids = enc.encode(text, disallowed_special=())
    byte_to_char = []
    for ci, ch in enumerate(text):
        byte_to_char.extend([ci] * len(ch.encode("utf-8")))
    out = collections.Counter()
    off = 0
    for piece in enc.decode_tokens_bytes(ids):
        ci = byte_to_char[off] if off < len(byte_to_char) else len(text) - 1
        out[lab[ci]] += 1
        off += len(piece)
    return len(ids), out


def main():
    directory = sys.argv[1]
    layout = sys.argv[2] if len(sys.argv) > 2 else "v3"
    labeller = label_v3 if layout == "v3" else label_v2
    for name in ENCODINGS:
        enc = tiktoken.get_encoding(name)
        print(f"encoding {name} layout {layout}")
        print("tool,budget,files,tokens," + ",".join(COMPONENTS))
        for tool in TOOLS:
            for budget in BUDGETS:
                files = sorted(glob.glob(os.path.join(directory, f"{tool}-{budget}-*.c")))
                total, tally = 0, collections.Counter()
                for path in files:
                    with open(path, encoding="utf-8") as fh:
                        text = fh.read()
                    n, c = components(enc, text, labeller(text))
                    total += n
                    tally.update(c)
                print(f"{tool},{budget},{len(files)},{total}," + ",".join(str(tally.get(c, 0)) for c in COMPONENTS))


if __name__ == "__main__":
    main()
