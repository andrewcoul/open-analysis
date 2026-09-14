//! Small forms that create entities or loads. Each dialog owns its input
//! state in an entity; the dialog builder only renders it, so the same
//! inputs survive re-renders of the overlay.
use crate::document::{Document, unused_name};
use crate::text::parse_num;
use gpui_kit::component::form::{Field, Form};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::select::{SearchableVec, Select, SelectState};
use gpui_kit::component::{IndexPath, Sizable as _, WindowExt as _};
use gpui_kit::*;
use oa_core::units::Length;
use oa_core::units::*;
use oa_model::{
    Axes, Command, EntityId, EntityKind, Frame, Library, LoadCase, Material, MemberLoad, Model,
    NodalLoad, Node, Section,
};

type Choice = Entity<SelectState<SearchableVec<SharedString>>>;

/// Labelled text inputs plus optional choice lists, read back by label.
struct Inputs {
    texts: Vec<(SharedString, Entity<InputState>)>,
    choices: Vec<(SharedString, Choice, Vec<SharedString>)>,
}
impl Inputs {
    fn new(window: &mut Window, cx: &mut App, fields: &[(&str, &str)]) -> Self {
        let texts = fields
            .iter()
            .map(|(label, value)| {
                let value = value.to_string();
                (
                    SharedString::from(label.to_string()),
                    cx.new(|cx| InputState::new(window, cx).default_value(value)),
                )
            })
            .collect();
        Self {
            texts,
            choices: vec![],
        }
    }
    fn with_choice(
        mut self,
        label: &str,
        options: Vec<SharedString>,
        selected: Option<usize>,
        window: &mut Window,
        cx: &mut App,
    ) -> Self {
        let select = cx.new(|cx| {
            SelectState::new(
                SearchableVec::from(options.clone()),
                selected.map(IndexPath::new),
                window,
                cx,
            )
        });
        self.choices.push((label.into(), select, options));
        self
    }
    fn text(&self, label: &str, cx: &App) -> String {
        self.texts
            .iter()
            .find(|(l, _)| l.as_ref() == label)
            .map(|(_, s)| s.read(cx).value().to_string())
            .unwrap_or_default()
    }
    fn num(&self, label: &str, cx: &App) -> Result<f64, String> {
        parse_num(label, &self.text(label, cx))
    }
    fn choice(&self, label: &str, cx: &App) -> Option<usize> {
        self.choices
            .iter()
            .find(|(l, _, _)| l.as_ref() == label)
            .and_then(|(_, s, _)| s.read(cx).selected_index(cx))
            .map(|ix| ix.row)
    }
    fn form(&self) -> Form {
        let mut form = Form::new().label_text_size(rems(0.8));
        for (label, select, _) in &self.choices {
            form = form.child(
                Field::new()
                    .label(label.clone())
                    .child(Select::new(select).small()),
            );
        }
        for (label, input) in &self.texts {
            form = form.child(
                Field::new()
                    .label(label.clone())
                    .child(Input::new(input).small()),
            );
        }
        form
    }
}

fn notify_error(window: &mut Window, cx: &mut App, message: impl Into<SharedString>) {
    window.push_notification(Notification::error(message), cx);
}

/// Applies a command and reports failure in a notification. Returns whether it succeeded.
fn apply(document: &Entity<Document>, command: Command, window: &mut Window, cx: &mut App) -> bool {
    match document.update(cx, |document, cx| document.apply(command, cx)) {
        Ok(()) => true,
        Err(e) => {
            notify_error(window, cx, e.to_string());
            false
        }
    }
}

/// Opens a dialog whose OK button runs `on_ok`; the dialog closes when it returns true.
fn open<F>(title: &'static str, inputs: Inputs, window: &mut Window, cx: &mut App, on_ok: F)
where
    F: Fn(&Inputs, &mut Window, &mut App) -> bool + 'static,
{
    let inputs = std::rc::Rc::new(inputs);
    let on_ok = std::rc::Rc::new(on_ok);
    window.open_dialog(cx, move |dialog, _, _| {
        let inputs_for_ok = inputs.clone();
        let on_ok = on_ok.clone();
        dialog
            .title(title)
            .w(px(420.))
            .child(inputs.form())
            .on_ok(move |_, window, cx| on_ok(&inputs_for_ok, window, cx))
    });
}

fn names(model: &Model, kind: EntityKind) -> Vec<SharedString> {
    crate::explorer::rows_of(model, kind)
        .into_iter()
        .map(|(_, n)| n.into())
        .collect()
}
fn id_at(model: &Model, kind: EntityKind, ix: Option<usize>) -> Option<EntityId> {
    crate::explorer::rows_of(model, kind)
        .get(ix?)
        .map(|(id, _)| *id)
}

pub fn add_node(document: Entity<Document>, window: &mut Window, cx: &mut App) {
    let name = unused_name::<Node>(document.read(cx).model(), "N");
    let inputs = Inputs::new(
        window,
        cx,
        &[
            ("Name", &name),
            ("X (m)", "0"),
            ("Y (m)", "0"),
            ("Z (m)", "0"),
        ],
    );
    open("Add node", inputs, window, cx, move |inputs, window, cx| {
        let position = ["X (m)", "Y (m)", "Z (m)"].map(|l| inputs.num(l, cx));
        let mut node = Node::new(inputs.text("Name", cx), [Length::ZERO; 3]);
        for (i, p) in position.into_iter().enumerate() {
            match p {
                Ok(v) => node.position[i] = Length::from_metres(v),
                Err(e) => {
                    notify_error(window, cx, e);
                    return false;
                }
            }
        }
        let id = document.read(cx).model().next_id;
        apply(
            &document,
            Command::AddNode {
                id: EntityId(id),
                node,
            },
            window,
            cx,
        )
    });
}

pub fn add_material_from_library(document: Entity<Document>, window: &mut Window, cx: &mut App) {
    let library = Library::starter();
    let designations: Vec<SharedString> = library
        .materials
        .iter()
        .map(|m| m.designation.clone().into())
        .collect();
    let name = unused_name::<Material>(document.read(cx).model(), "MAT");
    let inputs = Inputs::new(window, cx, &[("Name", &name)]).with_choice(
        "Library material",
        designations,
        Some(0),
        window,
        cx,
    );
    open(
        "Add material from library",
        inputs,
        window,
        cx,
        move |inputs, window, cx| {
            let Some(ix) = inputs.choice("Library material", cx) else {
                notify_error(window, cx, "Choose a library material");
                return false;
            };
            let designation = library.materials[ix].designation.clone();
            let material = library
                .material(&designation, inputs.text("Name", cx))
                .expect("designation from the list");
            let id = document.read(cx).model().next_id;
            apply(
                &document,
                Command::AddMaterial {
                    id: EntityId(id),
                    material,
                },
                window,
                cx,
            )
        },
    );
}

pub fn add_custom_material(document: Entity<Document>, window: &mut Window, cx: &mut App) {
    let name = unused_name::<Material>(document.read(cx).model(), "MAT");
    let inputs = Inputs::new(
        window,
        cx,
        &[
            ("Name", &name),
            ("E (Pa)", "2e11"),
            ("Poisson's ratio", "0.3"),
            ("Density (kg/m³)", "7850"),
        ],
    );
    open(
        "Add material",
        inputs,
        window,
        cx,
        move |inputs, window, cx| {
            let values = (
                inputs.num("E (Pa)", cx),
                inputs.num("Poisson's ratio", cx),
                inputs.num("Density (kg/m³)", cx),
            );
            let (young, poisson, density) = match values {
                (Ok(e), Ok(nu), Ok(rho)) => (e, nu, rho),
                (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => {
                    notify_error(window, cx, e);
                    return false;
                }
            };
            let material = Material {
                name: inputs.text("Name", cx),
                young: Pressure::from_si(young),
                poisson,
                density: MassDensity::from_si(density),
                provenance: None,
            };
            let id = document.read(cx).model().next_id;
            apply(
                &document,
                Command::AddMaterial {
                    id: EntityId(id),
                    material,
                },
                window,
                cx,
            )
        },
    );
}

pub fn add_section_from_library(document: Entity<Document>, window: &mut Window, cx: &mut App) {
    let library = Library::starter();
    let designations: Vec<SharedString> = library
        .section_designations()
        .into_iter()
        .map(|s| s.to_string().into())
        .collect();
    let name = unused_name::<Section>(document.read(cx).model(), "SEC");
    let inputs = Inputs::new(window, cx, &[("Name", &name)]).with_choice(
        "Library section",
        designations,
        Some(0),
        window,
        cx,
    );
    open(
        "Add section from library",
        inputs,
        window,
        cx,
        move |inputs, window, cx| {
            let Some(ix) = inputs.choice("Library section", cx) else {
                notify_error(window, cx, "Choose a library section");
                return false;
            };
            let designation = library.sections[ix].designation.clone();
            let section = library
                .section(&designation, inputs.text("Name", cx))
                .expect("designation from the list");
            let id = document.read(cx).model().next_id;
            apply(
                &document,
                Command::AddSection {
                    id: EntityId(id),
                    section,
                },
                window,
                cx,
            )
        },
    );
}

pub fn add_custom_section(document: Entity<Document>, window: &mut Window, cx: &mut App) {
    let name = unused_name::<Section>(document.read(cx).model(), "SEC");
    let inputs = Inputs::new(
        window,
        cx,
        &[
            ("Name", &name),
            ("Area (m²)", "0.01"),
            ("Iy (m⁴)", "2e-5"),
            ("Iz (m⁴)", "4e-5"),
            ("J (m⁴)", "1e-5"),
        ],
    );
    open(
        "Add section",
        inputs,
        window,
        cx,
        move |inputs, window, cx| {
            let values = ["Area (m²)", "Iy (m⁴)", "Iz (m⁴)", "J (m⁴)"].map(|l| inputs.num(l, cx));
            let mut v = [0.0; 4];
            for (i, value) in values.into_iter().enumerate() {
                match value {
                    Ok(x) => v[i] = x,
                    Err(e) => {
                        notify_error(window, cx, e);
                        return false;
                    }
                }
            }
            let section = Section {
                name: inputs.text("Name", cx),
                area: Area::from_si(v[0]),
                iy: SecondMoment::from_si(v[1]),
                iz: SecondMoment::from_si(v[2]),
                torsion: SecondMoment::from_si(v[3]),
                provenance: None,
            };
            let id = document.read(cx).model().next_id;
            apply(
                &document,
                Command::AddSection {
                    id: EntityId(id),
                    section,
                },
                window,
                cx,
            )
        },
    );
}

/// The load case the dialogs default to: the selected one, else the first.
fn default_case(document: &Document) -> Option<usize> {
    let cases = crate::explorer::rows_of(document.model(), EntityKind::LoadCase);
    document
        .selected_of(EntityKind::LoadCase)
        .first()
        .and_then(|sel| cases.iter().position(|(id, _)| id == sel))
        .or_else(|| (!cases.is_empty()).then_some(0))
}

/// Adds one nodal load per selected node to a load case.
pub fn add_nodal_load(document: Entity<Document>, window: &mut Window, cx: &mut App) {
    let (cases, selected_case, nodes) = {
        let doc = document.read(cx);
        (
            names(doc.model(), EntityKind::LoadCase),
            default_case(doc),
            doc.selected_of(EntityKind::Node),
        )
    };
    if cases.is_empty() {
        notify_error(window, cx, "Define a load case first (Define > Load case)");
        return;
    }
    if nodes.is_empty() {
        notify_error(window, cx, "Select the nodes to load first");
        return;
    }
    let inputs = Inputs::new(
        window,
        cx,
        &[
            ("Fx (N)", "0"),
            ("Fy (N)", "0"),
            ("Fz (N)", "0"),
            ("Mx (N·m)", "0"),
            ("My (N·m)", "0"),
            ("Mz (N·m)", "0"),
        ],
    )
    .with_choice("Load case", cases, selected_case, window, cx);
    open(
        "Add nodal load to selected nodes",
        inputs,
        window,
        cx,
        move |inputs, window, cx| {
            let values = [
                "Fx (N)",
                "Fy (N)",
                "Fz (N)",
                "Mx (N·m)",
                "My (N·m)",
                "Mz (N·m)",
            ]
            .map(|l| inputs.num(l, cx));
            let mut v = [0.0; 6];
            for (i, value) in values.into_iter().enumerate() {
                match value {
                    Ok(x) => v[i] = x,
                    Err(e) => {
                        notify_error(window, cx, e);
                        return false;
                    }
                }
            }
            let Some(case) = id_at(
                document.read(cx).model(),
                EntityKind::LoadCase,
                inputs.choice("Load case", cx),
            ) else {
                notify_error(window, cx, "Choose a load case");
                return false;
            };
            let mut load_case: LoadCase = document.read(cx).model().load_cases[&case].clone();
            for node in &nodes {
                load_case.nodal.push(NodalLoad {
                    node: *node,
                    force: [
                        Force::from_si(v[0]),
                        Force::from_si(v[1]),
                        Force::from_si(v[2]),
                    ],
                    moment: [
                        Moment::from_si(v[3]),
                        Moment::from_si(v[4]),
                        Moment::from_si(v[5]),
                    ],
                });
            }
            apply(
                &document,
                Command::UpdateLoadCase {
                    id: case,
                    load_case,
                },
                window,
                cx,
            )
        },
    );
}

/// Adds a full-length uniform load to every selected frame.
pub fn add_distributed_load(document: Entity<Document>, window: &mut Window, cx: &mut App) {
    let (cases, selected_case, frames) = {
        let doc = document.read(cx);
        (
            names(doc.model(), EntityKind::LoadCase),
            default_case(doc),
            doc.selected_of(EntityKind::Frame),
        )
    };
    if cases.is_empty() {
        notify_error(window, cx, "Define a load case first (Define > Load case)");
        return;
    }
    if frames.is_empty() {
        notify_error(window, cx, "Select the frames to load first");
        return;
    }
    let inputs = Inputs::new(
        window,
        cx,
        &[("wx (N/m)", "0"), ("wy (N/m)", "0"), ("wz (N/m)", "-10000")],
    )
    .with_choice("Load case", cases, selected_case, window, cx)
    .with_choice(
        "Axes",
        vec!["Global".into(), "Local".into()],
        Some(0),
        window,
        cx,
    );
    open(
        "Add uniform load to selected frames",
        inputs,
        window,
        cx,
        move |inputs, window, cx| {
            let values = ["wx (N/m)", "wy (N/m)", "wz (N/m)"].map(|l| inputs.num(l, cx));
            let mut w = [LineLoad::ZERO; 3];
            for (i, value) in values.into_iter().enumerate() {
                match value {
                    Ok(x) => w[i] = LineLoad::from_si(x),
                    Err(e) => {
                        notify_error(window, cx, e);
                        return false;
                    }
                }
            }
            let axes = if inputs.choice("Axes", cx) == Some(1) {
                Axes::Local
            } else {
                Axes::Global
            };
            let Some(case) = id_at(
                document.read(cx).model(),
                EntityKind::LoadCase,
                inputs.choice("Load case", cx),
            ) else {
                notify_error(window, cx, "Choose a load case");
                return false;
            };
            let model = document.read(cx).model();
            let mut load_case: LoadCase = model.load_cases[&case].clone();
            for frame in &frames {
                let Some(length) = frame_length(model, &model.frames[frame]) else {
                    continue;
                };
                load_case.member.push(MemberLoad::Distributed {
                    member: *frame,
                    start: Length::ZERO,
                    end: Length::from_metres(length),
                    start_load: w,
                    end_load: w,
                    axes,
                });
            }
            apply(
                &document,
                Command::UpdateLoadCase {
                    id: case,
                    load_case,
                },
                window,
                cx,
            )
        },
    );
}

fn frame_length(model: &Model, frame: &Frame) -> Option<f64> {
    let a = model.nodes.get(&frame.nodes[0])?.position;
    let b = model.nodes.get(&frame.nodes[1])?.position;
    Some(
        (0..3)
            .map(|i| (b[i].si() - a[i].si()).powi(2))
            .sum::<f64>()
            .sqrt(),
    )
}
