# benefit/ (F02-P5, components 5.1-5.13 + run_benefit)

Port of `application/utils/geos/v3/benefit/*.py`, one file per Python file. Proof:
`proof/run_python_benefit.py` (original, writes `proof/expected/benefit_*`) and
`tests/proof_benefit.rs` (port, exact JSON comparison):

    cd rust/geo && . ./env.sh
    CARGO_TARGET_DIR=target-benefit cargo test --release --test proof_benefit -- --ignored --nocapture
    PROOF_AOIS=id_small PROOF_COMPONENTS=stream ...   # restrict

Inputs per AOI mirror POST /feature/benefit: duration 30, rate_pct = the recorded 1.5
`historical_deforestation_percentage` (None on id_small / id_multi2; the per-component runs use
1.23 there so the raster paths run, the stream uses the real None), ecosystem_class 1 (3 on
id_multi12, 2 on my_large), a frontend `selections` object, default deductions.

## NOTES (running)

- 4.2 is the port's own (`pathway::activity_list`, proven 6/6 against the original), run by the
  un-emitted `activity_stage` step (`benefit::activity_stage`); `values.by_category` keeps its key
  order (this crate has serde_json `preserve_order`). The per-component proofs of 5.1/5.2 still
  feed the recorded stage (`expected/benefit_stage`); the stream proof runs the real 4.2.
- `stream_benefit(settings, aoi, &BenefitRequest, workers, emit)` emits the
  NDJSON lines as JSON values in order (preparation, components cheapest first, end), validating
  first like the route. Outer pool 6 (as `_MAX_WORKERS`), dependency waits, `pipeline.safe` /
  `after` semantics (exception TYPE only, dependents of a failed step retryable).
  `run_benefit` = the dict form. Not done: persisting lines into `benefit_json`
  (`persist_ndjson`) and `_limit_slots` belong to the route (api agent).
- Species pool width: the Python benefit modules use the CONSTANT `AOH_MAX_WORKERS` (16), not the
  tbl_master_settings row; the port takes `workers` from the caller (pass
  `settings.aoh_max_workers()` as asked, or 16 to mirror the original exactly). Results are
  identical for any width (reduced in candidate order; union/count order-free).
- Merged (geo-streams): `np_argsort`, `nargsort_desc`, `truthy` now live in `py.rs` (re-exported
  by util.rs); `py_sum` was `py::fsum_py` and is gone; `error_status` is `pipeline::error_status`;
  `py_float_repr` uses `pipeline::py_float` (ties to even, as Python's repr). Still here:
  `np_round`, `py_floordiv`, `np_sum_f32`, `NativeGrid`, `read_vrt_filled0`, `pool_map`,
  `aoi_polygons_4326` (benefit-only users).
- `inventory.rs` now calls 2.3's loader (`habitat_area::load_inventory`, the `parquet` crate): ONE
  species-inventory loader; the hand-written Parquet reader is gone.
- Wire order: the stream proof also compares the NDJSON TEXT with the original's
  (`expected/benefit_stream/<aoi>.ndjson`, `proof/run_python_streams.py benefit_stream`). It found
  six cards (5.2, 5.3, 5.8, 5.10, 5.11, 5.12) whose view keys came out in a different order from
  the Python dict literal; fixed, 6/6 byte-equal.
- RISK, sort ties: 5.2 (`np.argsort(risk)` on ties of the u16 risk) and the pandas
  `sort_values` of the 5.10 / 5.11 species tables use numpy's quicksort. On Windows (MSVC numpy,
  the proof machine) that is the generic introsort, ported exactly (`np_argsort`). A Linux numpy
  1.26 on an AVX512_SKX CPU dispatches to x86-simd-sort `avx512_argsort`, which orders ties
  differently, so staging's own output on such a CPU can differ from both. Check staging's CPU
  flags; if AVX512_SKX, port `avx512-64bit-argsort.hpp` (numpy 1.26.4 sources) scalar.
- Small leftovers: Python's `union_all()` before intersects/rasterize is skipped (same point
  set); `Thrift` keeps unused variants for skipping.

## Result 2026-11-27

14 proof names (13 components + the full `stream_benefit` NDJSON) x 6 AOIs: all EXACT
(`tests/proof_benefit.rs`, 997 s, network bound; species loops ~20-90 s per AOI, about as Python).
Tie-order sensitivity measured in the original: swapping 5.2's `np.argsort` for a stable sort
changes 5.2's view on kh_large, my_large and id_multi12 (headline equal, detail fields not), so
the AVX512 risk above is real for staging on such a CPU.

## Staging CPU (checked by the user 2026-11-27)
`lscpu` on the staging VM (QEMU virtual CPU, AMD): no avx512 flags (no AVX/AVX2 either). numpy
1.26 therefore uses its generic sort there, the one ported here, so the tie order of 5.2, 5.10 and
5.11 matches staging. The AVX-512 argsort port is not needed for staging. Production CPU: unchecked.
Production (checked by the user 2026-11-27): AMD EPYC 7B13, has avx2 and fma, no avx512. numpy
1.26's SIMD sort is AVX-512 only (AVX2 sorting arrived in numpy 2.0), so production also uses
the generic sort ported here. Revisit if numpy is ever upgraded to 2.x on the Python side.
