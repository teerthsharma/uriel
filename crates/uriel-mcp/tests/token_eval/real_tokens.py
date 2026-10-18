import glob
import json
import os
import sys

import tiktoken

HERE = os.path.dirname(os.path.abspath(__file__))
ENCODINGS = ("o200k_base", "cl100k_base")
TOOLS = ("rag_search", "seal_recall", "view", "soul", "session")
BUDGETS = (300, 800, 4000)
REQUIRED_TOTAL_SAVING = 0.07


def read_all(directory):
    out = {}
    for tool in TOOLS:
        for budget in BUDGETS:
            files = sorted(glob.glob(os.path.join(directory, f"{tool}-{budget}-*.c")))
            if files:
                out[f"{tool}-{budget}"] = [open(f, encoding="utf-8").read() for f in files]
    return out


def count(texts):
    result = {}
    for name in ENCODINGS:
        enc = tiktoken.get_encoding(name)
        result[name] = [len(enc.encode(t, disallowed_special=())) for t in texts]
    return result


def record(directory, path):
    baseline = {"keys": {}}
    for key, texts in read_all(directory).items():
        baseline["keys"][key] = count(texts)
    with open(path, "w", encoding="utf-8") as fh:
        json.dump(baseline, fh, indent=1, sort_keys=True)
        fh.write("\n")


def compare(directory, path):
    with open(path, encoding="utf-8") as fh:
        baseline = json.load(fh)["keys"]
    current = {key: count(texts) for key, texts in read_all(directory).items()}
    failures = []
    for name in ENCODINGS:
        print(f"{name}: tokens over the 100 frozen queries (soul: 14 focus values)")
        print(f"{'tool':12s} {'budget':>6s} {'old':>8s} {'new':>8s} {'change':>8s} {'mean':>7s} {'worst':>7s}")
        old_total = new_total = 0
        by_budget = {b: [0, 0] for b in BUDGETS}
        for key in sorted(current, key=lambda k: (TOOLS.index(k.split("-")[0]), int(k.split("-")[1]))):
            if key not in baseline:
                failures.append(f"{key} has no recorded baseline")
                continue
            old, new = baseline[key][name], current[key][name]
            if len(old) != len(new):
                failures.append(f"{key}: {len(new)} outputs against {len(old)} recorded")
                continue
            tool, budget = key.split("-")[0], int(key.split("-")[1])
            savings = [1 - n / o for o, n in zip(old, new)]
            print(
                f"{tool:12s} {budget:6d} {sum(old):8d} {sum(new):8d} {100 * (sum(new) / sum(old) - 1):+7.2f}%"
                f" {100 * sum(savings) / len(savings):6.2f}% {100 * min(savings):6.2f}%"
            )
            if sum(new) > sum(old):
                failures.append(f"{name} {key}: {sum(new)} tokens against {sum(old)} recorded")
            if tool != "session":
                old_total += sum(old)
                new_total += sum(new)
                by_budget[budget][0] += sum(old)
                by_budget[budget][1] += sum(new)
        for budget, (o, n) in by_budget.items():
            print(f"  budget {budget}: {o} -> {n} ({100 * (n / o - 1):+.2f}%)")
        saving = 1 - new_total / old_total
        print(f"  total: {old_total} -> {new_total} ({-100 * saving:+.2f}%)")
        if saving < REQUIRED_TOTAL_SAVING:
            failures.append(f"{name}: total saving {100 * saving:.2f}% is below {100 * REQUIRED_TOTAL_SAVING:.0f}%")
    return failures


def main():
    if len(sys.argv) >= 4 and sys.argv[1] == "record":
        record(sys.argv[2], sys.argv[3])
        return 0
    directory = sys.argv[1]
    path = sys.argv[2] if len(sys.argv) > 2 else os.path.join(HERE, "real_tokens_baseline.json")
    failures = compare(directory, path)
    for f in failures:
        print("FAIL", f)
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
