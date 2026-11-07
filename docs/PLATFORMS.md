# Platforms and memory

Every feature runs on Windows, Linux and macOS. Where a feature is OS specific it has a fallback, not an error.

| Feature | Windows | Linux | macOS | Other unix |
| --- | --- | --- | --- | --- |
| Resident set (`process_memory`) | `GetProcessMemoryInfo` working set | `/proc/self/statm`, then `getrusage` peak | `task_info(MACH_TASK_BASIC_INFO)`, then `getrusage` peak | `getrusage` peak |
| Free RAM (`memory_status`) | `GlobalMemoryStatusEx` | `/proc/meminfo` MemAvailable, capped by cgroup v2 `memory.max` | `host_statistics64` free+inactive+speculative+purgeable | `sysconf` total only |
| Trim after a shed (`trim_heap`) | `HeapCompact` + `EmptyWorkingSet` | `malloc_trim` (glibc), none on musl | `malloc_zone_pressure_relief` | none |
| Large pages | `MEM_LARGE_PAGES` when the privilege is held | system allocator | system allocator | system allocator |
| Unbuffered reads | `FILE_FLAG_NO_BUFFERING` | buffered `pread` | buffered `pread` | buffered `pread` |
| Host local endpoint (`listen_pipe`) | named pipe `\\.\pipe\<name>` | unix socket `$XDG_RUNTIME_DIR/<name>.sock` or the temp dir, mode 0600 | unix socket in the temp dir, mode 0600 | same |
| Default db | `%USERPROFILE%\.seal\graph.seal` | `$HOME/.seal/graph.seal` | `$HOME/.seal/graph.seal` | same |

The unsafe calls live only in `uriel-sys`. The parsers (`statm`, `meminfo`, cgroup limit) are plain functions with tests that run on every OS.

## Budget

`--memory-mib N` is used exactly as given (minimum 16). With no flag, or `--memory-mib auto`, the budget is `auto_memory_mib(free RAM)`: 50 MiB when 2 GiB or more is free, scaled down linearly below that, never under 16 and never above 50. `uriel doctor` prints the free RAM, its source and the size auto would pick.

## Governor

`uriel-mcp` samples the resident set every 2 s and on each idle tick. The limit is the budget plus its reserve share (62.5 MiB for 50 MiB).

| Level | Enters at | Leaves at | Action |
| --- | --- | --- | --- |
| green | | | none |
| amber | 80 % of the limit | below 70 % | drop the query cache and the node tiers, trim the allocator |
| red | 100 % of the limit | below 90 % (to amber) | amber actions, then close the database (it reopens on the next call), trim again |

A level that holds sheds at most once every 10 s; a climb sheds at once. The in-memory graph is never closed. The `usage` tool reports `memory.{rss_mib, peak_rss_mib, budget_mib, limit_mib, level, shrinks, reopens, last_freed_mib, free_ram_mib, os, rss_source, free_ram_source}`.
