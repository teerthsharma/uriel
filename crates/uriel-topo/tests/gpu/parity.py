import importlib.util
import math
import struct
import sys
from pathlib import Path

try:
    import torch
except ImportError:
    print("SKIP: torch is not importable")
    sys.exit(0)

if not torch.cuda.is_available():
    print("SKIP: cuda is not available")
    sys.exit(0)

HEAD_DIM = 64
TF32_REL = 2.0**-10
MEM_LIMIT = 4 * 2**30
WARMUP = 10
ITERS = 50


def decode(raw):
    if len(raw) < 20 or raw[:4] != b"UCSR":
        raise ValueError("bad header")
    version, block, blocks, nnz = struct.unpack_from("<4I", raw, 4)
    if version != 1 or len(raw) != 20 + 4 * (blocks + 1 + nnz):
        raise ValueError("bad version or length")
    values = struct.unpack_from(f"<{blocks + 1 + nnz}i", raw, 20)
    return block, list(values[: blocks + 1]), list(values[blocks + 1 :])


def violations(offsets, indices):
    bad = []
    if offsets[0] != 0 or offsets[-1] != len(indices):
        bad.append("offset bounds")
    for r in range(len(offsets) - 1):
        row = indices[offsets[r] : offsets[r + 1]]
        if not row:
            bad.append(f"row {r} empty")
            continue
        if any(a >= b for a, b in zip(row, row[1:])) or row[0] < 0:
            bad.append(f"row {r} not strictly ascending")
        if row[-1] != r:
            bad.append(f"row {r} does not end on its diagonal")
        if any(c > r for c in row):
            bad.append(f"row {r} has a block above the diagonal")
    return bad


def block_mask(offsets, indices, block, device):
    nb = len(offsets) - 1
    allow = torch.zeros(nb, nb, dtype=torch.bool)
    for r in range(nb):
        cols = [c for c in indices[offsets[r] : offsets[r + 1]] if 0 <= c < nb]
        allow[r, cols] = True
    allow = allow.repeat_interleave(block, 0).repeat_interleave(block, 1).to(device)
    pos = torch.arange(nb * block, device=device)
    return allow & (pos[None, :] <= pos[:, None])


def reference(q, k, v, mask):
    logits = (q.float() @ k.float().transpose(-1, -2)) / math.sqrt(q.shape[-1])
    logits = logits.masked_fill(~mask, float("-inf"))
    return torch.softmax(logits, -1) @ v.float()


def tolerance(v):
    return TF32_REL * v.abs().max().item() + 1e-5


def errors(out, ref):
    diff = (out.float() - ref).abs()
    return diff.max().item(), diff.max().item() / ref.abs().max().item()


def to_dev(offsets, indices):
    return (
        torch.tensor(offsets, dtype=torch.int32, device="cuda"),
        torch.tensor(indices, dtype=torch.int32, device="cuda"),
    )


def timed(fn):
    for _ in range(WARMUP):
        fn()
    torch.cuda.synchronize()
    start, end = torch.cuda.Event(enable_timing=True), torch.cuda.Event(enable_timing=True)
    samples = []
    for _ in range(ITERS):
        start.record()
        fn()
        end.record()
        torch.cuda.synchronize()
        samples.append(start.elapsed_time(end))
    samples.sort()
    return samples[len(samples) // 2]


def main(csr_dir, kernel_path):
    spec = importlib.util.spec_from_file_location("scheduled_kernel", kernel_path)
    kernel = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(kernel)
    torch.backends.cuda.matmul.allow_tf32 = False
    torch.cuda.reset_peak_memory_stats()
    print(f"torch {torch.__version__} cuda {torch.version.cuda} {torch.cuda.get_device_name(0)}")
    failed = False
    data = {}
    for path in sorted(Path(csr_dir).glob("*.ucsr")):
        block, offsets, indices = decode(path.read_bytes())
        nb = len(offsets) - 1
        seq = nb * block
        torch.manual_seed(1234 + seq)
        q, k, v = (torch.randn(seq, HEAD_DIM, device="cuda", dtype=torch.float16) for _ in range(3))
        data[path.stem] = (block, offsets, indices, q, k, v)
        problems = violations(offsets, indices)
        mask = block_mask(offsets, indices, block, "cuda")
        ref = reference(q, k, v, mask)
        out = kernel.scheduled_attention(q, k, v, *to_dev(offsets, indices), block)
        torch.cuda.synchronize()
        tol = tolerance(v)
        abs_err, rel_err = errors(out, ref)
        ok = not problems and abs_err <= tol
        line = f"{path.stem:<16} seq={seq} block={block} nnz={len(indices)}/{nb * (nb + 1) // 2} max_abs={abs_err:.3e} rel={rel_err:.3e} tol={tol:.3e}"
        if path.stem.startswith("all_"):
            sdpa = torch.nn.functional.scaled_dot_product_attention(
                q.float()[None, None], k.float()[None, None], v.float()[None, None], is_causal=True
            )[0, 0]
            mask_vs_sdpa = (ref - sdpa).abs().max().item()
            kernel_vs_sdpa = (out.float() - sdpa).abs().max().item()
            ok = ok and mask_vs_sdpa <= 1e-4 and kernel_vs_sdpa <= tol
            line += f" ref_vs_sdpa={mask_vs_sdpa:.3e} kernel_vs_sdpa={kernel_vs_sdpa:.3e}"
        pruned = [r for r in range(nb) if offsets[r + 1] - offsets[r] < r + 1]
        if pruned:
            rows = torch.cat([torch.arange(r * block, (r + 1) * block) for r in pruned]).to("cuda")
            line += f" pruned_rows={len(pruned)}/{nb} pruned_max_abs={(out.float() - ref)[rows].abs().max().item():.3e}"
        print(("PASS " if ok else "FAIL ") + line)
        failed |= not ok

    name = next(n for n in data if not n.startswith("all_"))
    block, offsets, indices, q, k, v = data[name]
    row = (len(offsets) - 1) // 2
    out_ok = kernel.scheduled_attention(q, k, v, *to_dev(offsets, indices), block)
    first = list(indices)
    first[offsets[row]] = row + 3
    first_off = offsets
    after = indices[: offsets[row + 1]] + [row + 3] + indices[offsets[row + 1] :]
    after_off = offsets[: row + 1] + [o + 1 for o in offsets[row + 1 :]]
    for label, off, idx in (("first", first_off, first), ("after_diagonal", after_off, after)):
        problems = violations(off, idx)
        out_bad = kernel.scheduled_attention(q, k, v, *to_dev(off, idx), block)
        ref_bad = reference(q, k, v, block_mask(off, idx, block, "cuda"))
        torch.cuda.synchronize()
        nan_rows = int(out_bad.isnan().any(dim=-1).sum().item())
        moved = (out_bad.float() - out_ok.float()).nan_to_num(nan=float("inf")).abs().max().item()
        flagged = any("above the diagonal" in p for p in problems)
        print(f"negative {name} {label}: block {row + 3} in row {row}; reference flags={flagged} {problems}")
        print(f"  kernel accepted it without error; nan_rows={nan_rows}; max abs vs clean kernel output={moved:.3e}; ref_bad vs clean ref={(ref_bad - reference(q, k, v, block_mask(offsets, indices, block, 'cuda'))).abs().max().item():.3e}")
        failed |= not flagged

    sdpa = torch.nn.functional.scaled_dot_product_attention(
        q.float()[None, None], k.float()[None, None], v.float()[None, None], is_causal=True
    )[0, 0]
    sparse_ref = reference(q, k, v, block_mask(offsets, indices, block, "cuda"))
    gap = (sparse_ref - sdpa).abs().max().item()
    print(f"control {name}: sparse reference vs plain causal SDPA max_abs={gap:.3e} (must exceed tol {tolerance(v):.3e})")
    failed |= gap <= tolerance(v)

    print("secondary timing, median ms of", ITERS, "after", WARMUP, "warmup, fp16, head_dim", HEAD_DIM)
    for name in sorted(n for n in data if not n.startswith("all_")):
        block, offsets, indices, q, k, v = data[name]
        seq = q.shape[0]
        dense = next(n for n in data if n == f"all_s{seq}")
        d_off, d_idx = to_dev(*data[dense][1:3])
        s_off, s_idx = to_dev(offsets, indices)
        mask = block_mask(offsets, indices, block, "cuda")
        for heads in (1, 8):
            shape = (1, heads, seq, HEAD_DIM) if heads > 1 else (seq, HEAD_DIM)
            qq, kk, vv = (torch.randn(shape, device="cuda", dtype=torch.float16) for _ in range(3))
            sparse_ms = timed(lambda: kernel.scheduled_attention(qq, kk, vv, s_off, s_idx, block))
            dense_ms = timed(lambda: kernel.scheduled_attention(qq, kk, vv, d_off, d_idx, block))
            ref_ms = timed(lambda: reference(qq, kk, vv, mask))
            print(
                f"  {name:<16} shape={tuple(shape)} sparse_kernel={sparse_ms:.3f} dense_csr_kernel={dense_ms:.3f} "
                f"fp32_masked_reference={ref_ms:.3f} sparse_vs_dense_csr={dense_ms / sparse_ms:.2f}x"
            )

    peak = torch.cuda.max_memory_allocated()
    print(f"peak torch memory {peak / 2**20:.1f} MiB (limit {MEM_LIMIT / 2**20:.0f} MiB)")
    failed |= peak > MEM_LIMIT
    print("RESULT", "FAIL" if failed else "PASS")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1], sys.argv[2]))
