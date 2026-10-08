# 0007. Source access: mapped files with guarded reads, chunked reads for remote files

- Status: Accepted
- Date: 2026-10-08

## Context

§5.5: memory-map source files and never copy a whole file into the JS heap or one Rust buffer. Network drives and cloud placeholder files (OneDrive Files On-Demand) are the exception: they use a buffered chunk cache (pread + LRU), because an I/O error on a mapped view surfaces as a crash (`EXCEPTION_IN_PAGE_ERROR` on Windows, `SIGBUS` on Unix). Risk #7 asks for that fault to be contained even on local files: a file truncated by another process, a removed USB disk, a failing sector.

The TXT row in §4 needs a cold 2 GB file indexed in ≤ 4 s (≈ 2.3 s effective in ref-sim) and searched in ≤ 3 s (≈ 1.7 s), so how bulk scans read the file matters as much as how the viewport reads it.

## Decision

1. **`Source` trait** (`quire-text::source`): `len`, `read_at(offset, &mut buf)`, `prefetch(offset, len)`. Callers always copy the range they need. No borrowed slice of a mapping ever escapes, so there is no `&[u8]` whose backing pages can vanish.
2. **Local files are mapped** (`memmap2`, read-only). Every read is a **guarded copy** in a small C helper (`src/source/guard.c`, built with `cc`):
   - Windows: `__try { memcpy } __except` that handles only `EXCEPTION_IN_PAGE_ERROR`. An access violation is a real bug and keeps propagating.
   - Unix: a process-wide `SIGBUS` handler, installed once, `siglongjmp`s out of a guarded copy on the faulting thread. A `SIGBUS` anywhere else goes to the previous handler (Rust's stack-overflow handler or the default action), so nothing else changes.
   - The fault becomes an `io::Error`. The viewer shows the affected rows as unreadable, and a save fails cleanly with the original untouched.
3. **Bulk scans** (line index, search, save) read the mapping in 1 MiB pieces from several scheduler workers, with `PrefetchVirtualMemory` (Windows) or `madvise(WILLNEED)` (Unix) issued about 64 MiB ahead of the readers.
4. **Remote and placeholder files** use `ChunkedSource`: positional reads (`ReadFile` with an offset / `pread`) into 256 KiB chunks in an `LruCache` registered with the memory governor. Reads over 1 MiB bypass the cache so a scan doesn't flush it. Detection:
   - Windows: UNC paths (`\\server\share`, `\\?\UNC\`) are checked first, because touching a share that is down can block for seconds. Then file attributes `RECALL_ON_DATA_ACCESS`, `RECALL_ON_OPEN` and `OFFLINE` mark cloud placeholders, and `GetDriveTypeW` on the volume root (`GetVolumePathNameW`, which handles mount points) marks mapped network drives.
   - Linux: `statfs` magic numbers for NFS, SMB/CIFS, FUSE (cloud clients, sshfs), 9P, Coda and AFS.
   - macOS: `statfs` without `MNT_LOCAL`.
5. **Empty files** are an empty in-memory source, since an empty file can't be mapped.
6. **Identity** (`FileStamp`): length, last-write time and file id (volume serial + file index on Windows, device + inode on Unix). External change detection and the recovery journal compare stamps.

## Evidence

Cold sequential newline count of the 2 GiB fixture (`spikes/source-scan`, `bench/results/m1/source-scan-2026-10-08.json`). Ref-sim pinning (4 P-cores), file purged before each cold run, 3 runs each, dev machine on AC power:

| Access path | 1 thread, cold | 4 threads, cold | 4 threads, warm |
|---|---|---|---|
| `pread`, 1 MiB reads | 0.66–0.72 s | 0.94–1.40 s | 0.32 s |
| Mapping, guarded copy | 1.90–2.00 s | 1.60–1.65 s | 0.35 s |
| **Mapping + prefetch 64 MiB ahead, guarded copy** | 1.52–1.63 s | **0.49–0.53 s** | 0.36 s |

- A mapping without prefetch is page-fault bound: about 1.1 GB/s cold. Windows reads a mapped file in small clusters on demand.
- `PrefetchVirtualMemory` turns that into large, queued reads: 4.3 GB/s with four readers, faster than one thread of `pread` (3.1 GB/s), which is limited by a queue depth of one.
- Warm, every path is memory-bound at 5.5–6.5 GB/s; the guarded copy costs nothing measurable.
- At 0.5 s for a cold full pass, the line index (≈ 2.3 s effective budget) and literal search (≈ 1.7 s) have room for their CPU work.

The guard is tested on every platform: Windows raises a real `EXCEPTION_IN_PAGE_ERROR` inside it (a real one needs a failing device), and Unix truncates a file under a live mapping, reads it, and gets an error instead of `SIGBUS` (`source::tests`).

## Alternatives considered

| Option | License | Why not |
|---|---|---|
| Read the whole file into memory | — | Violates §5.5; 2 GB of RAM per document |
| `pread` for everything, no mapping | — | Each document would need its own cache to avoid re-reading the viewport, counted against the 1.5 GB budget; slower than prefetched mapping for cold scans (above). Kept for remote files, where a mapping is unsafe. |
| Mapping with borrowed slices and a vectored exception handler | — | A handler can't make the faulting instruction "return an error" without rewriting the thread context; unwinding a hardware exception through Rust frames is undefined behavior. |
| `ReadProcessMemory` on our own process as the guard (Windows) | — | Works, but costs a system call per read and has no Unix equivalent that sandboxes allow everywhere (`process_vm_readv` is blocked by default seccomp profiles). |
| Mapping without a guard | — | A network drop or a truncation by another process crashes the app (risk #7). |

Dependencies: `memmap2` (MIT/Apache-2.0), `cc` (MIT/Apache-2.0, build only), `windows-sys` (MIT/Apache-2.0), `libc` (MIT/Apache-2.0).

## Consequences

- While a file is mapped, Windows refuses to truncate it (`ERROR_USER_MAPPED_FILE`), so another program that saves by truncating and rewriting in place gets an error. Programs that save by writing a temp file and renaming it, as we do, are unaffected. The same is true of every editor that maps files; it's listed in the user docs.
- Bytes changed in place by another program change what we read. External change detection (§5.8) reports it; while a document has unsaved edits we additionally hold a handle that denies write sharing (task 9).
- A cloud file that is hydrated (fully local) is mapped like any local file. If it's dehydrated while open, the guard turns the failed read into an error.
