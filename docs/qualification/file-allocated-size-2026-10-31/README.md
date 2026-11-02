# Allocated file size in Get Info

Independent adaptation of [Dust's disk usage versus apparent-size distinction](https://github.com/bootandy/dust/blob/master/README.md), cohort #82. No upstream implementation copied.

Get Info displays **Allocated size** separately from the logical **Size**, including exact byte count. Native Unix listing metadata supplies `st_blocks * 512` with checked arithmetic, only for regular files. Opening/rendering Info reads the retained facts; it adds no metadata or content I/O. Unsupported/unobserved facts are Unavailable, rather than zero. Directories and symbolic links say Not measured; no target allocation or shallow directory block count masquerades as recursive folder usage.

The card explains that blocks can be shared and do not predict freed space. The number is a listing-time filesystem observation, not exclusive physical ownership: hard links, clones, compression and filesystem policies remain relevant. Refresh the directory to obtain updated facts. Windows allocation, folder allocation totals, allocated-size sorting and treemap modes remain open.

## Validation

- Three new regression tests use real sparse, empty, written, hard-linked and symbolic-linked files; compare metadata block counts; distinguish unknown/zero; reject folder totals; and build Info after source deletion to prove snapshot use without a restat.
- Full suite **3108 passed / 3 ignored**. Six commands in `checks.json` must all exit zero.
- Native `allocated_info` opens the actual Get Info palette command for an 8-MiB sparse file and requires logical 8 MiB versus exactly 0 allocated bytes on this host. The state is written to `info-state.json`; screenshot is visually inspected.
- Existing native `symlink_info` checks target/kind/status behavior with the new row present. Both scenarios **14/14**, native-port effects zero.
- `verify.py` binds both captures to the final executable, checks command outcomes, actual Info fields and manifests. The native fixtures exercise this host's sparse-file capability; no universal zero-allocation filesystem guarantee is claimed.
