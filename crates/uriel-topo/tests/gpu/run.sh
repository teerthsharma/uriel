#!/usr/bin/env bash
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
py="${URIEL_GPU_PYTHON:-}"
kernel="${URIEL_GPU_KERNEL:-}"
if [ -z "$py" ] || [ -z "$kernel" ] || [ ! -f "$kernel" ]; then
    echo "SKIP: set URIEL_GPU_PYTHON (python with torch+triton) and URIEL_GPU_KERNEL (topology_sparse_attention.py)"
    exit 0
fi
out="$(mktemp -d)"
trap 'rm -rf "$out"' EXIT
cargo run -q -p uriel-topo --example export_csr -- "$out"
cd "$out"
exec timeout 300 "$py" -I "$here/parity.py" "$out" "$kernel"
