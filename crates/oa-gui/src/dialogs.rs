//! Small forms that create entities or loads. Each dialog owns its input
//! state in an entity; the dialog builder only renders it, so the same
//! inputs survive re-renders of the overlay.
use crate::document::{Document, unused_name};
use crate::text::{label, parse_num, parse_opt_q, parse_q};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dialog::{Cancel, Confirm, DialogFooter};
use gpui_kit::component::form::{Field, Form};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::select::{SearchableVec, Select, SelectState};
use gpui_kit::component::{IndexPath, Sizable as _, WindowExt as _};
use gpui_kit::*;
use oa_core::units::Length;
use oa_core::units::*;
use oa_model::asce7::{Edition, Method};
use oa_model::{
    Axes, Command, EntityId, EntityKind, Frame, Library, LoadCase, LoadType, Material, MemberLoad,
    Model, NodalLoad, Node, Role, Section, Underlay,
};

type Choice = Entity<SelectState<SearchableVec<SharedString>>>;

/// How much of a row a field takes on the forms' six-column grid.
#[derive(Clone, Copy)]
enum Width {
    Full,
    Half,
    Third,
}
impl Width {
    fn span(self) -> u16 {
        match self {
            Width::Full => 6,
            Width::Half => 3,
            Width::Third => 2,
        }
    }
}

enum Widget {
    Text(Entity<InputState>),
    Choice(Choice),
}

/// Labelled text inputs and choice lists, laid out in the order they are
/// added and read back by label.
#[derive(Default)]
struct Inputs {
    fields: Vec<(SharedString, Widget, Width)>,
}
impl Inputs {
    fn with_text(
        mut self,
        label: &str,
        value: &str,
        width: Width,
        window: &mut Window,
        cx: &mut App,
    ) -> Self {
        let value = value.to_string();
        let input = cx.new(|cx| InputState::new(window, cx).default_value(value));
        self.fields
            .push((label.to_string().into(), Widget::Text(input), width));
        self
    }
    /// A text input that may be left blank, saying what a blank one means.
    fn with_optional_text(
        mut self,
        label: &str,
        value: &str,
        blank: &str,
        width: Width,
        window: &mut Window,
        cx: &mut App,
    ) -> Self {
        let (value, blank) = (value.to_string(), blank.to_string());
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(value)
                .placeholder(blank)
        });
        self.fields
            .push((label.to_string().into(), Widget::Text(input), width));
        self
    }
    /// A name that may be left blank, saying what a blank one becomes.
    fn with_optional_name(mut self, blank: &str, window: &mut Window, cx: &mut App) -> Self {
        let blank = blank.to_string();
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(blank));
        self.fields
            .push(("Name".into(), Widget::Text(input), Width::Full));
        self
    }
    fn with_choice(
        mut self,
        label: &str,
        options: Vec<SharedString>,
        selected: Option<usize>,
        width: Width,
        window: &mut Window,
        cx: &mut App,
    ) -> Self {
        // A list too long to scan, such as the AISC shapes, gets a search box.
        let searchable = options.len() > 20;
        let select = cx.new(|cx| {
            SelectState::new(
                SearchableVec::from(options),
                selected.map(IndexPath::new),
                window,
                cx,
            )
            .searchable(searchable)
        });
        self.fields
            .push((label.to_string().into(), Widget::Choice(select), width));
        self
    }
    fn text(&self, label: &str, cx: &App) -> String {
        self.fields
            .iter()
            .find_map(|(l, widget, _)| match widget {
                Widget::Text(input) if l.as_ref() == label => {
                    Some(input.read(cx).value().to_string())
                }
                _ => None,
            })
            .unwrap_or_default()
    }
    fn num(&self, label: &str, cx: &App) -> Result<f64, String> {
        parse_num(label, &self.text(label, cx))
    }
    /// A quantity typed in display units, as SI.
    fn qty(&self, label: &str, role: Role, cx: &App) -> Result<f64, String> {
        parse_q(role, label, &self.text(label, cx))
    }
    /// A quantity that may be left blank, as SI, or none when blank.
    fn opt_qty(&self, label: &str, role: Role, cx: &App) -> Result<Option<f64>, String> {
        parse_opt_q(role, label, &self.text(label, cx))
    }
    fn choice(&self, label: &str, cx: &App) -> Option<usize> {
        self.fields
            .iter()
            .find_map(|(l, widget, _)| match widget {
                Widget::Choice(select) if l.as_ref() == label => select.read(cx).selected_index(cx),
                _ => None,
            })
            .map(|ix| ix.row)
    }
    fn form(&self) -> Form {
        let mut form = Form::new().small().columns(6).label_text_size(rems(0.8));
        for (label, widget, width) in &self.fields {
            let field = Field::new().col_span(width.span()).label(label.clone());
            form = form.child(match widget {
                Widget::Text(input) => field.child(Input::new(input).small()),
                Widget::Choice(select) => field.child(Select::new(select).small()),
            });
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

/// Opens a dialog whose confirm button, labelled `ok`, runs `on_ok`; Enter
/// in any field does the same. The dialog closes when `on_ok` returns true.
/// The footer is built here because the kit's `Dialog` draws none on its
/// own: only `AlertDialog` turns button props into buttons.
fn open<F>(
    title: &'static str,
    ok: &'static str,
    inputs: Inputs,
    window: &mut Window,
    cx: &mut App,
    on_ok: F,
) where
    F: Fn(&Inputs, &mut Window, &mut App) -> bool + 'static,
{
    let inputs = std::rc::Rc::new(inputs);
    let on_ok = std::rc::Rc::new(on_ok);
    window.open_dialog(cx, move |dialog, _, _| {
        let inputs_for_ok = inputs.clone();
        let on_ok = on_ok.clone();
        dialog
            .title(title)
            .w(px(480.))
            .child(inputs.form())
            .footer(
                DialogFooter::new()
                    .child(
                        Button::new("cancel")
                            .label("Cancel")
                            .on_click(|_, window, cx| window.dispatch_action(Box::new(Cancel), cx)),
                    )
                    .child(
                        Button::new("ok")
                            .primary()
                            .label(ok)
                            .on_click(|_, window, cx| {
                                window.dispatch_action(Box::new(Confirm { secondary: false }), cx)
                            }),
                    ),
            )
            .on_ok(move |_, window, cx| on_ok(&inputs_for_ok, window, cx))
    });
}

/// The name typed, or when it is left blank, `fallback`, numbered on if an
/// entity of this kind already has it: "A992", then "A992 2".
fn name_or<T: oa_model::model::Entity>(model: &Model, typed: String, fallback: &str) -> String {
    if !typed.trim().is_empty() {
        return typed;
    }
    std::iter::once(fallback.to_string())
        .chain((2..).map(|n| format!("{fallback} {n}")))
        .find(|name| model.find::<T>(name).is_none())
        .expect("unbounded")
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

/// A node by plan coordinates on a level, at an offset above its datum.
/// The level list is lowest first and defaults to `active`.
pub fn add_node(
    document: Entity<Document>,
    active: Option<EntityId>,
    window: &mut Window,
    cx: &mut App,
) {
    let (name, levels, selected) = {
        let model = document.read(cx).model();
        let levels: Vec<(EntityId, SharedString)> = model
            .levels_by_elevation()
            .into_iter()
            .map(|id| {
                let l = &model.levels[&id];
                let elevation = crate::text::fmt_q(Role::Length, l.elevation.si());
                let unit = crate::text::UNITS.symbol(Role::Length);
                (id, format!("{} ({elevation} {unit})", l.name).into())
            })
            .collect();
        let selected = active
            .and_then(|a| levels.iter().position(|(id, _)| *id == a))
            .or_else(|| (!levels.is_empty()).then_some(0));
        (unused_name::<Node>(model, "N"), levels, selected)
    };
    let fields = ["X", "Y", "Offset above level"].map(|a| label(a, Role::Length));
    let inputs = Inputs::default()
        .with_text("Name", &name, Width::Full, window, cx)
        .with_choice(
            "Level",
            levels.iter().map(|(_, l)| l.clone()).collect(),
            selected,
            Width::Full,
            window,
            cx,
        )
        .with_text(&fields[0], "0", Width::Third, window, cx)
        .with_text(&fields[1], "0", Width::Third, window, cx)
        .with_text(&fields[2], "0", Width::Third, window, cx);
    open(
        "Add node",
        "Add node",
        inputs,
        window,
        cx,
        move |inputs, window, cx| {
            let Some(level) = inputs.choice("Level", cx).and_then(|ix| levels.get(ix)) else {
                notify_error(window, cx, "Choose a level");
                return false;
            };
            let values = fields.each_ref().map(|l| inputs.qty(l, Role::Length, cx));
            let mut v = [0.0; 3];
            for (i, value) in values.into_iter().enumerate() {
                match value {
                    Ok(x) => v[i] = x,
                    Err(e) => {
                        notify_error(window, cx, e);
                        return false;
                    }
                }
            }
            let model = document.read(cx).model();
            let Some(datum) = model.levels.get(&level.0) else {
                notify_error(window, cx, "That level no longer exists");
                return false;
            };
            let node = Node::new(
                inputs.text("Name", cx),
                level.0,
                [
                    Length::from_si(v[0]),
                    Length::from_si(v[1]),
                    Length::from_si(datum.elevation.si() + v[2]),
                ],
            );
            let id = model.next_id;
            apply(
                &document,
                Command::AddNode {
                    id: EntityId(id),
                    node,
                },
                window,
                cx,
            )
        },
    );
}

/// A rectangular plan grid as one undo step: X grid lines lettered across,
/// Y grid lines numbered up, at spacings typed as a list such as "3@30, 25".
/// `on_added` runs once the grid is in the model.
pub fn add_grid(
    document: Entity<Document>,
    on_added: impl Fn(&mut App) + 'static,
    window: &mut Window,
    cx: &mut App,
) {
    let [x_spacings, y_spacings, origin_x, origin_y, overhang] = [
        "X spacings",
        "Y spacings",
        "Origin X",
        "Origin Y",
        "Overhang",
    ]
    .map(|name| label(name, Role::Length));
    let inputs = Inputs::default()
        .with_text(&x_spacings, "3@30", Width::Full, window, cx)
        .with_text("First X label", "A", Width::Half, window, cx)
        .with_text(&y_spacings, "2@25", Width::Full, window, cx)
        .with_text("First Y label", "1", Width::Half, window, cx)
        .with_text(&origin_x, "0", Width::Third, window, cx)
        .with_text(&origin_y, "0", Width::Third, window, cx)
        .with_text(&overhang, "5", Width::Third, window, cx);
    open(
        "Add grid",
        "Add",
        inputs,
        window,
        cx,
        move |inputs, window, cx| {
            let spacings = |field: &str| {
                oa_model::grids::parse_spacings(&inputs.text(field, cx), |v| {
                    parse_q(Role::Length, field, v)
                })
                .map(|v| v.into_iter().map(Length::from_si).collect::<Vec<_>>())
            };
            let read = || -> Result<oa_model::grids::RectangularGrid, String> {
                Ok(oa_model::grids::RectangularGrid {
                    origin: [
                        Length::from_si(inputs.qty(&origin_x, Role::Length, cx)?),
                        Length::from_si(inputs.qty(&origin_y, Role::Length, cx)?),
                    ],
                    x_spacings: spacings(&x_spacings)?,
                    y_spacings: spacings(&y_spacings)?,
                    x_label: inputs.text("First X label", cx),
                    y_label: inputs.text("First Y label", cx),
                    overhang: Length::from_si(inputs.qty(&overhang, Role::Length, cx)?),
                })
            };
            let command = read().and_then(|grid| {
                grid.command(document.read(cx).model())
                    .map_err(|e| e.to_string())
            });
            match command {
                Ok(command) => {
                    let added = apply(&document, command, window, cx);
                    if added {
                        on_added(cx);
                    }
                    added
                }
                Err(e) => {
                    notify_error(window, cx, e);
                    false
                }
            }
        },
    );
}

/// Lays a drawing read from `path` on a level. The units default to what the
/// file declares and the origin to the model's; both are asked for because
/// many drawings declare no unit and few share the model's origin.
/// `on_imported` runs once the underlay is in the model.
pub fn import_underlay(
    document: Entity<Document>,
    active: Option<EntityId>,
    drawing: crate::cad::Drawing,
    path: &std::path::Path,
    on_imported: impl Fn(&mut App) + 'static,
    window: &mut Window,
    cx: &mut App,
) {
    let (name, levels, selected) = {
        let model = document.read(cx).model();
        let levels: Vec<(EntityId, SharedString)> = model
            .levels_by_elevation()
            .into_iter()
            .map(|id| (id, model.levels[&id].name.clone().into()))
            .collect();
        let selected = active
            .and_then(|a| levels.iter().position(|(id, _)| *id == a))
            .or_else(|| (!levels.is_empty()).then_some(0));
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Underlay".into());
        let name = match model.find::<Underlay>(&stem) {
            Some(_) => unused_name::<Underlay>(model, &format!("{stem} ")),
            None => stem,
        };
        (name, levels, selected)
    };
    let fields = ["Origin X", "Origin Y"].map(|a| label(a, Role::Length));
    let inputs = Inputs::default()
        .with_text("Name", &name, Width::Full, window, cx)
        .with_choice(
            "Level",
            levels.iter().map(|(_, l)| l.clone()).collect(),
            selected,
            Width::Half,
            window,
            cx,
        )
        .with_choice(
            "Drawing units",
            crate::cad::UNITS.iter().map(|(u, _)| (*u).into()).collect(),
            drawing.unit,
            Width::Half,
            window,
            cx,
        )
        .with_text(&fields[0], "0", Width::Half, window, cx)
        .with_text(&fields[1], "0", Width::Half, window, cx);
    open(
        "Import CAD underlay",
        "Import",
        inputs,
        window,
        cx,
        move |inputs, window, cx| {
            let Some(level) = inputs.choice("Level", cx).and_then(|ix| levels.get(ix)) else {
                notify_error(window, cx, "Choose a level");
                return false;
            };
            let Some((_, metres)) = inputs
                .choice("Drawing units", cx)
                .and_then(|ix| crate::cad::UNITS.get(ix))
            else {
                notify_error(window, cx, "Choose the units the drawing was made in");
                return false;
            };
            let mut origin = [Length::ZERO; 2];
            for (i, field) in fields.iter().enumerate() {
                match inputs.qty(field, Role::Length, cx) {
                    Ok(v) => origin[i] = Length::from_si(v),
                    Err(e) => {
                        notify_error(window, cx, e);
                        return false;
                    }
                }
            }
            let underlay = Underlay {
                name: inputs.text("Name", cx),
                level: level.0,
                origin,
                segments: drawing
                    .segments
                    .iter()
                    .map(|s| s.map(|p| p.map(|v| Length::from_si(v * metres))))
                    .collect(),
            };
            let id = EntityId(document.read(cx).model().next_id);
            if !apply(&document, Command::AddUnderlay { id, underlay }, window, cx) {
                return false;
            }
            let mut message = format!("Imported {} segments", drawing.segments.len());
            if drawing.skipped > 0 {
                message += &format!(
                    "; skipped {} entities that are not line work",
                    drawing.skipped
                );
            }
            window.push_notification(Notification::info(message), cx);
            on_imported(cx);
            true
        },
    );
}

pub fn add_material_from_library(document: Entity<Document>, window: &mut Window, cx: &mut App) {
    let library = Library::starter();
    let designations: Vec<SharedString> = library
        .materials
        .iter()
        .map(|m| m.designation.clone().into())
        .collect();
    let inputs = Inputs::default()
        .with_choice(
            "Library material",
            designations,
            Some(0),
            Width::Full,
            window,
            cx,
        )
        .with_optional_name("Same as the library material", window, cx);
    open(
        "Add material from library",
        "Add material",
        inputs,
        window,
        cx,
        move |inputs, window, cx| {
            let Some(ix) = inputs.choice("Library material", cx) else {
                notify_error(window, cx, "Choose a library material");
                return false;
            };
            let designation = library.materials[ix].designation.clone();
            let model = document.read(cx).model();
            let name = name_or::<Material>(model, inputs.text("Name", cx), &designation);
            let material = library
                .material(&designation, name)
                .expect("designation from the list");
            let id = model.next_id;
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
    let (young, density) = (label("E", Role::Stress), label("Density", Role::Density));
    let [fy, fu, fc] = ["Fy", "Fu", "f'c"].map(|name| label(name, Role::Stress));
    let inputs = Inputs::default()
        .with_text("Name", &name, Width::Full, window, cx)
        .with_text(&young, "29000", Width::Third, window, cx)
        .with_text("Poisson's ratio", "0.3", Width::Third, window, cx)
        .with_text(&density, "490", Width::Third, window, cx)
        .with_optional_text(&fy, "50", "None", Width::Third, window, cx)
        .with_optional_text(&fu, "65", "None", Width::Third, window, cx)
        .with_optional_text(&fc, "", "None", Width::Third, window, cx);
    open(
        "Add material",
        "Add material",
        inputs,
        window,
        cx,
        move |inputs, window, cx| {
            let values = (
                inputs.qty(&young, Role::Stress, cx),
                inputs.num("Poisson's ratio", cx),
                inputs.qty(&density, Role::Density, cx),
            );
            let (young, poisson, density) = match values {
                (Ok(e), Ok(nu), Ok(rho)) => (e, nu, rho),
                (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => {
                    notify_error(window, cx, e);
                    return false;
                }
            };
            let strengths = (
                inputs.opt_qty(&fy, Role::Stress, cx),
                inputs.opt_qty(&fu, Role::Stress, cx),
                inputs.opt_qty(&fc, Role::Stress, cx),
            );
            let (fy, fu, fc) = match strengths {
                (Ok(fy), Ok(fu), Ok(fc)) => (fy, fu, fc),
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
                fy: fy.map(Pressure::from_si),
                fu: fu.map(Pressure::from_si),
                fc: fc.map(Pressure::from_si),
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
    let library = Library::aisc();
    let designations: Vec<SharedString> = library
        .section_designations()
        .into_iter()
        .map(|s| s.to_string().into())
        .collect();
    let inputs = Inputs::default()
        .with_choice(
            "Library section",
            designations,
            Some(0),
            Width::Full,
            window,
            cx,
        )
        .with_optional_name("Same as the library section", window, cx);
    open(
        "Add section from library",
        "Add section",
        inputs,
        window,
        cx,
        move |inputs, window, cx| {
            let Some(ix) = inputs.choice("Library section", cx) else {
                notify_error(window, cx, "Choose a library section");
                return false;
            };
            let designation = library.sections[ix].designation.clone();
            let model = document.read(cx).model();
            let name = name_or::<Section>(model, inputs.text("Name", cx), &designation);
            let section = library
                .section(&designation, name)
                .expect("designation from the list");
            let id = model.next_id;
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
    let area = label("Area", Role::Area);
    let moments = ["Iy", "Iz", "J"].map(|l| label(l, Role::SecondMoment));
    let shears = ["Shear area y", "Shear area z"].map(|l| label(l, Role::Area));
    let inputs = Inputs::default()
        .with_text("Name", &name, Width::Full, window, cx)
        .with_text(&area, "10", Width::Full, window, cx)
        .with_text(&moments[0], "50", Width::Third, window, cx)
        .with_text(&moments[1], "200", Width::Third, window, cx)
        .with_text(&moments[2], "1", Width::Third, window, cx)
        .with_optional_text(&shears[0], "", "Rigid", Width::Half, window, cx)
        .with_optional_text(&shears[1], "", "Rigid", Width::Half, window, cx);
    open(
        "Add section",
        "Add section",
        inputs,
        window,
        cx,
        move |inputs, window, cx| {
            let values = [
                inputs.qty(&area, Role::Area, cx),
                inputs.qty(&moments[0], Role::SecondMoment, cx),
                inputs.qty(&moments[1], Role::SecondMoment, cx),
                inputs.qty(&moments[2], Role::SecondMoment, cx),
            ];
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
            let (shear_y, shear_z) = match (
                inputs.opt_qty(&shears[0], Role::Area, cx),
                inputs.opt_qty(&shears[1], Role::Area, cx),
            ) {
                (Ok(y), Ok(z)) => (y, z),
                (Err(e), _) | (_, Err(e)) => {
                    notify_error(window, cx, e);
                    return false;
                }
            };
            let section = Section {
                name: inputs.text("Name", cx),
                area: Area::from_si(v[0]),
                iy: SecondMoment::from_si(v[1]),
                iz: SecondMoment::from_si(v[2]),
                torsion: SecondMoment::from_si(v[3]),
                shear_y: shear_y.map(Area::from_si),
                shear_z: shear_z.map(Area::from_si),
                shape: None,
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

/// Adds a load case of one of the nominal loads of ASCE 7 chapter 2 (the
/// list is the same in 7-16 and 7-22), named after the load. The first dead
/// case in a model gets the self weight, acting down Z.
pub fn add_asce_load_case(document: Entity<Document>, window: &mut Window, cx: &mut App) {
    let types: Vec<LoadType> = LoadType::ALL
        .into_iter()
        .filter(|t| *t != LoadType::Other)
        .collect();
    let options = types
        .iter()
        .map(|t| crate::loads::type_label(*t).into())
        .collect();
    let inputs = Inputs::default().with_choice("Load", options, Some(0), Width::Full, window, cx);
    open(
        "Add ASCE 7 load case",
        "Add load case",
        inputs,
        window,
        cx,
        move |inputs, window, cx| {
            let Some(ix) = inputs.choice("Load", cx) else {
                notify_error(window, cx, "Choose a load");
                return false;
            };
            let load_type = types[ix];
            let model = document.read(cx).model();
            let name = name_or::<LoadCase>(model, String::new(), load_type.label());
            let mut load_case = LoadCase::new(name).with_type(load_type);
            let carries_self_weight = model.load_cases.values().any(|c| c.self_weight != [0.0; 3]);
            if load_type == LoadType::Dead && !carries_self_weight {
                load_case.self_weight = [0.0, 0.0, -1.0];
            }
            let id = EntityId(model.next_id);
            apply(
                &document,
                Command::AddLoadCase { id, load_case },
                window,
                cx,
            )
        },
    );
}

/// Generates the ASCE 7 combinations the defined load cases can form, for
/// the chosen edition and method, skipping any the model already has.
pub fn generate_combinations(document: Entity<Document>, window: &mut Window, cx: &mut App) {
    if document.read(cx).model().load_cases.is_empty() {
        notify_error(window, cx, "Define a load case first (Define > Load cases)");
        return;
    }
    let inputs = Inputs::default()
        .with_choice(
            "Edition",
            vec!["ASCE 7-16".into(), "ASCE 7-22".into()],
            Some(1),
            Width::Half,
            window,
            cx,
        )
        .with_choice(
            "Method",
            vec!["Strength (LRFD)".into(), "Allowable stress (ASD)".into()],
            Some(0),
            Width::Half,
            window,
            cx,
        );
    open(
        "Generate load combinations",
        "Generate",
        inputs,
        window,
        cx,
        move |inputs, window, cx| {
            let edition = match inputs.choice("Edition", cx) {
                Some(0) => Edition::Asce7_16,
                _ => Edition::Asce7_22,
            };
            let method = match inputs.choice("Method", cx) {
                Some(1) => Method::AllowableStress,
                _ => Method::Strength,
            };
            let commands = oa_model::asce7::commands(document.read(cx).model(), edition, method);
            let count = commands.len();
            if count == 0 {
                window.push_notification(
                    Notification::info(
                        "No new combinations: every one the defined cases can form already exists",
                    ),
                    cx,
                );
                return true;
            }
            if apply(&document, Command::Batch { commands }, window, cx) {
                window.push_notification(
                    Notification::info(format!("Added {count} combinations")),
                    cx,
                );
                true
            } else {
                false
            }
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
        notify_error(window, cx, "Define a load case first (Define > Load cases)");
        return;
    }
    if nodes.is_empty() {
        notify_error(window, cx, "Select the nodes to load first");
        return;
    }
    let forces = ["Fx", "Fy", "Fz"].map(|l| label(l, Role::Force));
    let moments = ["Mx", "My", "Mz"].map(|l| label(l, Role::Moment));
    let mut inputs =
        Inputs::default().with_choice("Load case", cases, selected_case, Width::Full, window, cx);
    for label in forces.iter().chain(&moments) {
        inputs = inputs.with_text(label, "0", Width::Third, window, cx);
    }
    open(
        "Add nodal load to selected nodes",
        "Add load",
        inputs,
        window,
        cx,
        move |inputs, window, cx| {
            let values = [
                inputs.qty(&forces[0], Role::Force, cx),
                inputs.qty(&forces[1], Role::Force, cx),
                inputs.qty(&forces[2], Role::Force, cx),
                inputs.qty(&moments[0], Role::Moment, cx),
                inputs.qty(&moments[1], Role::Moment, cx),
                inputs.qty(&moments[2], Role::Moment, cx),
            ];
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
        notify_error(window, cx, "Define a load case first (Define > Load cases)");
        return;
    }
    if frames.is_empty() {
        notify_error(window, cx, "Select the frames to load first");
        return;
    }
    let loads = ["wx", "wy", "wz"].map(|l| label(l, Role::LineLoad));
    let inputs = Inputs::default()
        .with_choice("Load case", cases, selected_case, Width::Half, window, cx)
        .with_choice(
            "Axes",
            vec!["Global".into(), "Local".into()],
            Some(0),
            Width::Half,
            window,
            cx,
        )
        .with_text(&loads[0], "0", Width::Third, window, cx)
        .with_text(&loads[1], "0", Width::Third, window, cx)
        .with_text(&loads[2], "-1", Width::Third, window, cx);
    open(
        "Add uniform load to selected frames",
        "Add load",
        inputs,
        window,
        cx,
        move |inputs, window, cx| {
            let values = loads.each_ref().map(|l| inputs.qty(l, Role::LineLoad, cx));
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
                    end: Length::from_si(length),
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

#[cfg(test)]
mod tests {
    use super::name_or;
    use oa_model::{Library, Material, Model};

    #[test]
    fn a_blank_name_takes_the_designation_numbered_on() {
        let mut model = Model::default();
        let typed = name_or::<Material>(&model, "Steel".into(), "A992");
        assert_eq!(typed, "Steel");
        assert_eq!(name_or::<Material>(&model, "  ".into(), "A992"), "A992");
        let library = Library::starter();
        model.insert(library.material("A992", "A992").unwrap());
        assert_eq!(name_or::<Material>(&model, String::new(), "A992"), "A992 2");
    }
}
