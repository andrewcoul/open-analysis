//! Load cases and load combinations as editable tables, each in its own
//! dialog from the Define menu (Ctrl+L and Ctrl+Shift+L). The cases table
//! has a row per case with its name, ASCE 7 load type, and self-weight
//! vector; the combinations table has a row per combination and a column per
//! case holding the factor, blank where the case is not in the combination.
//! Every cell commits on Enter or blur as an undoable command. Cases can come
//! from the ASCE 7 list or be the user's own, and combinations can be
//! generated from ASCE 7-16 or 7-22 using only the cases that exist.
use crate::actions::{AddAsceLoadCase, GenerateCombinations};
use crate::document::{Document, unused_name};
use crate::text::{fmt_num, parse_num};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::select::{SearchableVec, Select, SelectEvent, SelectState};
use gpui_kit::component::table::{Table, TableBody, TableCell, TableHead, TableHeader, TableRow};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, IndexPath, Sizable as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::*;
use oa_model::{Combination, Command, EntityId, LoadCase, LoadType};

type Choice = Entity<SelectState<SearchableVec<SharedString>>>;

/// "Roof live (Lr)", or just "Other".
pub fn type_label(load_type: LoadType) -> String {
    match load_type.symbol() {
        "" => load_type.label().to_string(),
        symbol => format!("{} ({symbol})", load_type.label()),
    }
}

/// "0, 0, -1" for a self-weight vector.
fn fmt_vec3(v: [f64; 3]) -> String {
    v.map(fmt_num).join(", ")
}

fn parse_vec3(text: &str) -> Result<[f64; 3], String> {
    let parts: Vec<&str> = text
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|s| !s.is_empty())
        .collect();
    let [x, y, z] = parts.as_slice() else {
        return Err(format!(
            "Self weight: {text:?} should be three numbers, like 0, 0, -1"
        ));
    };
    Ok([
        parse_num("Self weight", x)?,
        parse_num("Self weight", y)?,
        parse_num("Self weight", z)?,
    ])
}

struct CaseRow {
    id: EntityId,
    name: Entity<InputState>,
    load_type: Choice,
    self_weight: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
}

struct ComboRow {
    id: EntityId,
    name: Entity<InputState>,
    /// One factor input per load case, in the cases' table order.
    factors: Vec<(EntityId, Entity<InputState>)>,
    _subscriptions: Vec<Subscription>,
}

/// Which table a panel shows.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Cases,
    Combinations,
}

pub struct LoadPanel {
    document: Entity<Document>,
    section: Section,
    /// Revision the rows were built for.
    built_for: u64,
    /// Rows of the cases table; empty in a combinations panel.
    cases: Vec<CaseRow>,
    /// The cases as combination columns: id and header label.
    columns: Vec<(EntityId, String)>,
    combos: Vec<ComboRow>,
    /// Input to focus after the next rebuild, so Enter keeps the caret in place.
    pending_focus: Option<String>,
    _subscriptions: Vec<Subscription>,
}

type Commit = fn(&mut LoadPanel, EntityId, &mut Window, &mut Context<LoadPanel>);
/// A load case as the rows need it: id, name, type, self weight.
type CaseData = (EntityId, String, LoadType, [f64; 3]);
/// A combination as the rows need it: id, name, terms.
type ComboData = (EntityId, String, Vec<(EntityId, f64)>);

impl LoadPanel {
    pub fn new(
        document: Entity<Document>,
        section: Section,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let subscription = cx.observe_in(&document, window, |this, _, window, cx| {
            this.sync(window, cx)
        });
        let mut this = Self {
            document,
            section,
            built_for: u64::MAX,
            cases: vec![],
            columns: vec![],
            combos: vec![],
            pending_focus: None,
            _subscriptions: vec![subscription],
        };
        this.sync(window, cx);
        this
    }

    /// Rebuilds the rows when the model changed.
    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let revision = self.document.read(cx).revision();
        if self.built_for == revision {
            return;
        }
        self.built_for = revision;
        self.build(window, cx);
        if let Some(key) = self.pending_focus.take()
            && let Some(input) = self.input_for(&key)
        {
            input.update(cx, |input, cx| input.focus(window, cx));
        }
        cx.notify();
    }

    fn input_for(&self, key: &str) -> Option<Entity<InputState>> {
        let case_inputs = self.cases.iter().flat_map(|r| {
            [
                (format!("case-name-{}", r.id.0), &r.name),
                (format!("case-weight-{}", r.id.0), &r.self_weight),
            ]
        });
        let combo_inputs = self.combos.iter().flat_map(|r| {
            std::iter::once((format!("combo-name-{}", r.id.0), &r.name)).chain(
                r.factors
                    .iter()
                    .map(move |(case, input)| (format!("factor-{}-{}", r.id.0, case.0), input)),
            )
        });
        case_inputs
            .chain(combo_inputs)
            .find(|(k, _)| k == key)
            .map(|(_, input)| input.clone())
    }

    /// A text input that commits its row on Enter or blur.
    fn text_input(
        &self,
        key: String,
        value: String,
        id: EntityId,
        commit: Commit,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (Entity<InputState>, Subscription) {
        let input = cx.new(|cx| InputState::new(window, cx).default_value(value));
        let subscription = cx.subscribe_in(
            &input,
            window,
            move |this, _, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { .. } => {
                    this.pending_focus = Some(key.clone());
                    commit(this, id, window, cx);
                }
                InputEvent::Blur => commit(this, id, window, cx),
                _ => {}
            },
        );
        (input, subscription)
    }

    fn build(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (cases, combos) = {
            let model = self.document.read(cx).model();
            let cases: Vec<CaseData> = model
                .load_cases
                .iter()
                .map(|(id, c)| (*id, c.name.clone(), c.load_type, c.self_weight))
                .collect();
            let combos: Vec<ComboData> = model
                .combinations
                .iter()
                .map(|(id, c)| (*id, c.name.clone(), c.terms.clone()))
                .collect();
            (cases, combos)
        };
        self.columns = cases
            .iter()
            .map(|(id, name, load_type, _)| {
                let head = match load_type.symbol() {
                    "" => name.clone(),
                    symbol => format!("{name} ({symbol})"),
                };
                (*id, head)
            })
            .collect();
        if self.section == Section::Combinations {
            self.cases.clear();
            self.combos = self.build_combos(&cases, &combos, window, cx);
            return;
        }
        self.combos.clear();
        let type_options: Vec<SharedString> = LoadType::ALL
            .into_iter()
            .map(|t| type_label(t).into())
            .collect();
        self.cases = cases
            .iter()
            .map(|(id, name, load_type, self_weight)| {
                let id = *id;
                let (name, s1) = self.text_input(
                    format!("case-name-{}", id.0),
                    name.clone(),
                    id,
                    Self::commit_case,
                    window,
                    cx,
                );
                let (self_weight, s2) = self.text_input(
                    format!("case-weight-{}", id.0),
                    fmt_vec3(*self_weight),
                    id,
                    Self::commit_case,
                    window,
                    cx,
                );
                let selected = LoadType::ALL.iter().position(|t| t == load_type);
                let load_type = cx.new(|cx| {
                    SelectState::new(
                        SearchableVec::from(type_options.clone()),
                        selected.map(IndexPath::new),
                        window,
                        cx,
                    )
                });
                let s3 = cx.subscribe_in(
                    &load_type,
                    window,
                    move |this, _, _: &SelectEvent<SearchableVec<SharedString>>, window, cx| {
                        this.commit_case(id, window, cx)
                    },
                );
                CaseRow {
                    id,
                    name,
                    load_type,
                    self_weight,
                    _subscriptions: vec![s1, s2, s3],
                }
            })
            .collect();
    }

    fn build_combos(
        &self,
        cases: &[CaseData],
        combos: &[ComboData],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<ComboRow> {
        combos
            .iter()
            .map(|(id, name, terms)| {
                let id = *id;
                let (name, s) = self.text_input(
                    format!("combo-name-{}", id.0),
                    name.clone(),
                    id,
                    Self::commit_combo,
                    window,
                    cx,
                );
                let mut subscriptions = vec![s];
                let factors = cases
                    .iter()
                    .map(|(case, ..)| {
                        let factor = terms
                            .iter()
                            .find(|(c, _)| c == case)
                            .map(|(_, f)| fmt_num(*f))
                            .unwrap_or_default();
                        let (input, s) = self.text_input(
                            format!("factor-{}-{}", id.0, case.0),
                            factor,
                            id,
                            Self::commit_combo,
                            window,
                            cx,
                        );
                        subscriptions.push(s);
                        (*case, input)
                    })
                    .collect();
                ComboRow {
                    id,
                    name,
                    factors,
                    _subscriptions: subscriptions,
                }
            })
            .collect()
    }

    // MARK: Editing

    fn error(&self, message: impl Into<SharedString>, window: &mut Window, cx: &mut App) {
        window.push_notification(Notification::error(message), cx);
    }
    fn apply(&mut self, command: Command, window: &mut Window, cx: &mut Context<Self>) {
        let result = self
            .document
            .update(cx, |document, cx| document.apply(command, cx));
        if let Err(e) = result {
            self.error(e.to_string(), window, cx);
        }
    }

    /// Reads a case's row and applies the update if anything changed.
    fn commit_case(&mut self, id: EntityId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.cases.iter().find(|r| r.id == id) else {
            return;
        };
        let name = row.name.read(cx).value().to_string();
        let load_type = row
            .load_type
            .read(cx)
            .selected_index(cx)
            .map(|ix| LoadType::ALL[ix.row])
            .unwrap_or(LoadType::Other);
        let self_weight = match parse_vec3(&row.self_weight.read(cx).value()) {
            Ok(v) => v,
            Err(e) => return self.error(e, window, cx),
        };
        let command = {
            let model = self.document.read(cx).model();
            let Some(current) = model.load_cases.get(&id) else {
                return;
            };
            let mut load_case = current.clone();
            load_case.name = name;
            load_case.load_type = load_type;
            load_case.self_weight = self_weight;
            (load_case != *current).then_some(Command::UpdateLoadCase { id, load_case })
        };
        if let Some(command) = command {
            self.apply(command, window, cx);
        }
    }

    /// Reads a combination's row: a blank factor leaves the case out.
    fn commit_combo(&mut self, id: EntityId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.combos.iter().find(|r| r.id == id) else {
            return;
        };
        let name = row.name.read(cx).value().to_string();
        let mut terms = vec![];
        for (case, input) in &row.factors {
            let text = input.read(cx).value();
            let text = text.trim();
            if text.is_empty() {
                continue;
            }
            match parse_num("Factor", text) {
                Ok(f) => terms.push((*case, f)),
                Err(e) => return self.error(e, window, cx),
            }
        }
        let command = {
            let model = self.document.read(cx).model();
            let Some(current) = model.combinations.get(&id) else {
                return;
            };
            let combination = Combination { name, terms };
            (combination != *current).then_some(Command::UpdateCombination { id, combination })
        };
        if let Some(command) = command {
            self.apply(command, window, cx);
        }
    }

    fn add_case(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let model = self.document.read(cx).model();
        let load_case = LoadCase::new(unused_name::<LoadCase>(model, "LC"));
        let id = EntityId(model.next_id);
        self.pending_focus = Some(format!("case-name-{}", id.0));
        self.apply(Command::AddLoadCase { id, load_case }, window, cx);
    }

    /// Removes a case, dropping it from every combination first.
    fn remove_case(&mut self, id: EntityId, window: &mut Window, cx: &mut Context<Self>) {
        let commands = {
            let model = self.document.read(cx).model();
            let mut commands: Vec<Command> = model
                .combinations
                .iter()
                .filter(|(_, c)| c.terms.iter().any(|(case, _)| *case == id))
                .map(|(cid, c)| {
                    let mut combination = c.clone();
                    combination.terms.retain(|(case, _)| *case != id);
                    Command::UpdateCombination {
                        id: *cid,
                        combination,
                    }
                })
                .collect();
            commands.push(Command::RemoveLoadCase { id });
            commands
        };
        self.apply(Command::Batch { commands }, window, cx);
    }

    fn add_combo(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let model = self.document.read(cx).model();
        let combination = Combination {
            name: unused_name::<Combination>(model, "COMB"),
            terms: model
                .load_cases
                .keys()
                .next()
                .map(|c| (*c, 1.0))
                .into_iter()
                .collect(),
        };
        let id = EntityId(model.next_id);
        self.pending_focus = Some(format!("combo-name-{}", id.0));
        self.apply(Command::AddCombination { id, combination }, window, cx);
    }

    fn remove_combo(&mut self, id: EntityId, window: &mut Window, cx: &mut Context<Self>) {
        self.apply(Command::RemoveCombination { id }, window, cx);
    }

    // MARK: Rendering

    fn render_cases(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.cases.is_empty() {
            return div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child("No load cases yet.")
                .into_any_element();
        }
        let header = TableRow::new()
            .child(TableHead::new().child("Name"))
            .child(TableHead::new().child("Type"))
            .child(TableHead::new().child("Self weight (x, y, z)"))
            .child(TableHead::new().child(""));
        let rows = self.cases.iter().enumerate().map(|(ix, row)| {
            let id = row.id;
            TableRow::new()
                .child(TableCell::new().child(Input::new(&row.name).small()))
                .child(TableCell::new().child(Select::new(&row.load_type).small()))
                .child(TableCell::new().child(Input::new(&row.self_weight).small()))
                .child(
                    TableCell::new().child(
                        Button::new(("remove-case", ix))
                            .xsmall()
                            .danger()
                            .outline()
                            .label("Remove")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.remove_case(id, window, cx)
                            })),
                    ),
                )
        });
        Table::new()
            .small()
            .child(TableHeader::new().child(header))
            .child(TableBody::new().children(rows))
            .into_any_element()
    }

    fn render_combos(&self, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        if self.combos.is_empty() {
            return div()
                .text_sm()
                .text_color(muted)
                .child("No combinations yet.")
                .into_any_element();
        }
        let mut header = TableRow::new().child(TableHead::new().child("Name"));
        for (_, head) in &self.columns {
            header = header.child(TableHead::new().child(head.clone()));
        }
        header = header.child(TableHead::new().child(""));
        let rows = self.combos.iter().enumerate().map(|(ix, row)| {
            let id = row.id;
            let mut tr = TableRow::new().child(TableCell::new().child(Input::new(&row.name).small()));
            for (_, factor) in &row.factors {
                tr = tr.child(TableCell::new().child(Input::new(factor).small()));
            }
            tr.child(
                TableCell::new().child(
                    Button::new(("remove-combo", ix))
                        .xsmall()
                        .danger()
                        .outline()
                        .label("Remove")
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.remove_combo(id, window, cx)
                        })),
                ),
            )
        });
        Table::new()
            .small()
            .child(TableHeader::new().child(header))
            .child(TableBody::new().children(rows))
            .into_any_element()
    }
}

impl Render for LoadPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let heading = move |text: &'static str| div().text_xs().text_color(muted).child(text);
        let button = |id: &'static str, label: &'static str| {
            Button::new(id).small().outline().label(label)
        };
        let content = match self.section {
            Section::Cases => v_flex()
                .gap_2()
                .child(heading("Name, ASCE 7 load type, and self-weight multipliers per axis."))
                .child(self.render_cases(cx))
                .child(
                    h_flex()
                        .gap_2()
                        .flex_wrap()
                        .child(button("add-case", "Add load case").on_click(cx.listener(
                            |this, _, window, cx| this.add_case(window, cx),
                        )))
                        .child(button("add-asce-case", "Add ASCE 7 load case…").on_click(
                            |_, window, cx| window.dispatch_action(Box::new(AddAsceLoadCase), cx),
                        )),
                ),
            Section::Combinations => v_flex()
                .gap_2()
                .child(heading("A factor per case; leave a cell blank to leave the case out."))
                .child(self.render_combos(cx))
                .child(
                    h_flex()
                        .gap_2()
                        .flex_wrap()
                        .child(button("add-combo", "Add combination").on_click(cx.listener(
                            |this, _, window, cx| this.add_combo(window, cx),
                        )))
                        .child(
                            button("generate-combos", "Generate from ASCE 7…")
                                .disabled(self.columns.is_empty())
                                .on_click(|_, window, cx| {
                                    window.dispatch_action(Box::new(GenerateCombinations), cx)
                                }),
                        ),
                ),
        };
        v_flex()
            .id("loads-scroll")
            .size_full()
            .overflow_scrollbar()
            .child(content)
    }
}

#[cfg(test)]
mod tests {
    // Named imports: the gpui glob carries its own `test` attribute macro.
    use super::{fmt_vec3, parse_vec3, type_label};
    use oa_model::LoadType;

    #[test]
    fn self_weight_round_trips() {
        assert_eq!(fmt_vec3([0.0, 0.0, -1.0]), "0, 0, -1");
        assert_eq!(parse_vec3("0, 0, -1").unwrap(), [0.0, 0.0, -1.0]);
        assert_eq!(parse_vec3(" 0 0 -9.81 ").unwrap(), [0.0, 0.0, -9.81]);
        assert!(parse_vec3("0, 0").is_err());
        assert!(parse_vec3("0, 0, x").is_err());
    }

    #[test]
    fn type_labels_carry_the_symbol() {
        assert_eq!(type_label(LoadType::RoofLive), "Roof live (Lr)");
        assert_eq!(type_label(LoadType::Other), "Other");
    }
}
