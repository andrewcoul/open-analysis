//! Diagnostic only. Copy into crates/oa-core/tests/parallelism_audit.rs to run.
//! Imports unchanged engine source to expose private stages without runtime edits.
#![allow(dead_code)]
#[path = "../src/analysis.rs"]
pub mod analysis;
#[path = "../src/error.rs"]
pub mod error;
#[path = "../src/io.rs"]
pub mod io;
#[path = "../src/modal.rs"]
pub mod modal;
#[path = "../src/model.rs"]
pub mod model;
#[path = "../src/results.rs"]
pub mod results;
#[path = "../src/spectrum.rs"]
pub mod spectrum;
#[path = "../src/units.rs"]
pub mod units;
pub use analysis::*;
pub use error::{Error, Result};
pub use modal::*;
pub use model::*;
use rayon::prelude::*;
pub use results::*;
use serde_json::{Value, json};
pub use spectrum::*;
use std::{hint::black_box, time::Instant};
#[path = "../src/assembly.rs"]
mod assembly;
#[path = "../src/element/mod.rs"]
mod element;
#[path = "../src/exec.rs"]
mod exec;

fn frame(b: usize, stories: usize, combos: usize) -> Model {
    let nx = b + 1;
    let id = |i, j, k| k * nx * nx + j * nx + i;
    let mut nodes = vec![];
    for k in 0..=stories {
        for j in 0..=b {
            for i in 0..=b {
                nodes.push(json!({"position":[i as f64*6.0,k as f64*4.0,j as f64*6.0],"restrained":vec![k==0;6]}));
            }
        }
    }
    let mut frames = vec![];
    let mut member = vec![];
    for k in 0..stories {
        for j in 0..=b {
            for i in 0..=b {
                frames.push(json!({"nodes":[id(i,j,k),id(i,j,k+1)],"material":0,"section":0}));
            }
        }
    }
    for k in 1..=stories {
        for j in 0..=b {
            for i in 0..=b {
                for (di, dj) in [(1, 0), (0, 1)] {
                    if i + di <= b && j + dj <= b {
                        member.push(json!({"member":frames.len(),"type":"distributed","start":0.0,"end":6.0,"start_load":[0.0,-10000.0,0.0],"end_load":[0.0,-10000.0,0.0],"axes":"global"}));
                        frames.push(
                            json!({"nodes":[id(i,j,k),id(i+di,j+dj,k)],"material":0,"section":0}),
                        );
                    }
                }
            }
        }
    }
    let nodal: Vec<_> = (nx * nx..nodes.len())
        .map(|i| json!({"node":i,"force":[5000.0,0.0,0.0]}))
        .collect();
    let combinations: Vec<_> = (0..combos).map(|c| json!({"name":format!("c{c}"),"terms":[[0,1.0+0.05*c as f64],[1,if c%2==0 {0.5+0.1*c as f64} else {-0.5-0.1*c as f64}]]})).collect();
    serde_json::from_value(json!({"schema_version":1,"nodes":nodes,"materials":[{"young":200e9,"poisson":0.3,"density":7850.0}],"sections":[{"area":0.01,"iy":2e-5,"iz":4e-5,"torsion":1e-5}],"frames":frames,"load_cases":[{"name":"dead","member":member},{"name":"wind","nodal":nodal}],"combinations":combinations})).unwrap()
}

fn plate(n: usize) -> Model {
    let nodes: Vec<_> = (0..=n).flat_map(|j| (0..=n).map(move |i| json!({"position":[i as f64/n as f64,j as f64/n as f64,0.0],"restrained":[true,true,i==0||i==n||j==0||j==n,false,false,true]}))).collect();
    let shells: Vec<_> = (0..n).flat_map(|j| (0..n).map(move |i| { let a=j*(n+1)+i; json!({"nodes":[a,a+1,a+n+2,a+n+1],"material":0,"thickness":0.01,"formulation":"dkmq","drilling_ratio":1e-3}) })).collect();
    let surface: Vec<_> = (0..shells.len())
        .map(|s| json!({"shell":s,"pressure":-1000.0}))
        .collect();
    serde_json::from_value(json!({"schema_version":1,"nodes":nodes,"materials":[{"young":200e9,"poisson":0.3,"density":7850.0}],"shells":shells,"load_cases":[{"name":"pressure","surface":surface}]})).unwrap()
}

fn timed<T>(f: impl FnOnce() -> T) -> (f64, T) {
    let t = Instant::now();
    let value = f();
    (t.elapsed().as_secs_f64() * 1000.0, value)
}
fn median(values: &mut [f64]) -> f64 {
    values.sort_by(f64::total_cmp);
    values[values.len() / 2]
}
fn sample(mut f: impl FnMut(), repeats: usize) -> Value {
    f();
    let mut ms: Vec<_> = (0..repeats).map(|_| timed(&mut f).0).collect();
    let med = median(&mut ms);
    json!({"median_ms":med,"samples_ms":ms})
}
fn displacements(r: &InMemoryResults) -> Vec<f64> {
    r.combinations
        .iter()
        .flat_map(|c| c.displacements.as_ref().unwrap().iter().flatten().copied())
        .collect()
}

fn stages(model: &Model, threads: usize, label: &str) -> Value {
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .thread_name(move |i| format!("stages-{threads}-{i}"))
        .build()
        .unwrap();
    pool.install(|| {
        let prep=assembly::Prepared::new(model).unwrap();
        let combo=&model.effective_combinations()[0];
        let loads=prep.loads(combo);
        let axial=vec![0.0;prep.frames.len()]; let active=vec![true;prep.frames.len()];
        let states=prep.states(&loads,&axial,&active).unwrap();
        let system=prep.assemble(model,&states).unwrap();
        let force=prep.force(&loads,&states);
        let (u,_,_)=system.solve(&force).unwrap();
        let scaled: Vec<_>=system.reduced_entries().iter().map(|t| faer::sparse::Triplet::new(t.row,t.col,t.val*system.scale()[t.row]*system.scale()[t.col])).collect();
        let k=faer::sparse::SparseColMat::try_new_from_triplets(system.free.len(),system.free.len(),&scaled).unwrap();
        let symbolic=faer::sparse::linalg::solvers::SymbolicLlt::try_new(k.symbolic(),faer::Side::Lower).unwrap();
        let repeats=5;
        // After the 2026-09-15 changes `states` borrows cached stiffness and is
        // itself parallel above a threshold; the trial recomputes every state.
        let parallel_states=|| prep.frames.par_iter().enumerate().map(|(i,e)| e.state(element::frame::Stiffness::Owned(Box::new(e.stiffness(axial[i]))),loads.member[i]).map(Some)).collect::<Result<Vec<_>>>().unwrap();
        let alternative=parallel_states();
        for (a,b) in states.iter().zip(&alternative) { assert_eq!(a.as_ref().unwrap().global_k(),b.as_ref().unwrap().global_k()); }
        json!({"kind":"stages","model":label,"threads":threads,"nodes":model.nodes.len(),"frames":model.frames.len(),"shells":model.shells.len(),
            "prepare":sample(|| { black_box(assembly::Prepared::new(model).unwrap()); },repeats),
            "loads":sample(|| { black_box(prep.loads(combo)); },repeats),
            "states_serial":sample(|| { black_box(prep.states(&loads,&axial,&active).unwrap()); },repeats),
            "states_parallel_trial":sample(|| { black_box(parallel_states()); },repeats),
            "assembly_and_factor":sample(|| { black_box(prep.assemble(model,&states).unwrap()); },repeats),
            "reduced_csc_build":sample(|| { black_box(faer::sparse::SparseColMat::try_new_from_triplets(system.free.len(),system.free.len(),&scaled).unwrap()); },repeats),
            "symbolic_analysis":sample(|| { black_box(faer::sparse::linalg::solvers::SymbolicLlt::try_new(k.symbolic(),faer::Side::Lower).unwrap()); },repeats),
            "numeric_factor":sample(|| { black_box(faer::sparse::linalg::solvers::Llt::try_new_with_symbolic(symbolic.clone(),k.as_ref(),faer::Side::Lower).unwrap()); },repeats),
            "force":sample(|| { black_box(prep.force(&loads,&states)); },repeats),
            "solve_and_residual":sample(|| { black_box(system.solve(&force).unwrap()); },repeats),
            "sparse_apply":sample(|| { black_box(system.apply(&u)); },repeats),
            "frame_recovery_serial":sample(|| { black_box(prep.frames.iter().zip(&states).map(|(e,s)| s.as_ref().unwrap().recover(e,&u)).collect::<Vec<_>>()); },repeats),
            "frame_recovery_parallel_trial":sample(|| { black_box(prep.frames.par_iter().zip(&states).map(|(e,s)| s.as_ref().unwrap().recover(e,&u)).collect::<Vec<_>>()); },repeats),
            "shell_recovery_serial":sample(|| { black_box(prep.shells.iter().enumerate().map(|(i,e)| e.recover(&u,loads.pressure[i])).collect::<Result<Vec<_>>>().unwrap()); },repeats),
            "shell_recovery_parallel_trial":sample(|| { black_box(prep.shells.par_iter().enumerate().map(|(i,e)| e.recover(&u,loads.pressure[i])).collect::<Result<Vec<_>>>().unwrap()); },repeats)
        })
    })
}

#[test]
fn parallelism_audit() {
    let mut rows = vec![
        json!({"kind":"environment","logical_cpus":std::thread::available_parallelism().unwrap().get(),"global_pool":rayon::current_num_threads(),"faer":format!("{:?}",faer::get_global_parallelism())}),
    ];
    let pools: Vec<_> = [1, 4, 8, 16]
        .into_iter()
        .map(|t| {
            rayon::ThreadPoolBuilder::new()
                .num_threads(t)
                .thread_name(move |i| format!("probe-{t}-{i}"))
                .build()
                .unwrap()
        })
        .collect();
    for (b, s, count) in [(6, 8, 1), (6, 8, 8), (6, 8, 32), (10, 16, 8)] {
        let model = frame(b, s, count);
        let opts = StaticOptions {
            max_in_flight: 1,
            ..Default::default()
        };
        let reference = pools[0].install(|| analyze_static(&model, &opts).unwrap());
        let expected = displacements(&reference);
        let scale = expected.iter().map(|x| x.abs()).fold(0.0, f64::max);
        for (pi, flight) in [(0, 1), (1, 4), (2, 4), (2, 8), (3, 4), (3, 16)] {
            let pool = &pools[pi];
            let opts = StaticOptions {
                max_in_flight: flight,
                ..Default::default()
            };
            let mut error = 0.0_f64;
            {
                let r = pool.install(|| analyze_static(&model, &opts).unwrap());
                let actual = displacements(&r);
                for (a, b) in actual.iter().zip(&expected) {
                    error = error.max((a - b).abs() / scale);
                }
                black_box(r);
            }
            assert!(error < 1e-10, "parallel displacement difference {error}");
            let timing = sample(
                || {
                    black_box(pool.install(|| analyze_static(&model, &opts).unwrap()));
                },
                5,
            );
            rows.push(json!({"kind":"static","model":format!("{b}x{b}x{s}"),"combos":count,"pool_threads":pool.current_num_threads(),"max_in_flight":flight,"timing":timing,"max_relative_displacement_diff":error}));
            println!("{}", rows.last().unwrap());
        }
        if count == 8 {
            let opts = StaticOptions {
                threads: 1,
                max_in_flight: 1,
                ..Default::default()
            };
            rows.push(json!({"kind":"current_threads_option","model":format!("{b}x{b}x{s}"),"combos":count,"threads":1,"timing":sample(|| {black_box(analyze_static(&model,&opts).unwrap());},5)}));
            let opts = StaticOptions {
                max_in_flight: 16,
                ..Default::default()
            };
            let result = pools[3].install(|| analyze_static(&model, &opts).unwrap());
            rows.push(json!({"kind":"serialization","model":format!("{b}x{b}x{s}"),"combos":count,"bytes":serde_json::to_vec(&result.combinations).unwrap().len(),"timing":sample(|| {black_box(serde_json::to_vec(&result.combinations).unwrap());},5)}));
        }
    }
    for (label, model) in [
        ("frame_6x6x8", frame(6, 8, 8)),
        ("frame_10x10x16", frame(10, 16, 8)),
        ("plate_32x32", plate(32)),
    ] {
        for t in [1, 8, 16] {
            let row = stages(&model, t, label);
            println!("{row}");
            rows.push(row);
        }
    }
    // A moderately sized P-Delta case; gravity reduced to avoid a buckling benchmark.
    let mut nonlinear = frame(4, 4, 8);
    for combo in &mut nonlinear.combinations {
        for (_, factor) in &mut combo.terms {
            *factor *= 0.01;
        }
    }
    for (pi, flight) in [(0, 1), (1, 4), (2, 8), (3, 16)] {
        let opts = StaticOptions {
            method: StaticMethod::PDelta,
            max_in_flight: flight,
            ..Default::default()
        };
        let result = pools[pi].install(|| analyze_static(&nonlinear, &opts).unwrap());
        let iterations: Vec<_> = result.combinations.iter().map(|r| r.iterations).collect();
        rows.push(json!({"kind":"p_delta","model":"4x4x4","combos":8,"pool_threads":pools[pi].current_num_threads(),"max_in_flight":flight,"iterations":iterations,"timing":sample(|| {black_box(pools[pi].install(|| analyze_static(&nonlinear,&opts).unwrap()));},5)}));
        println!("{}", rows.last().unwrap());
    }
    let modal_model = frame(4, 4, 1);
    for pi in [0, 2, 3] {
        let opts = ModalOptions {
            modes: 12,
            ..Default::default()
        };
        let row = json!({"kind":"modal","model":"4x4x4","modes":12,"pool_threads":pools[pi].current_num_threads(),"timing":sample(|| {black_box(pools[pi].install(|| analyze_modal(&modal_model,&opts).unwrap()));},3)});
        println!("{row}");
        rows.push(row);
    }
    let output = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/parallelism-audit.json");
    std::fs::write(&output, serde_json::to_string_pretty(&rows).unwrap()).unwrap();
    println!("AUDIT_OUTPUT={}", output.display());
}
