//! Thread budget, rolling in-flight window, and ordered consumption.
use oa_core::{units::*, *};

fn base() -> Model {
    let mut m = Model::default();
    m.add_material(Material {
        young: Pressure::from_gpa(200.0),
        poisson: 0.3,
        density: MassDensity::from_si(7850.0),
    });
    m.add_section(Section {
        area: Area::from_si(0.01),
        iy: SecondMoment::from_si(2e-5),
        iz: SecondMoment::from_si(4e-5),
        torsion: SecondMoment::from_si(1e-5),
    });
    m
}

/// Regular moment frame with `combos` combinations of a gravity and a lateral case.
fn frame(bays: usize, stories: usize, combos: usize) -> Model {
    let mut m = base();
    let nx = bays + 1;
    let id = |i: usize, j: usize, k: usize| NodeId(k * nx * nx + j * nx + i);
    for k in 0..=stories {
        for j in 0..=bays {
            for i in 0..=bays {
                let p = [
                    Length::from_si(6.0 * i as f64),
                    Length::from_si(4.0 * k as f64),
                    Length::from_si(6.0 * j as f64),
                ];
                m.add_node(if k == 0 { Node::fixed(p) } else { Node::new(p) });
            }
        }
    }
    let mut member = vec![];
    for k in 0..stories {
        for j in 0..=bays {
            for i in 0..=bays {
                m.add_frame(Frame::new(
                    [id(i, j, k), id(i, j, k + 1)],
                    MaterialId(0),
                    SectionId(0),
                ));
            }
        }
    }
    for k in 1..=stories {
        for j in 0..=bays {
            for i in 0..=bays {
                for (di, dj) in [(1, 0), (0, 1)] {
                    if i + di <= bays && j + dj <= bays {
                        let fid = FrameId(m.frames.len());
                        member.push(MemberLoad::Distributed {
                            member: fid,
                            start: Length::ZERO,
                            end: Length::from_si(6.0),
                            start_load: [LineLoad::ZERO, LineLoad::from_si(-10e3), LineLoad::ZERO],
                            end_load: [LineLoad::ZERO, LineLoad::from_si(-10e3), LineLoad::ZERO],
                            axes: Axes::Global,
                        });
                        m.add_frame(Frame::new(
                            [id(i, j, k), id(i + di, j + dj, k)],
                            MaterialId(0),
                            SectionId(0),
                        ));
                    }
                }
            }
        }
    }
    let nodal = (nx * nx..m.nodes.len())
        .map(|i| NodalLoad {
            node: NodeId(i),
            force: [Force::from_si(5e3), Force::ZERO, Force::ZERO],
            moment: [Moment::ZERO; 3],
        })
        .collect();
    m.load_cases.push(LoadCase {
        name: "dead".into(),
        member,
        self_weight: [0.0, -1.0, 0.0],
        ..Default::default()
    });
    m.load_cases.push(LoadCase {
        name: "wind".into(),
        nodal,
        ..Default::default()
    });
    for c in 0..combos {
        m.combinations.push(LoadCombination {
            name: format!("c{c}"),
            terms: vec![
                (LoadCaseId(0), 1.0 + 0.05 * c as f64),
                (
                    LoadCaseId(1),
                    if c % 2 == 0 { 0.5 } else { -0.5 } * (1.0 + c as f64),
                ),
            ],
        });
    }
    m
}

fn displacements(r: &InMemoryResults) -> Vec<f64> {
    r.combinations
        .iter()
        .flat_map(|c| c.displacements.as_ref().unwrap().iter().flatten().copied())
        .collect()
}
fn assert_same(a: &InMemoryResults, b: &InMemoryResults) {
    let (a, b) = (displacements(a), displacements(b));
    assert_eq!(a.len(), b.len());
    let scale = a.iter().fold(0.0_f64, |m, v| m.max(v.abs()));
    for (x, y) in a.iter().zip(&b) {
        assert!((x - y).abs() <= 1e-10 * scale, "{x} vs {y}");
    }
}

struct Recorder {
    names: Vec<String>,
    fail_at: Option<usize>,
}
impl ResultConsumer for Recorder {
    fn consume(&mut self, result: CombinationResult) -> Result<()> {
        if self.fail_at == Some(self.names.len()) {
            return Err(Error::Request("consumer refused".into()));
        }
        self.names.push(result.combination);
        Ok(())
    }
}

#[test]
fn window_preserves_order_and_matches_serial_run() {
    let model = frame(3, 4, 13);
    let serial = analyze_static(
        &model,
        &StaticOptions {
            threads: 1,
            max_in_flight: 1,
            ..Default::default()
        },
    )
    .unwrap();
    for (threads, in_flight) in [(0, 3), (4, 2), (4, 13), (2, 5)] {
        let options = StaticOptions {
            threads,
            max_in_flight: in_flight,
            ..Default::default()
        };
        let mut recorder = Recorder {
            names: vec![],
            fail_at: None,
        };
        analyze_static_into(&model, &options, &mut recorder).unwrap();
        let expected: Vec<_> = (0..13).map(|c| format!("c{c}")).collect();
        assert_eq!(
            recorder.names, expected,
            "threads {threads}, in flight {in_flight}"
        );
        assert_same(&serial, &analyze_static(&model, &options).unwrap());
    }
    // P-Delta builds a system per combination: the window bounds those too.
    let options = StaticOptions {
        method: StaticMethod::PDelta,
        threads: 3,
        max_in_flight: 4,
        ..Default::default()
    };
    let mut recorder = Recorder {
        names: vec![],
        fail_at: None,
    };
    analyze_static_into(&frame(2, 2, 6), &options, &mut recorder).unwrap();
    assert_eq!(recorder.names.len(), 6);
}

#[test]
fn consumer_error_stops_after_ordered_prefix() {
    let model = frame(2, 3, 9);
    let options = StaticOptions {
        threads: 4,
        max_in_flight: 4,
        ..Default::default()
    };
    let mut recorder = Recorder {
        names: vec![],
        fail_at: Some(3),
    };
    let err = analyze_static_into(&model, &options, &mut recorder).unwrap_err();
    assert!(matches!(err, Error::Request(m) if m == "consumer refused"));
    assert_eq!(recorder.names, ["c0", "c1", "c2"]);
}

#[test]
fn failing_combination_reports_in_order_after_earlier_results() {
    // A torsional point moment on a member released in torsion at both ends
    // has no supporting stiffness, so only the combinations that include it fail.
    let mut model = frame(2, 2, 6);
    model.frames[0].releases[3] = true;
    model.frames[0].releases[9] = true;
    model.load_cases.push(LoadCase {
        name: "twist".into(),
        member: vec![MemberLoad::Point {
            member: FrameId(0),
            position: Length::from_si(2.0),
            force: [Force::ZERO; 3],
            moment: [Moment::from_si(1e3), Moment::ZERO, Moment::ZERO],
            axes: Axes::Local,
        }],
        ..Default::default()
    });
    model.combinations[4].terms.push((LoadCaseId(2), 1.0));
    let options = StaticOptions {
        threads: 4,
        max_in_flight: 6,
        ..Default::default()
    };
    let mut recorder = Recorder {
        names: vec![],
        fail_at: None,
    };
    let err = analyze_static_into(&model, &options, &mut recorder).unwrap_err();
    assert!(matches!(err, Error::Model(_)), "{err}");
    assert_eq!(recorder.names, ["c0", "c1", "c2", "c3"]);
}

#[test]
fn frame_output_off_still_activates_nonlinear_members() {
    let mut model = frame(2, 2, 4);
    for f in &mut model.frames {
        f.behavior = AxialBehavior::TensionOnly;
    }
    // Brace-free frames have columns in compression, so tension-only columns
    // must deactivate; the run reports this as an unstable structure or as
    // deactivated members, never as a silently linear answer.
    let outputs = OutputSelection {
        frames: false,
        ..Default::default()
    };
    let with = analyze_static(
        &model,
        &StaticOptions {
            method: StaticMethod::Nonlinear,
            max_iterations: 5,
            ..Default::default()
        },
    );
    let without = analyze_static(
        &model,
        &StaticOptions {
            method: StaticMethod::Nonlinear,
            max_iterations: 5,
            outputs,
            ..Default::default()
        },
    );
    match (with, without) {
        (Ok(a), Ok(b)) => {
            assert_same(&a, &b);
            assert!(b.combinations[0].frames.is_none());
        }
        (Err(a), Err(b)) => assert_eq!(a.to_string(), b.to_string()),
        (a, b) => panic!("outputs changed the outcome: {a:?} vs {b:?}"),
    }
}

#[test]
fn modal_and_spectrum_honour_thread_budget() {
    let model = frame(2, 3, 1);
    let options = |threads| ModalOptions {
        modes: 6,
        threads,
        ..Default::default()
    };
    let a = analyze_modal(&model, &options(1)).unwrap();
    let b = analyze_modal(&model, &options(0)).unwrap();
    for (x, y) in a.modes.iter().zip(&b.modes) {
        assert!((x.frequency_hz - y.frequency_hz).abs() <= 1e-9 * x.frequency_hz);
    }
    let spectrum = |threads, combination| SpectrumOptions {
        modal: options(threads),
        spectrum: vec![
            SpectrumPoint {
                period_seconds: 0.0,
                acceleration: Acceleration::from_si(4.0),
            },
            SpectrumPoint {
                period_seconds: 10.0,
                acceleration: Acceleration::from_si(1.0),
            },
        ],
        combination,
        ..Default::default()
    };
    let cqc = analyze_spectrum(&model, &spectrum(1, ModalCombination::Cqc)).unwrap();
    let cqc_pool = analyze_spectrum(&model, &spectrum(0, ModalCombination::Cqc)).unwrap();
    let srss = analyze_spectrum(&model, &spectrum(3, ModalCombination::Srss)).unwrap();
    for i in 0..6 {
        let scale = cqc.base_reaction[i].abs().max(1e-9);
        assert!((cqc.base_reaction[i] - cqc_pool.base_reaction[i]).abs() <= 1e-9 * scale);
        // SRSS drops cross terms, so it differs from CQC but stays comparable.
        assert!((srss.base_reaction[i] - cqc.base_reaction[i]).abs() <= 0.5 * scale);
    }
}
