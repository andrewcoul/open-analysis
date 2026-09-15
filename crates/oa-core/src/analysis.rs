use crate::{
    Error, Result,
    assembly::{Loads, Prepared, SparseSystem},
    element::frame::{FrameState, Stiffness},
    exec::Exec,
    model::*,
    results::*,
};
#[cfg(feature = "parallel")]
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StaticMethod {
    #[default]
    Linear,
    Nonlinear,
    PDelta,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct OutputSelection {
    pub displacements: bool,
    pub reactions: bool,
    pub frames: bool,
    pub shells: bool,
}
impl Default for OutputSelection {
    fn default() -> Self {
        Self {
            displacements: true,
            reactions: true,
            frames: true,
            shells: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct StaticOptions {
    pub method: StaticMethod,
    /// Empty selects all defined combinations (or each case if none defined).
    pub combinations: Vec<String>,
    pub max_iterations: usize,
    pub tolerance: f64,
    /// Absolute axial-force threshold for activation, in newtons.
    pub force_tolerance: crate::units::Force,
    /// Bound on combinations started but not yet consumed: running solves plus
    /// finished results waiting for their turn in the ordered consumer.
    pub max_in_flight: usize,
    /// Worker count for the whole run, element preparation and factorization
    /// included. Zero uses the current Rayon pool. Ignored in a serial/WASM build.
    pub threads: usize,
    pub outputs: OutputSelection,
}
impl Default for StaticOptions {
    fn default() -> Self {
        Self {
            method: StaticMethod::Linear,
            combinations: vec![],
            max_iterations: 100,
            tolerance: 1e-8,
            force_tolerance: crate::units::Force::from_si(1e-6),
            max_in_flight: 4,
            threads: 0,
            outputs: OutputSelection::default(),
        }
    }
}

pub fn analyze_static(model: &Model, options: &StaticOptions) -> Result<InMemoryResults> {
    let mut results = InMemoryResults::default();
    analyze_static_into(model, options, &mut results)?;
    Ok(results)
}

pub fn analyze_static_into(
    model: &Model,
    options: &StaticOptions,
    consumer: &mut dyn ResultConsumer,
) -> Result<()> {
    if options.max_iterations == 0
        || options.max_in_flight == 0
        || !options.tolerance.is_finite()
        || options.tolerance <= 0.0
        || options.tolerance >= 1.0
        || !options.force_tolerance.si().is_finite()
        || options.force_tolerance.si() < 0.0
    {
        return Err(Error::Request(
            "invalid iteration, tolerance, or batching settings".into(),
        ));
    }
    let exec = Exec::new(options.threads)?;
    // Preparation and the shared linear system run inside the selected pool
    // so parallel element loops and faer's kernels honour the thread budget.
    let (prep, combos, shared) =
        exec.install(|| -> Result<_> {
            let mut prep = Prepared::new(model)?;
            if options.method == StaticMethod::Linear
                && model
                    .frames
                    .iter()
                    .any(|m| m.behavior != AxialBehavior::Both)
            {
                return Err(Error::Request(
                    "use nonlinear or p_delta for tension/compression-only members".into(),
                ));
            }
            let all = model.effective_combinations();
            let combos =
                if options.combinations.is_empty() {
                    all
                } else {
                    let mut names = std::collections::HashSet::new();
                    let mut selected = vec![];
                    for name in &options.combinations {
                        if !names.insert(name) {
                            return Err(Error::Request(format!(
                                "duplicate requested combination {name:?}"
                            )));
                        }
                        selected.push(all.iter().find(|c| &c.name == name).cloned().ok_or_else(
                            || Error::Request(format!("unknown combination {name:?}")),
                        )?);
                    }
                    selected
                };
            if combos.is_empty() {
                return Err(Error::Request(
                    "static analysis needs at least one load case".into(),
                ));
            }
            prep.cache_case_loads(model, &combos);
            let shared = if options.method == StaticMethod::Linear {
                Some(prep.assemble(model, &prep.elastic_states()?)?)
            } else {
                None
            };
            Ok((prep, combos, shared))
        })?;
    let work = |i: usize| solve_combination(model, &prep, &combos[i], options, shared.as_ref());
    // The consumer stays on this calling thread, so it need not be Send or
    // Sync, and it runs while the pool solves the next combinations.
    #[cfg(feature = "parallel")]
    {
        use std::sync::atomic::{AtomicBool, Ordering};
        if rayon::current_thread_index().is_some() {
            // Already inside a pool (an analysis launched from `install` or a
            // parallel iterator): a worker blocked on a channel looks busy to
            // Rayon's scheduler and starves the pool, so fall back to ordered
            // batches that this thread helps compute.
            let indices: Vec<usize> = (0..combos.len()).collect();
            for batch in indices.chunks(options.max_in_flight) {
                let completed =
                    exec.install(|| batch.par_iter().map(|&i| work(i)).collect::<Vec<_>>());
                for result in completed {
                    consumer.consume(result?)?;
                }
            }
            return Ok(());
        }
        let stop = AtomicBool::new(false);
        let (tx, rx) = std::sync::mpsc::channel::<(usize, Result<CombinationResult>)>();
        exec.in_place_scope(|scope| {
            let mut next = 0;
            let mut expected = 0;
            let mut pending = std::collections::BTreeMap::<usize, Result<CombinationResult>>::new();
            loop {
                while let Some(result) = pending.remove(&expected) {
                    if let Err(e) = result.and_then(|r| consumer.consume(r)) {
                        stop.store(true, Ordering::Relaxed);
                        return Err(e);
                    }
                    expected += 1;
                }
                if expected == combos.len() {
                    return Ok(());
                }
                // Keep started-but-unconsumed work within the bound; a result
                // waiting for its turn holds a slot just like a running solve.
                while next < combos.len() && next - expected < options.max_in_flight {
                    let i = next;
                    let tx = tx.clone();
                    let (work, stop) = (&work, &stop);
                    scope.spawn(move |_| {
                        let result = if stop.load(Ordering::Relaxed) {
                            Ok(Err(Error::Request("analysis stopped".into())))
                        } else {
                            // A worker that unwinds must still report, or the
                            // receiver below waits forever: the scope cannot
                            // re-raise the panic until this closure returns.
                            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| work(i)))
                        };
                        match result {
                            Ok(result) => {
                                let _ = tx.send((i, result));
                            }
                            Err(payload) => {
                                let _ = tx.send((
                                    i,
                                    Err(Error::Solver("combination worker panicked".into())),
                                ));
                                std::panic::resume_unwind(payload);
                            }
                        }
                    });
                    next += 1;
                }
                // Combination `expected` is neither consumed nor pending, so a
                // worker owns it and a message is guaranteed to arrive.
                let (i, result) = receive(&rx)?;
                pending.insert(i, result);
            }
        })
    }
    #[cfg(not(feature = "parallel"))]
    {
        for i in 0..combos.len() {
            consumer.consume(work(i)?)?;
        }
        Ok(())
    }
}

/// Waits for the next finished combination on a thread outside the pool.
#[cfg(feature = "parallel")]
fn receive(
    rx: &std::sync::mpsc::Receiver<(usize, Result<CombinationResult>)>,
) -> Result<(usize, Result<CombinationResult>)> {
    rx.recv()
        .map_err(|_| Error::Solver("combination worker vanished".into()))
}

fn solve_combination(
    model: &Model,
    prep: &Prepared,
    combo: &LoadCombination,
    options: &StaticOptions,
    shared: Option<&SparseSystem>,
) -> Result<CombinationResult> {
    let loads = prep.loads(model, combo);
    let mut active = vec![true; prep.frames.len()];
    let mut axial = vec![0.0; prep.frames.len()];
    let mut previous_u = vec![0.0; prep.ndof];
    let mut last_residual = f64::INFINITY;
    for iteration in 1..=options.max_iterations {
        let states = prep.states(&loads, &axial, &active)?;
        let owned;
        let system = if let Some(s) = shared {
            s
        } else {
            owned = prep.assemble(model, &states)?;
            &owned
        };
        let force = prep.force(&loads, &states);
        let (u, reaction, linear_residual) = system.solve(&force)?;
        // Nonlinear activation needs member forces even when frame output is off.
        let frames = if options.method == StaticMethod::Linear && !options.outputs.frames {
            vec![]
        } else {
            recover_frames(prep, &states, &u)
        };
        if options.method == StaticMethod::Linear {
            return package(
                prep,
                combo,
                options,
                iteration,
                linear_residual,
                &u,
                &reaction,
                frames,
                &loads,
            );
        }
        let mut next_active = active.clone();
        let mut next_axial = vec![0.0; axial.len()];
        for (i, member) in model.frames.iter().enumerate() {
            let f = if active[i] {
                frames[i].local_end_forces
            } else {
                let trial =
                    prep.frames[i].state(Stiffness::Shared(&prep.elastic[i]), loads.member[i])?;
                let (_, f) = trial.recover(&prep.frames[i], &u);
                std::array::from_fn(|j| f[j])
            };
            let n = (f[6] - f[0]) / 2.0;
            match member.behavior {
                AxialBehavior::Both => {}
                AxialBehavior::TensionOnly => {
                    if n < -options.force_tolerance.si() {
                        next_active[i] = false;
                    } else if n > options.force_tolerance.si() {
                        next_active[i] = true;
                    }
                }
                AxialBehavior::CompressionOnly => {
                    if n > options.force_tolerance.si() {
                        next_active[i] = false;
                    } else if n < -options.force_tolerance.si() {
                        next_active[i] = true;
                    }
                }
            }
            if next_active[i] && options.method == StaticMethod::PDelta {
                next_axial[i] = n;
            }
        }
        // Check equilibrium against the updated stiffness state, not only displacement change.
        let updated = prep.states(&loads, &next_axial, &next_active)?;
        let next_force = prep.force(&loads, &updated);
        let mut internal = vec![0.0; prep.ndof];
        for (e, s) in prep.frames.iter().zip(&updated) {
            if let Some(s) = s {
                let d = crate::element::frame::V12::from_fn(|i, _| u[e.dofs[i]]);
                let f = s.global_k() * d;
                for i in 0..12 {
                    internal[e.dofs[i]] += f[i];
                }
            }
        }
        for (e, k) in prep.shells.iter().zip(&prep.shell_k) {
            let d = crate::element::shell::V24::from_fn(|i, _| u[e.dofs[i]]);
            let f = k * d;
            for i in 0..24 {
                internal[e.dofs[i]] += f[i];
            }
        }
        // Grounded springs are part of the solved stiffness, so their force
        // belongs in the internal vector too; leaving it out reports a
        // constant spurious imbalance of k_spring * u.
        for (i, &k) in prep.springs.iter().enumerate() {
            if k > 0.0 {
                internal[i] += k * u[i];
            }
        }
        // Equilibrium holds in the reduced space; constraint forces cancel under Tᵀ.
        let out_of_balance: Vec<f64> = internal
            .iter()
            .zip(&next_force)
            .map(|(a, b)| a - b)
            .collect();
        let residual = crate::assembly::norm(&system.reduce(&out_of_balance));
        let force_norm = crate::assembly::norm(&system.reduce(&next_force)).max(1.0);
        let du = u
            .iter()
            .zip(&previous_u)
            .map(|(a, b)| (a - b).powi(2))
            .sum::<f64>()
            .sqrt();
        let unorm = u.iter().map(|v| v * v).sum::<f64>().sqrt().max(1e-12);
        last_residual = residual / force_norm;
        if active == next_active
            && last_residual <= options.tolerance
            && (iteration > 1 && du / unorm <= options.tolerance
                || options.method == StaticMethod::Nonlinear)
        {
            // Output the same stiffness state whose displacement was solved.
            return package(
                prep,
                combo,
                options,
                iteration,
                last_residual.max(linear_residual),
                &u,
                &reaction,
                frames,
                &loads,
            );
        }
        previous_u = u;
        active = next_active;
        axial = next_axial;
    }
    Err(Error::NonConvergence {
        combination: combo.name.clone(),
        iterations: options.max_iterations,
        residual: last_residual,
    })
}

pub(crate) fn recover_frames(
    prep: &Prepared,
    states: &[Option<FrameState>],
    u: &[f64],
) -> Vec<FrameResult> {
    prep.frames
        .iter()
        .zip(states)
        .map(|(e, s)| {
            if let Some(s) = s {
                let (d, f) = s.recover(e, u);
                FrameResult {
                    active: true,
                    local_end_forces: std::array::from_fn(|i| f[i]),
                    local_displacements: std::array::from_fn(|i| d[i]),
                }
            } else {
                FrameResult {
                    active: false,
                    local_end_forces: [0.0; 12],
                    local_displacements: [0.0; 12],
                }
            }
        })
        .collect()
}
#[allow(clippy::too_many_arguments)]
fn package(
    prep: &Prepared,
    combo: &LoadCombination,
    options: &StaticOptions,
    iterations: usize,
    residual: f64,
    u: &[f64],
    reaction: &[f64],
    frames: Vec<FrameResult>,
    loads: &Loads,
) -> Result<CombinationResult> {
    let shells = if options.outputs.shells {
        Some(
            prep.shells
                .iter()
                .enumerate()
                .map(|(i, e)| e.recover(u, loads.pressure[i]))
                .collect::<Result<Vec<_>>>()?,
        )
    } else {
        None
    };
    Ok(CombinationResult {
        combination: combo.name.clone(),
        iterations,
        relative_residual: residual,
        displacements: options.outputs.displacements.then(|| node_values(u)),
        reactions: options.outputs.reactions.then(|| node_values(reaction)),
        frames: options.outputs.frames.then_some(frames),
        shells,
    })
}
pub(crate) fn node_values(values: &[f64]) -> Vec<[f64; 6]> {
    values
        .chunks_exact(6)
        .map(|c| std::array::from_fn(|i| c[i]))
        .collect()
}
