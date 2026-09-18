//! Level arithmetic with no state of its own: the order of the datums, which
//! objects belong to a level's floor, and the plan for moving a datum with
//! the nodes bound to it. Z is the structural vertical; a level is a plane
//! of constant Z. See docs/model/LEVEL_SYSTEMS.md.
use crate::{command::*, entity::*, model::*};
use oa_core::units::Length;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Two datums closer than this, in metres, count as coincident.
pub const TOLERANCE: f64 = 1e-6;

/// Which datums an elevation change carries with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ElevationScope {
    /// Move this datum and its bound nodes; every other datum stays.
    ThisLevel,
    /// Move this datum and every higher one, with their nodes, by the same
    /// amount, so the storey heights above are preserved.
    ThisAndAbove,
}

/// A node's height above its level, in metres. Positive is upward.
pub fn offset(node: &Node, level: &Level) -> f64 {
    node.position[2].si() - level.elevation.si()
}

/// The datum just below and just above a level, by elevation.
pub fn neighbours(model: &Model, id: EntityId) -> (Option<EntityId>, Option<EntityId>) {
    let order = model.levels_by_elevation();
    let Some(ix) = order.iter().position(|l| *l == id) else {
        return (None, None);
    };
    let below = ix.checked_sub(1).map(|i| order[i]);
    let above = order.get(ix + 1).copied();
    (below, above)
}

/// The storey height from the level below, in metres; none for the lowest level.
pub fn height_below(model: &Model, id: EntityId) -> Option<f64> {
    let (below, _) = neighbours(model, id);
    let below = below?;
    Some(model.levels[&id].elevation.si() - model.levels[&below].elevation.si())
}

/// The objects a level's plan view shows. Floor objects are those whose
/// nodes all bind to the level, whatever their offsets. Spanning objects
/// have some node on the level, or cross its plane between their nodes,
/// and are shown as intersections rather than as members of the floor.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Membership {
    pub nodes: BTreeSet<EntityId>,
    pub frames: BTreeSet<EntityId>,
    pub shells: BTreeSet<EntityId>,
    pub spanning_frames: BTreeSet<EntityId>,
    pub spanning_shells: BTreeSet<EntityId>,
}

pub fn membership(model: &Model, level: EntityId) -> Membership {
    let mut out = Membership::default();
    let Some(datum) = model.levels.get(&level) else {
        return out;
    };
    let elevation = datum.elevation.si();
    out.nodes = model
        .nodes
        .iter()
        .filter(|(_, n)| n.level == level)
        .map(|(id, _)| *id)
        .collect();
    let classify = |nodes: &[EntityId]| -> Option<bool> {
        let bound = nodes.iter().filter(|n| out.nodes.contains(n)).count();
        if bound == nodes.len() {
            return Some(true);
        }
        if bound > 0 {
            return Some(false);
        }
        let zs: Vec<f64> = nodes
            .iter()
            .filter_map(|n| model.nodes.get(n))
            .map(|n| n.position[2].si())
            .collect();
        if zs.len() != nodes.len() {
            return None;
        }
        let (lo, hi) = zs
            .iter()
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), z| {
                (lo.min(*z), hi.max(*z))
            });
        (lo < elevation - TOLERANCE && hi > elevation + TOLERANCE).then_some(false)
    };
    for (id, f) in &model.frames {
        match classify(&f.nodes) {
            Some(true) => {
                out.frames.insert(*id);
            }
            Some(false) => {
                out.spanning_frames.insert(*id);
            }
            None => {}
        }
    }
    for (id, s) in &model.shells {
        match classify(&s.nodes) {
            Some(true) => {
                out.shells.insert(*id);
            }
            Some(false) => {
                out.spanning_shells.insert(*id);
            }
            None => {}
        }
    }
    out
}

/// A validated elevation change, ready to apply as one batch: the levels
/// moved, the nodes carried with them, and the shift in metres.
#[derive(Debug, Clone, PartialEq)]
pub struct MovePlan {
    pub commands: Vec<Command>,
    pub levels: Vec<EntityId>,
    pub nodes: Vec<EntityId>,
    pub delta: f64,
}

fn level_of(model: &Model, id: EntityId) -> Result<&Level> {
    model
        .levels
        .get(&id)
        .ok_or_else(|| match model.kind_of(id) {
            Some(actual) => ModelError::WrongKind {
                entity: model.describe(id),
                expected: EntityKind::Level,
                actual,
            },
            None => ModelError::NotFound(id),
        })
}

/// Plans moving `id` to `elevation`. Bound nodes keep their offsets, other
/// levels' nodes stay put, and the move is refused if it would cross or
/// coincide with a level that is not moving, or make geometry invalid that
/// was valid before.
pub fn plan_set_elevation(
    model: &Model,
    id: EntityId,
    elevation: Length,
    scope: ElevationScope,
) -> Result<MovePlan> {
    let level = level_of(model, id)?;
    if !elevation.si().is_finite() {
        return Err(ModelError::Invalid("level elevation must be finite".into()));
    }
    let old = level.elevation.si();
    let delta = elevation.si() - old;
    let mut moved: Vec<EntityId> = match scope {
        ElevationScope::ThisLevel => vec![id],
        ElevationScope::ThisAndAbove => model
            .levels
            .iter()
            .filter(|(_, l)| l.elevation.si() >= old - TOLERANCE)
            .map(|(l, _)| *l)
            .collect(),
    };
    if delta == 0.0 {
        return Ok(MovePlan {
            commands: vec![],
            levels: moved,
            nodes: vec![],
            delta,
        });
    }
    for (other, fixed) in model.levels.iter().filter(|(l, _)| !moved.contains(l)) {
        for m in &moved {
            let before = model.levels[m].elevation.si() - fixed.elevation.si();
            let after = before + delta;
            if after.abs() <= TOLERANCE || (before > 0.0) != (after > 0.0) {
                return Err(ModelError::Invalid(format!(
                    "moving {} would put it at or past {}; move the levels above with it, or move them first",
                    model.describe(*m),
                    model.describe(*other)
                )));
            }
        }
    }
    // Highest first when going up, lowest first when going down, so no two
    // moved levels coincide on the way.
    moved.sort_by(|a, b| {
        let (ea, eb) = (
            model.levels[a].elevation.si(),
            model.levels[b].elevation.si(),
        );
        if delta > 0.0 {
            eb.total_cmp(&ea)
        } else {
            ea.total_cmp(&eb)
        }
    });
    let mut commands = vec![];
    for m in &moved {
        let mut level = model.levels[m].clone();
        level.elevation = if *m == id {
            elevation
        } else {
            Length::from_si(level.elevation.si() + delta)
        };
        commands.push(Command::UpdateLevel { id: *m, level });
    }
    let mut nodes = vec![];
    for (nid, n) in &model.nodes {
        if !moved.contains(&n.level) {
            continue;
        }
        let mut node = n.clone();
        node.position[2] = Length::from_si(node.position[2].si() + delta);
        commands.push(Command::UpdateNode { id: *nid, node });
        nodes.push(*nid);
    }
    let mut after = model.clone();
    Command::Batch {
        commands: commands.clone(),
    }
    .apply(&mut after)?;
    let newly_invalid = newly_invalid(model, &after, &nodes.iter().copied().collect());
    if !newly_invalid.is_empty() {
        return Err(ModelError::Invalid(format!(
            "the move would make geometry invalid: {}",
            newly_invalid.join("; ")
        )));
    }
    Ok(MovePlan {
        commands,
        levels: moved,
        nodes,
        delta,
    })
}

/// Plans a storey-height change: the level moves to sit `height` above the
/// level below it, carrying every higher level so their heights are kept.
pub fn plan_set_height_below(model: &Model, id: EntityId, height: Length) -> Result<MovePlan> {
    level_of(model, id)?;
    let (below, _) = neighbours(model, id);
    let Some(below) = below else {
        return Err(ModelError::Invalid(format!(
            "{} is the lowest level and has no height below",
            model.describe(id)
        )));
    };
    let elevation = Length::from_si(model.levels[&below].elevation.si() + height.si());
    plan_set_elevation(model, id, elevation, ElevationScope::ThisAndAbove)
}

/// Rebinds every node on `id` to `target`, keeping world coordinates, then
/// removes the level, as one batch.
pub fn plan_remove(model: &Model, id: EntityId, target: EntityId) -> Result<Vec<Command>> {
    level_of(model, id)?;
    level_of(model, target)?;
    if id == target {
        return Err(ModelError::Invalid(
            "choose a different level to move the nodes to".into(),
        ));
    }
    let mut commands: Vec<Command> = model
        .nodes
        .iter()
        .filter(|(_, n)| n.level == id)
        .map(|(nid, n)| Command::UpdateNode {
            id: *nid,
            node: Node {
                level: target,
                ..n.clone()
            },
        })
        .collect();
    commands.push(Command::RemoveLevel { id });
    Ok(commands)
}

/// Problems `after` has that `before` did not. A model that was already
/// incomplete is not asked to become solvable by a level edit. The elements
/// on the moved `nodes` are also checked one by one, because `compile`
/// reports only the first kind of problem it meets and an old one would
/// otherwise hide a new one.
fn newly_invalid(before: &Model, after: &Model, nodes: &BTreeSet<EntityId>) -> Vec<String> {
    let texts = |m: &Model| -> BTreeSet<String> {
        crate::compile::compile(m)
            .err()
            .unwrap_or_default()
            .iter()
            .chain(&crate::compile::geometry_problems(m, nodes))
            .map(|p| p.to_string())
            .collect()
    };
    let old = texts(before);
    texts(after)
        .into_iter()
        .filter(|p| !old.contains(p))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ft(v: f64) -> Length {
        Length::from_feet(v)
    }

    /// Base, L1, and Roof at 0, 12, and 24 ft with one node bound to each,
    /// and an extra node on L1 half a foot below its datum.
    fn stack() -> (Model, [EntityId; 3], EntityId) {
        let mut m = Model::default();
        let base = m.base_level().unwrap();
        let l1 = m.insert(Level::new("L1", ft(12.0)));
        let roof = m.insert(Level::new("Roof", ft(24.0)));
        for (level, z) in [(base, 0.0), (l1, 12.0), (roof, 24.0)] {
            m.insert(Node::new(format!("N{z}"), level, [ft(0.0), ft(0.0), ft(z)]));
        }
        let low = m.insert(Node::new("low", l1, [ft(5.0), ft(0.0), ft(11.5)]));
        (m, [base, l1, roof], low)
    }

    #[test]
    fn order_neighbours_and_heights() {
        let (m, [base, l1, roof], _) = stack();
        assert_eq!(m.levels_by_elevation(), vec![base, l1, roof]);
        assert_eq!(neighbours(&m, l1), (Some(base), Some(roof)));
        assert_eq!(neighbours(&m, base), (None, Some(l1)));
        assert!(height_below(&m, base).is_none());
        assert!((height_below(&m, roof).unwrap() - ft(12.0).si()).abs() < 1e-12);
        assert!((offset(&m.nodes[&EntityId(7)], &m.levels[&l1]) - ft(-0.5).si()).abs() < 1e-12);
    }

    #[test]
    fn membership_separates_floor_and_spanning_objects() {
        let (mut m, [base, l1, roof], low) = stack();
        let mat = m.insert(Material {
            name: "m".into(),
            young: oa_core::units::Pressure::from_si(2e11),
            poisson: 0.3,
            density: Default::default(),
            provenance: None,
        });
        let sec = m.insert(Section {
            name: "s".into(),
            area: oa_core::units::Area::from_si(0.01),
            iy: oa_core::units::SecondMoment::from_si(1e-5),
            iz: oa_core::units::SecondMoment::from_si(1e-5),
            torsion: oa_core::units::SecondMoment::from_si(1e-5),
            provenance: None,
        });
        let [n0, n12, n24] = [0.0, 12.0, 24.0].map(|z| m.find::<Node>(&format!("N{z}")).unwrap());
        let beam = m.insert(Frame::new("beam", [n12, low], mat, sec));
        let column = m.insert(Frame::new("column", [n0, n12], mat, sec));
        let tall = m.insert(Frame::new("tall", [n0, n24], mat, sec));
        let at_l1 = membership(&m, l1);
        assert_eq!(at_l1.nodes, [n12, low].into_iter().collect());
        assert_eq!(at_l1.frames, [beam].into_iter().collect());
        assert_eq!(at_l1.spanning_frames, [column, tall].into_iter().collect());
        let at_base = membership(&m, base);
        assert!(at_base.frames.is_empty());
        assert_eq!(
            at_base.spanning_frames,
            [column, tall].into_iter().collect()
        );
        let at_roof = membership(&m, roof);
        assert_eq!(at_roof.spanning_frames, [tall].into_iter().collect());
    }

    #[test]
    fn scopes_move_the_right_levels_and_keep_offsets() {
        let (m, [base, l1, roof], low) = stack();
        let z = |m: &Model, id: EntityId| m.nodes[&id].position[2].si();
        let e = |m: &Model, id: EntityId| m.levels[&id].elevation.si();

        let mut only = m.clone();
        let plan = plan_set_elevation(&m, l1, ft(14.0), ElevationScope::ThisLevel).unwrap();
        assert_eq!(plan.levels, vec![l1]);
        assert_eq!(plan.nodes.len(), 2);
        Command::Batch {
            commands: plan.commands,
        }
        .apply(&mut only)
        .unwrap();
        assert!((e(&only, l1) - ft(14.0).si()).abs() < 1e-12);
        assert!((e(&only, roof) - ft(24.0).si()).abs() < 1e-12);
        assert!((z(&only, low) - ft(13.5).si()).abs() < 1e-12);
        assert!((z(&only, m.find::<Node>("N0").unwrap()) - 0.0).abs() < 1e-12);

        let mut above = m.clone();
        let plan = plan_set_elevation(&m, l1, ft(14.0), ElevationScope::ThisAndAbove).unwrap();
        assert_eq!(plan.levels.len(), 2);
        Command::Batch {
            commands: plan.commands,
        }
        .apply(&mut above)
        .unwrap();
        assert!((e(&above, roof) - ft(26.0).si()).abs() < 1e-12);
        assert!((z(&above, m.find::<Node>("N24").unwrap()) - ft(26.0).si()).abs() < 1e-12);
        assert!((z(&above, low) - ft(13.5).si()).abs() < 1e-12);
        assert!((e(&above, base)).abs() < 1e-12);

        // Height below resolves against the level beneath and carries the roof.
        let plan = plan_set_height_below(&m, l1, ft(10.0)).unwrap();
        assert!((plan.delta - ft(-2.0).si()).abs() < 1e-12);
        assert!(plan_set_height_below(&m, base, ft(1.0)).is_err());
    }

    #[test]
    fn moves_refuse_crossing_or_coinciding_with_a_fixed_level() {
        let (m, [_, l1, roof], _) = stack();
        assert!(plan_set_elevation(&m, l1, ft(24.0), ElevationScope::ThisLevel).is_err());
        assert!(plan_set_elevation(&m, l1, ft(30.0), ElevationScope::ThisLevel).is_err());
        assert!(plan_set_elevation(&m, roof, ft(12.0), ElevationScope::ThisAndAbove).is_err());
        assert!(plan_set_elevation(&m, l1, ft(-1.0), ElevationScope::ThisAndAbove).is_err());
        assert!(plan_set_elevation(&m, l1, ft(30.0), ElevationScope::ThisAndAbove).is_ok());
        assert!(
            plan_set_elevation(&m, l1, Length::from_si(f64::NAN), ElevationScope::ThisLevel)
                .is_err()
        );
    }

    #[test]
    fn remove_plan_rebinds_then_removes() {
        let (m, [base, l1, _], _) = stack();
        let commands = plan_remove(&m, l1, base).unwrap();
        assert_eq!(commands.len(), 3);
        assert!(matches!(commands.last(), Some(Command::RemoveLevel { id }) if *id == l1));
        assert!(plan_remove(&m, l1, l1).is_err());
        let mut after = m.clone();
        Command::Batch { commands }.apply(&mut after).unwrap();
        assert!(!after.levels.contains_key(&l1));
        assert!(after.nodes.values().all(|n| n.level != l1));
        assert!(
            (after.nodes[&m.find::<Node>("N12").unwrap()].position[2].si() - ft(12.0).si()).abs()
                < 1e-12
        );
    }
}
