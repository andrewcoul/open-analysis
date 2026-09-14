use crate::{
    Error, Result,
    assembly::{Loads, Prepared, SparseSystem},
    element::frame::FrameState,
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
    /// Hard bound on results and combination-specific systems in flight.
    pub max_in_flight: usize,
    /// Zero uses the current Rayon pool. Ignored in a serial/WASM build.
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
    // The consumer stays on this calling thread. Only the bounded batch calculation
    // runs in the Rayon pool, so consumers need not implement Send or Sync.
    #[cfg(feature = "parallel")]
    let pool = if options.threads > 0 {
        Some(
            rayon::ThreadPoolBuilder::new()
                .num_threads(options.threads)
                .build()
                .map_err(|e| Error::Request(e.to_string()))?,
        )
    } else {
        None
    };
    let prep = Prepared::new(model)?;
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
    let combos = if options.combinations.is_empty() {
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
            selected.push(
                all.iter()
                    .find(|c| &c.name == name)
                    .cloned()
                    .ok_or_else(|| Error::Request(format!("unknown combination {name:?}")))?,
            );
        }
        selected
    };
    if combos.is_empty() {
        return Err(Error::Request(
            "static analysis needs at least one load case".into(),
        ));
    }
    let shared = if options.method == StaticMethod::Linear {
        let zero = Loads {
            nodal: vec![0.0; prep.ndof],
            member: vec![Default::default(); prep.frames.len()],
            pressure: vec![0.0; prep.shells.len()],
        };
        let states = prep.states(
            &zero,
            &vec![0.0; prep.frames.len()],
            &vec![true; prep.frames.len()],
        )?;
        Some(prep.assemble(model, &states)?)
    } else {
        None
    };
    for batch in combos.chunks(options.max_in_flight) {
        let calculate = || {
            #[cfg(feature = "parallel")]
            let iter = batch.par_iter();
            #[cfg(not(feature = "parallel"))]
            let iter = batch.iter();
            iter.map(|combo| solve_combination(model, &prep, combo, options, shared.as_ref()))
                .collect::<Vec<_>>()
        };
        #[cfg(feature = "parallel")]
        let completed = if let Some(pool) = &pool {
            pool.install(calculate)
        } else {
            calculate()
        };
        #[cfg(not(feature = "parallel"))]
        let completed = calculate();
        for result in completed {
            consumer.consume(result?)?;
        }
    }
    Ok(())
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
        let frames = recover_frames(prep, &states, &u);
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
                let trial = prep.frames[i].state(0.0, loads.member[i])?;
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
                let f = s.global_k * d;
                for i in 0..12 {
                    internal[e.dofs[i]] += f[i];
                }
            }
        }
        for e in &prep.shells {
            let d = crate::element::shell::V24::from_fn(|i, _| u[e.dofs[i]]);
            let f = e.global_k() * d;
            for i in 0..24 {
                internal[e.dofs[i]] += f[i];
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
