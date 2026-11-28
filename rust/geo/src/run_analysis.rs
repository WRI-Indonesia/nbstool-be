//! `run_analysis.py`: the WHOLE analysis as ONE NDJSON stream (F02-P2 site characterisation,
//! F02-P3 threat sections, F02-P4 `pathway` as one component, and 4.4 `conservation actions` as a
//! dependent of `habitat area`), cheapest expected finish first across all three, on a pool of 4.

use serde_json::json;

use crate::Result;
use crate::pathway::{conservation_actions, run_pathway};
use crate::pipeline::{
    Job, Pair, PoolViews, Process, Stream, cheapest_first, plan, resolve, run_pool, stream,
};
use crate::site_characterisation::run_site_characterisation::{self as rsc, RunInputs};
use crate::threat::run_threat;

/// `_MAX_WORKERS`
pub const MAX_WORKERS: usize = 4;

/// `COMPONENTS`: `(name, cost, deps)` in `_ORDER`.
pub fn components() -> Vec<(String, f64, Vec<String>)> {
    // _UNORDERED: site characterisation (already in its own _ORDER), threat sections, pathway, 4.4
    let mut table: Vec<(String, f64, Vec<String>)> = rsc::components();
    let threat = run_threat::processes();
    for name in run_threat::SECTIONS {
        let w = threat.iter().find(|p| p.name == name).map_or(0.0, |p| p.w);
        table.push((name.to_string(), w, vec![]));
    }
    table.push(("pathway".into(), 3.5, vec![]));
    table.push((
        "conservation actions".into(),
        0.5,
        vec!["habitat area".into()],
    ));
    let borrowed: Vec<(&str, f64, Vec<&str>)> = table
        .iter()
        .map(|(n, c, d)| (n.as_str(), *c, d.iter().map(String::as_str).collect()))
        .collect();
    cheapest_first(&borrowed)
        .into_iter()
        .filter_map(|n| table.iter().find(|t| t.0 == n).cloned())
        .collect()
}

/// `processes`
pub fn processes() -> Vec<Process> {
    let mut p = vec![Process::new("preparation", 0.1)];
    p.extend(
        components()
            .into_iter()
            .map(|(n, c, _)| Process { name: n, w: c }),
    );
    p.push(Process::new("end", 0.1));
    p
}

/// One union component's body.
fn run_component(name: &str, inp: &RunInputs, deps: &[&Pair]) -> Result<Pair> {
    let (s, k, a) = (&*inp.settings, &*inp.keys, &*inp.aoi);
    if run_threat::SECTIONS.contains(&name) {
        return Ok(run_threat::run_section(name, s, k, a));
    }
    match name {
        // _pathway_component: the PathwaySelection document unchanged
        "pathway" => Ok((
            json!({"narrative": "", "tables": {}, "values": {}, "flags": [], "missing": []}),
            run_pathway::run_pathway(s, a)?,
        )),
        "conservation actions" => match deps {
            [habitat] => conservation_actions::analyze_conservation_actions(s, a, habitat),
            _ => Err(crate::Error::new(
                "TypeError",
                "conservation actions takes 2.3",
            )),
        },
        _ => rsc::run_component(name, s, k, a, deps, &inp.provided),
    }
}

/// `stream_analysis(aoi, wanted, retry_url)`: a lazy iterator of NDJSON lines. `wanted` names
/// must be known (`components()`; the route answers 400 first).
pub fn stream_analysis(
    inputs: RunInputs,
    wanted: Option<&[String]>,
    retry_url: Option<crate::pipeline::RetryUrl>,
) -> Result<Stream<PoolViews>> {
    let comps = components();
    let order: Vec<String> = comps.iter().map(|c| c.0.clone()).collect();
    let deps_of = |n: &str| {
        comps
            .iter()
            .find(|c| c.0 == n)
            .map(|c| c.2.clone())
            .unwrap_or_default()
    };
    let running = resolve(&order, deps_of, wanted);
    let plan = plan(&processes(), wanted);
    let jobs: Vec<Job> = running
        .iter()
        .map(|name| {
            let deps: Vec<usize> = deps_of(name)
                .iter()
                .filter_map(|d| running.iter().position(|r| r == d))
                .collect();
            let (inp, n) = (inputs.clone(), name.clone());
            Job {
                name: name.clone(),
                deps,
                run: Box::new(move |d| run_component(&n, &inp, d)),
            }
        })
        .collect();
    let emit: Vec<usize> = plan[1..plan.len() - 1]
        .iter()
        .filter_map(|p| running.iter().position(|r| *r == p.name))
        .collect();
    let workers = running.len().min(MAX_WORKERS);
    stream(plan, run_pool(jobs, emit, workers), retry_url)
}
