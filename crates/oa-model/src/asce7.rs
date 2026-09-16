//! Load combinations from ASCE 7 chapter 2, built from the load cases a
//! model defines. Cases are matched by [`LoadType`]. Cases of one gravity
//! type (dead, live, roof live, snow, rain) add up within a combination;
//! each wind or earthquake case gets combinations of its own. A combination
//! is dropped when a load it needs has no case, so a model with dead and
//! live only gets `1.4D` and `1.2D + 1.6L`; companion loads that are absent
//! are simply left out.
//!
//! Earthquake is one nominal load `E` with the vertical part folded in,
//! since the model carries no design spectral acceleration to expand `Ev`.
//! Live load keeps its full factor; the 0.5 exception for light occupancies
//! is not applied. ASCE 7-22 puts snow at 1.0 and 0.3 in the gravity
//! combinations, 0.15 in the seismic one, and 0.7 (0.525 as a companion,
//! 0.1 with earthquake) for allowable stress, where 7-16 used 1.6, 0.5,
//! 0.2, 1.0, and 0.75.
use crate::{Combination, EntityId, LoadType, Model};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edition {
    Asce7_16,
    Asce7_22,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    /// Section 2.3: strength design (LRFD).
    Strength,
    /// Section 2.4: allowable stress design.
    AllowableStress,
}

/// One addend of a combination, written in the standard as `0.5(Lr or S or R)`:
/// the alternatives, of which one is chosen per generated combination.
struct Slot {
    required: bool,
    alternatives: &'static [(LoadType, f64)],
}
const fn req(alternatives: &'static [(LoadType, f64)]) -> Slot {
    Slot {
        required: true,
        alternatives,
    }
}
const fn opt(alternatives: &'static [(LoadType, f64)]) -> Slot {
    Slot {
        required: false,
        alternatives,
    }
}

use LoadType::{Dead, Earthquake, Live, Rain, RoofLive, Snow, Wind};

/// ASCE 7-16 sections 2.3.1 and 2.3.6.
const STRENGTH_16: &[&[Slot]] = &[
    &[req(&[(Dead, 1.4)])],
    &[
        req(&[(Dead, 1.2)]),
        req(&[(Live, 1.6)]),
        opt(&[(RoofLive, 0.5), (Snow, 0.5), (Rain, 0.5)]),
    ],
    &[
        req(&[(Dead, 1.2)]),
        req(&[(RoofLive, 1.6), (Snow, 1.6), (Rain, 1.6)]),
        opt(&[(Live, 1.0), (Wind, 0.5)]),
    ],
    &[
        req(&[(Dead, 1.2)]),
        req(&[(Wind, 1.0)]),
        opt(&[(Live, 1.0)]),
        opt(&[(RoofLive, 0.5), (Snow, 0.5), (Rain, 0.5)]),
    ],
    &[req(&[(Dead, 0.9)]), req(&[(Wind, 1.0)])],
    &[
        req(&[(Dead, 1.2)]),
        req(&[(Earthquake, 1.0)]),
        opt(&[(Live, 1.0)]),
        opt(&[(Snow, 0.2)]),
    ],
    &[req(&[(Dead, 0.9)]), req(&[(Earthquake, 1.0)])],
];

/// ASCE 7-22 sections 2.3.1 and 2.3.6.
const STRENGTH_22: &[&[Slot]] = &[
    &[req(&[(Dead, 1.4)])],
    &[
        req(&[(Dead, 1.2)]),
        req(&[(Live, 1.6)]),
        opt(&[(RoofLive, 0.5), (Snow, 0.3), (Rain, 0.5)]),
    ],
    &[
        req(&[(Dead, 1.2)]),
        req(&[(RoofLive, 1.6), (Snow, 1.0), (Rain, 1.6)]),
        opt(&[(Live, 1.0), (Wind, 0.5)]),
    ],
    &[
        req(&[(Dead, 1.2)]),
        req(&[(Wind, 1.0)]),
        opt(&[(Live, 1.0)]),
        opt(&[(RoofLive, 0.5), (Snow, 0.3), (Rain, 0.5)]),
    ],
    &[req(&[(Dead, 0.9)]), req(&[(Wind, 1.0)])],
    &[
        req(&[(Dead, 1.2)]),
        req(&[(Earthquake, 1.0)]),
        opt(&[(Live, 1.0)]),
        opt(&[(Snow, 0.15)]),
    ],
    &[req(&[(Dead, 0.9)]), req(&[(Earthquake, 1.0)])],
];

/// ASCE 7-16 sections 2.4.1 and 2.4.5.
const ASD_16: &[&[Slot]] = &[
    &[req(&[(Dead, 1.0)])],
    &[req(&[(Dead, 1.0)]), req(&[(Live, 1.0)])],
    &[
        req(&[(Dead, 1.0)]),
        req(&[(RoofLive, 1.0), (Snow, 1.0), (Rain, 1.0)]),
    ],
    &[
        req(&[(Dead, 1.0)]),
        req(&[(Live, 0.75)]),
        req(&[(RoofLive, 0.75), (Snow, 0.75), (Rain, 0.75)]),
    ],
    &[req(&[(Dead, 1.0)]), req(&[(Wind, 0.6)])],
    &[
        req(&[(Dead, 1.0)]),
        req(&[(Wind, 0.45)]),
        opt(&[(Live, 0.75)]),
        opt(&[(RoofLive, 0.75), (Snow, 0.75), (Rain, 0.75)]),
    ],
    &[req(&[(Dead, 0.6)]), req(&[(Wind, 0.6)])],
    &[req(&[(Dead, 1.0)]), req(&[(Earthquake, 0.7)])],
    &[
        req(&[(Dead, 1.0)]),
        req(&[(Earthquake, 0.525)]),
        opt(&[(Live, 0.75)]),
        opt(&[(Snow, 0.75)]),
    ],
    &[req(&[(Dead, 0.6)]), req(&[(Earthquake, 0.7)])],
];

/// ASCE 7-22 sections 2.4.1 and 2.4.5, with the 0.7 snow adjustment.
const ASD_22: &[&[Slot]] = &[
    &[req(&[(Dead, 1.0)])],
    &[req(&[(Dead, 1.0)]), req(&[(Live, 1.0)])],
    &[
        req(&[(Dead, 1.0)]),
        req(&[(RoofLive, 1.0), (Snow, 0.7), (Rain, 1.0)]),
    ],
    &[
        req(&[(Dead, 1.0)]),
        req(&[(Live, 0.75)]),
        req(&[(RoofLive, 0.75), (Snow, 0.525), (Rain, 0.75)]),
    ],
    &[req(&[(Dead, 1.0)]), req(&[(Wind, 0.6)])],
    &[
        req(&[(Dead, 1.0)]),
        req(&[(Wind, 0.45)]),
        opt(&[(Live, 0.75)]),
        opt(&[(RoofLive, 0.75), (Snow, 0.525), (Rain, 0.75)]),
    ],
    &[req(&[(Dead, 0.6)]), req(&[(Wind, 0.6)])],
    &[req(&[(Dead, 1.0)]), req(&[(Earthquake, 0.7)])],
    &[
        req(&[(Dead, 1.0)]),
        req(&[(Earthquake, 0.525)]),
        opt(&[(Live, 0.75)]),
        opt(&[(Snow, 0.1)]),
    ],
    &[req(&[(Dead, 0.6)]), req(&[(Earthquake, 0.7)])],
];

/// Wind and earthquake cases are directions or patterns that never act at once.
fn exclusive(load_type: LoadType) -> bool {
    matches!(load_type, Wind | Earthquake)
}

/// One way to fill a slot: the cases it takes and how the name reads.
struct Choice {
    label: String,
    terms: Vec<(EntityId, f64)>,
}

fn label(factor: f64, load_type: LoadType, case: Option<&str>) -> String {
    let factor = if factor == 1.0 {
        String::new()
    } else {
        format!("{factor}")
    };
    match case {
        Some(name) => format!("{factor}{}({name})", load_type.symbol()),
        None => format!("{factor}{}", load_type.symbol()),
    }
}

/// The choices for each slot, or `None` when a required slot has none.
fn choices(template: &[Slot], cases: &[(EntityId, LoadType, &str)]) -> Option<Vec<Vec<Choice>>> {
    let mut slots = vec![];
    for slot in template {
        let mut options = vec![];
        for &(load_type, factor) in slot.alternatives {
            let of_type: Vec<(EntityId, &str)> = cases
                .iter()
                .filter(|(_, t, _)| *t == load_type)
                .map(|(id, _, name)| (*id, *name))
                .collect();
            if of_type.is_empty() {
                continue;
            }
            if exclusive(load_type) {
                let several = of_type.len() > 1;
                for (id, name) in of_type {
                    options.push(Choice {
                        label: label(factor, load_type, several.then_some(name)),
                        terms: vec![(id, factor)],
                    });
                }
            } else {
                options.push(Choice {
                    label: label(factor, load_type, None),
                    terms: of_type.into_iter().map(|(id, _)| (id, factor)).collect(),
                });
            }
        }
        if options.is_empty() {
            if slot.required {
                return None;
            }
            continue;
        }
        slots.push(options);
    }
    Some(slots)
}

fn product(slots: &[Vec<Choice>]) -> Vec<Vec<&Choice>> {
    let mut picks: Vec<Vec<&Choice>> = vec![vec![]];
    for slot in slots {
        picks = picks
            .iter()
            .flat_map(|pick| {
                slot.iter().map(move |choice| {
                    let mut pick = pick.clone();
                    pick.push(choice);
                    pick
                })
            })
            .collect();
    }
    picks
}

fn sorted(terms: &[(EntityId, f64)]) -> Vec<(EntityId, f64)> {
    let mut terms = terms.to_vec();
    terms.sort_by_key(|(id, _)| *id);
    terms
}

fn unique(name: String, taken: &BTreeSet<String>) -> String {
    if !taken.contains(&name) {
        return name;
    }
    (2..)
        .map(|n| format!("{name} ({n})"))
        .find(|candidate| !taken.contains(candidate))
        .expect("unbounded")
}

/// The combinations of the chosen edition and method that the model's load
/// cases can form and that it does not already have. Names read like the
/// standard, `1.2D + 1.6L + 0.5Lr`, with the case named after the symbol
/// when several cases share a wind or earthquake type; a name already taken
/// gets a numeric suffix.
pub fn generate(model: &Model, edition: Edition, method: Method) -> Vec<Combination> {
    let templates = match (edition, method) {
        (Edition::Asce7_16, Method::Strength) => STRENGTH_16,
        (Edition::Asce7_22, Method::Strength) => STRENGTH_22,
        (Edition::Asce7_16, Method::AllowableStress) => ASD_16,
        (Edition::Asce7_22, Method::AllowableStress) => ASD_22,
    };
    let cases: Vec<(EntityId, LoadType, &str)> = model
        .load_cases
        .iter()
        .map(|(id, c)| (*id, c.load_type, c.name.as_str()))
        .collect();
    let mut taken_terms: Vec<Vec<(EntityId, f64)>> = model
        .combinations
        .values()
        .map(|c| sorted(&c.terms))
        .collect();
    let mut taken_names: BTreeSet<String> =
        model.combinations.values().map(|c| c.name.clone()).collect();
    let mut out = vec![];
    for template in templates {
        let Some(slots) = choices(template, &cases) else {
            continue;
        };
        for pick in product(&slots) {
            let terms: Vec<(EntityId, f64)> =
                pick.iter().flat_map(|c| c.terms.iter().copied()).collect();
            let key = sorted(&terms);
            if taken_terms.contains(&key) {
                continue;
            }
            taken_terms.push(key);
            let labels: Vec<&str> = pick.iter().map(|c| c.label.as_str()).collect();
            let name = unique(labels.join(" + "), &taken_names);
            taken_names.insert(name.clone());
            out.push(Combination { name, terms });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LoadCase;

    fn model(cases: &[(&str, LoadType)]) -> Model {
        let mut m = Model::default();
        for (name, load_type) in cases {
            m.insert(LoadCase::new(*name).with_type(*load_type));
        }
        m
    }
    fn names(combos: &[Combination]) -> Vec<&str> {
        combos.iter().map(|c| c.name.as_str()).collect()
    }

    #[test]
    fn dead_and_live_only() {
        let m = model(&[("Dead", Dead), ("Live", Live)]);
        let combos = generate(&m, Edition::Asce7_16, Method::Strength);
        assert_eq!(names(&combos), ["1.4D", "1.2D + 1.6L"]);
        let combos = generate(&m, Edition::Asce7_16, Method::AllowableStress);
        assert_eq!(names(&combos), ["D", "D + L"]);
    }

    #[test]
    fn no_dead_means_no_combinations() {
        let m = model(&[("Live", Live), ("Wind", Wind)]);
        assert!(generate(&m, Edition::Asce7_22, Method::Strength).is_empty());
    }

    #[test]
    fn wind_cases_expand_and_gravity_cases_add_up() {
        let m = model(&[
            ("Dead", Dead),
            ("SDL", Dead),
            ("Live", Live),
            ("Roof live", RoofLive),
            ("Wind X", Wind),
            ("Wind Y", Wind),
        ]);
        let combos = generate(&m, Edition::Asce7_16, Method::Strength);
        let names = names(&combos);
        assert_eq!(
            names,
            [
                "1.4D",
                "1.2D + 1.6L + 0.5Lr",
                "1.2D + 1.6Lr + L",
                "1.2D + 1.6Lr + 0.5W(Wind X)",
                "1.2D + 1.6Lr + 0.5W(Wind Y)",
                "1.2D + W(Wind X) + L + 0.5Lr",
                "1.2D + W(Wind Y) + L + 0.5Lr",
                "0.9D + W(Wind X)",
                "0.9D + W(Wind Y)",
            ]
        );
        // Both dead cases carry the factor.
        assert_eq!(combos[0].terms.len(), 2);
        assert!(combos[0].terms.iter().all(|(_, f)| *f == 1.4));
    }

    #[test]
    fn asce_7_22_snow_factors() {
        let m = model(&[("Dead", Dead), ("Live", Live), ("Snow", Snow)]);
        let strength = generate(&m, Edition::Asce7_22, Method::Strength);
        assert_eq!(
            names(&strength),
            ["1.4D", "1.2D + 1.6L + 0.3S", "1.2D + S + L"]
        );
        let asd = generate(&m, Edition::Asce7_22, Method::AllowableStress);
        assert_eq!(
            names(&asd),
            ["D", "D + L", "D + 0.7S", "D + 0.75L + 0.525S"]
        );
        let old = generate(&m, Edition::Asce7_16, Method::Strength);
        assert_eq!(
            names(&old),
            ["1.4D", "1.2D + 1.6L + 0.5S", "1.2D + 1.6S + L"]
        );
    }

    #[test]
    fn seismic_snow_factors_follow_the_edition() {
        let m = model(&[
            ("Dead", Dead),
            ("Live", Live),
            ("Snow", Snow),
            ("Quake", Earthquake),
        ]);
        let seismic = |edition, method| -> Vec<String> {
            generate(&m, edition, method)
                .into_iter()
                .filter(|c| c.name.contains('E'))
                .map(|c| c.name)
                .collect()
        };
        assert_eq!(
            seismic(Edition::Asce7_16, Method::Strength),
            ["1.2D + E + L + 0.2S", "0.9D + E"]
        );
        assert_eq!(
            seismic(Edition::Asce7_22, Method::Strength),
            ["1.2D + E + L + 0.15S", "0.9D + E"]
        );
        assert_eq!(
            seismic(Edition::Asce7_16, Method::AllowableStress),
            ["D + 0.7E", "D + 0.525E + 0.75L + 0.75S", "0.6D + 0.7E"]
        );
        assert_eq!(
            seismic(Edition::Asce7_22, Method::AllowableStress),
            ["D + 0.7E", "D + 0.525E + 0.75L + 0.1S", "0.6D + 0.7E"]
        );
    }

    #[test]
    fn generating_again_adds_nothing_and_names_stay_unique() {
        let mut m = model(&[("Dead", Dead), ("Live", Live)]);
        for c in generate(&m, Edition::Asce7_16, Method::Strength) {
            m.insert(c);
        }
        assert!(generate(&m, Edition::Asce7_16, Method::Strength).is_empty());
        // A different combination that happens to use a generated name.
        let dead = *m.load_cases.keys().next().unwrap();
        m.insert(Combination {
            name: "D".into(),
            terms: vec![(dead, 1.1)],
        });
        let asd = generate(&m, Edition::Asce7_16, Method::AllowableStress);
        assert_eq!(names(&asd), ["D (2)", "D + L"]);
    }
}
