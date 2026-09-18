//! Pure compilation of the editable model into solver input, with a two-way
//! mapping so results can be attached back to entities.
use crate::{entity::*, model::*};
use oa_core::units::Length;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A validation problem tied to an entity when one is known.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Problem {
    pub entity: Option<EntityId>,
    pub name: Option<String>,
    pub message: String,
}
impl std::fmt::Display for Problem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (&self.name, self.entity) {
            (Some(n), Some(id)) => write!(f, "{n:?} (#{}): {}", id.0, self.message),
            _ => f.write_str(&self.message),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Mapping {
    pub node_index: BTreeMap<EntityId, usize>,
    /// Solver node index to entity; `None` for synthetic diaphragm masters.
    pub node_id: Vec<Option<EntityId>>,
    pub material_index: BTreeMap<EntityId, usize>,
    pub section_index: BTreeMap<EntityId, usize>,
    pub frame_index: BTreeMap<EntityId, usize>,
    pub frame_id: Vec<EntityId>,
    pub shell_index: BTreeMap<EntityId, usize>,
    pub shell_id: Vec<EntityId>,
    pub load_case_index: BTreeMap<EntityId, usize>,
    pub combination_index: BTreeMap<EntityId, usize>,
    /// Diaphragm entity and the solver node index created as its master.
    pub synthetic_masters: Vec<(EntityId, usize)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Compiled {
    pub solver: oa_core::Model,
    pub mapping: Mapping,
}
impl Compiled {
    pub fn content_hash(&self) -> String {
        self.solver.content_hash()
    }
    /// Solver indices for the members of a group, by kind.
    pub fn group_indices(&self, model: &Model, group: EntityId) -> GroupIndices {
        let mut out = GroupIndices::default();
        for id in model.group_members(group) {
            if let Some(&i) = self.mapping.node_index.get(&id) {
                out.nodes.push(i);
            } else if let Some(&i) = self.mapping.frame_index.get(&id) {
                out.frames.push(i);
            } else if let Some(&i) = self.mapping.shell_index.get(&id) {
                out.shells.push(i);
            }
        }
        out
    }
}
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GroupIndices {
    pub nodes: Vec<usize>,
    pub frames: Vec<usize>,
    pub shells: Vec<usize>,
}

fn check_names<T: Entity>(model: &Model, problems: &mut Vec<Problem>) {
    let mut seen = std::collections::HashMap::new();
    for (id, e) in T::table(model) {
        if e.name().trim().is_empty() {
            problems.push(Problem {
                entity: Some(*id),
                name: None,
                message: format!("{} #{} has no name", T::KIND, id.0),
            });
        } else if let Some(first) = seen.insert(e.name(), *id) {
            problems.push(Problem {
                entity: Some(*id),
                name: Some(e.name().into()),
                message: format!(
                    "{} name {:?} is also used by #{}",
                    T::KIND,
                    e.name(),
                    first.0
                ),
            });
        }
    }
}
fn check_references<T: Entity>(model: &Model, problems: &mut Vec<Problem>) {
    for (id, e) in T::table(model) {
        for (target, kind) in e.references() {
            match model.kind_of(target) {
                Some(k) if k == kind => {}
                Some(_) => problems.push(Problem {
                    entity: Some(*id),
                    name: Some(e.name().into()),
                    message: format!(
                        "references {} but a {kind} was expected",
                        model.describe(target)
                    ),
                }),
                None => problems.push(Problem {
                    entity: Some(*id),
                    name: Some(e.name().into()),
                    message: format!("references missing {kind} #{}", target.0),
                }),
            }
        }
    }
}

/// Validates the model and produces solver input. Returns every problem
/// found rather than stopping at the first.
/// Length of a frame from its nodes' positions, as the solver computes it;
/// zero when a node is missing, which validation reports separately.
fn frame_length(model: &Model, frame: EntityId) -> f64 {
    let Some(f) = model.frames.get(&frame) else {
        return 0.0;
    };
    let (Some(a), Some(b)) = (model.nodes.get(&f.nodes[0]), model.nodes.get(&f.nodes[1])) else {
        return 0.0;
    };
    (0..3)
        .map(|i| (b.position[i].si() - a.position[i].si()).powi(2))
        .sum::<f64>()
        .sqrt()
}

/// A position along a member, moved onto the member's end (or start) when
/// it lies within roundoff of it. Coordinates and load positions are
/// converted from display units independently, so a full-span load can
/// land a few ulps past the length the solver computes and fail its exact
/// range check.
fn snap_to_span(x: oa_core::units::Length, span: f64) -> oa_core::units::Length {
    let tolerance = 1e-9 * span.abs().max(1e-3);
    let v = x.si();
    if (v - span).abs() <= tolerance {
        oa_core::units::Length::from_si(span)
    } else if v.abs() <= tolerance {
        oa_core::units::Length::ZERO
    } else {
        x
    }
}

/// Levels are not solver input, but every node binds to one, so the datums
/// must exist, be finite, and be distinct.
fn check_levels(model: &Model, problems: &mut Vec<Problem>) {
    if model.levels.is_empty() {
        problems.push(Problem {
            entity: None,
            name: None,
            message: "model has no levels".into(),
        });
    }
    for (id, l) in &model.levels {
        if !l.elevation.si().is_finite() {
            problems.push(Problem {
                entity: Some(*id),
                name: Some(l.name.clone()),
                message: "elevation is not finite".into(),
            });
        }
    }
    let order = model.levels_by_elevation();
    for pair in order.windows(2) {
        let (a, b) = (&model.levels[&pair[0]], &model.levels[&pair[1]]);
        if (b.elevation.si() - a.elevation.si()).abs() <= crate::levels::TOLERANCE {
            problems.push(Problem {
                entity: Some(pair[1]),
                name: Some(b.name.clone()),
                message: format!("sits at the same elevation as {}", model.describe(pair[0])),
            });
        }
    }
}

pub fn compile(model: &Model) -> Result<Compiled, Vec<Problem>> {
    let mut problems = vec![];
    check_names::<Level>(model, &mut problems);
    check_levels(model, &mut problems);
    check_names::<Node>(model, &mut problems);
    check_names::<Material>(model, &mut problems);
    check_names::<Section>(model, &mut problems);
    check_names::<Frame>(model, &mut problems);
    check_names::<Shell>(model, &mut problems);
    check_names::<Diaphragm>(model, &mut problems);
    check_names::<LoadCase>(model, &mut problems);
    check_names::<Combination>(model, &mut problems);
    check_names::<Group>(model, &mut problems);
    check_references::<Node>(model, &mut problems);
    check_references::<Frame>(model, &mut problems);
    check_references::<Shell>(model, &mut problems);
    check_references::<Diaphragm>(model, &mut problems);
    check_references::<LoadCase>(model, &mut problems);
    check_references::<Combination>(model, &mut problems);
    for id in model.duplicate_ids() {
        problems.push(Problem {
            entity: Some(id),
            name: None,
            message: format!("id #{} is used by more than one entity", id.0),
        });
    }
    if !problems.is_empty() {
        return Err(problems);
    }
    let mut mapping = Mapping::default();
    let mut solver = oa_core::Model {
        gravity: model.gravity,
        ..Default::default()
    };
    for (id, n) in &model.nodes {
        mapping.node_index.insert(*id, solver.nodes.len());
        mapping.node_id.push(Some(*id));
        solver.nodes.push(oa_core::Node {
            position: n.position,
            restrained: n.restrained,
            prescribed: n.prescribed.clone(),
            mass: n.mass,
            mass_inertia: n.mass_inertia,
            spring_translation: n.spring_translation,
            spring_rotation: n.spring_rotation,
        });
    }
    for (id, m) in &model.materials {
        mapping.material_index.insert(*id, solver.materials.len());
        solver.materials.push(oa_core::Material {
            young: m.young,
            poisson: m.poisson,
            density: m.density,
        });
    }
    for (id, s) in &model.sections {
        mapping.section_index.insert(*id, solver.sections.len());
        solver.sections.push(oa_core::Section {
            area: s.area,
            iy: s.iy,
            iz: s.iz,
            torsion: s.torsion,
        });
    }
    let node = |id: &EntityId| oa_core::NodeId(mapping.node_index[id]);
    for (id, f) in &model.frames {
        mapping.frame_index.insert(*id, solver.frames.len());
        mapping.frame_id.push(*id);
        solver.frames.push(oa_core::Frame {
            nodes: [node(&f.nodes[0]), node(&f.nodes[1])],
            material: oa_core::MaterialId(mapping.material_index[&f.material]),
            section: oa_core::SectionId(mapping.section_index[&f.section]),
            local_y: f.local_y,
            roll: f.roll,
            releases: f.releases,
            behavior: f.behavior,
        });
    }
    for (id, s) in &model.shells {
        mapping.shell_index.insert(*id, solver.shells.len());
        mapping.shell_id.push(*id);
        solver.shells.push(oa_core::Shell {
            nodes: s.nodes.map(|n| node(&n)),
            material: oa_core::MaterialId(mapping.material_index[&s.material]),
            thickness: s.thickness,
            formulation: s.formulation,
            drilling_ratio: s.drilling_ratio,
        });
    }
    for (id, d) in &model.diaphragms {
        let master = match d.master {
            Some(m) => node(&m),
            None => {
                // Mass-weighted centroid of the slaves, plain centroid when massless.
                let mut weight = 0.0;
                let mut sum = [0.0; 3];
                let mut plain = [0.0; 3];
                for s in &d.nodes {
                    let n = &model.nodes[s];
                    let w: f64 = n.mass.iter().map(|m| m.si()).sum();
                    for a in 0..3 {
                        sum[a] += w * n.position[a].si();
                        plain[a] += n.position[a].si();
                    }
                    weight += w;
                }
                let count = d.nodes.len() as f64;
                let position = std::array::from_fn(|a| {
                    Length::from_si(if weight > 0.0 {
                        sum[a] / weight
                    } else {
                        plain[a] / count
                    })
                });
                let index = solver.nodes.len();
                solver.nodes.push(oa_core::Node::new(position));
                mapping.node_id.push(None);
                mapping.synthetic_masters.push((*id, index));
                oa_core::NodeId(index)
            }
        };
        solver.diaphragms.push(oa_core::Diaphragm {
            master,
            nodes: d.nodes.iter().map(node).collect(),
            normal: d.normal,
        });
    }
    for (id, c) in &model.load_cases {
        mapping.load_case_index.insert(*id, solver.load_cases.len());
        solver.load_cases.push(oa_core::LoadCase {
            name: c.name.clone(),
            self_weight: c.self_weight,
            nodal: c
                .nodal
                .iter()
                .map(|l| oa_core::NodalLoad {
                    node: node(&l.node),
                    force: l.force,
                    moment: l.moment,
                })
                .collect(),
            member: c
                .member
                .iter()
                .map(|l| {
                    let span = frame_length(model, l.member());
                    let along = |x: &oa_core::units::Length| snap_to_span(*x, span);
                    match l {
                        MemberLoad::Point {
                            member,
                            position,
                            force,
                            moment,
                            axes,
                        } => oa_core::MemberLoad::Point {
                            member: oa_core::FrameId(mapping.frame_index[member]),
                            position: along(position),
                            force: *force,
                            moment: *moment,
                            axes: *axes,
                        },
                        MemberLoad::Distributed {
                            member,
                            start,
                            end,
                            start_load,
                            end_load,
                            axes,
                        } => oa_core::MemberLoad::Distributed {
                            member: oa_core::FrameId(mapping.frame_index[member]),
                            start: along(start),
                            end: along(end),
                            start_load: *start_load,
                            end_load: *end_load,
                            axes: *axes,
                        },
                    }
                })
                .collect(),
            surface: c
                .surface
                .iter()
                .map(|l| oa_core::SurfaceLoad {
                    shell: oa_core::ShellId(mapping.shell_index[&l.shell]),
                    pressure: l.pressure,
                })
                .collect(),
        });
    }
    for (id, c) in &model.combinations {
        mapping
            .combination_index
            .insert(*id, solver.combinations.len());
        solver.combinations.push(oa_core::LoadCombination {
            name: c.name.clone(),
            terms: c
                .terms
                .iter()
                .map(|(case, f)| (oa_core::LoadCaseId(mapping.load_case_index[case]), *f))
                .collect(),
        });
    }
    if let Err(e) = solver.validate() {
        return Err(vec![Problem {
            entity: None,
            name: None,
            message: e.to_string(),
        }]);
    }
    // Element geometry is checked here too, so a degenerate shell or an
    // unusable frame orientation is a named problem at compile time rather
    // than a surprise when analysis starts.
    let mut problems = vec![];
    for (index, id) in mapping.frame_id.iter().enumerate() {
        if let Err(e) = solver.validate_frame(index) {
            problems.push(element_problem(model, *id, &e, &format!("frame {index}: ")));
        }
    }
    for (index, id) in mapping.shell_id.iter().enumerate() {
        if let Err(e) = solver.validate_shell(index) {
            problems.push(element_problem(model, *id, &e, &format!("shell {index}: ")));
        }
    }
    if !problems.is_empty() {
        return Err(problems);
    }
    Ok(Compiled { solver, mapping })
}

/// Ties a solver element error back to the entity it came from, dropping
/// the solver's index prefix since the problem already names the entity.
fn element_problem(model: &Model, id: EntityId, error: &oa_core::Error, prefix: &str) -> Problem {
    let message = match error {
        oa_core::Error::Model(m) => m.clone(),
        other => other.to_string(),
    };
    Problem {
        entity: Some(id),
        name: model.name_of(id).map(str::to_string),
        message: message.strip_prefix(prefix).unwrap_or(&message).to_string(),
    }
}
