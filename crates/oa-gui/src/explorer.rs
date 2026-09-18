//! The model tree: every entity kind as a collapsible section, with rows that
//! select the entity; a double-click opens its properties. Mirrors the
//! viewport's selection. Shown in a dialog from View > Model browser.
use crate::actions::ShowProperties;
use crate::document::Document;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use oa_model::{EntityId, EntityKind, Model};
use std::collections::BTreeSet;

/// Rows shown per section before the list is cut, to keep large models responsive.
const MAX_ROWS: usize = 2000;

const KINDS: [EntityKind; 10] = [
    EntityKind::Level,
    EntityKind::Node,
    EntityKind::Frame,
    EntityKind::Shell,
    EntityKind::Material,
    EntityKind::Section,
    EntityKind::LoadCase,
    EntityKind::Combination,
    EntityKind::Diaphragm,
    EntityKind::Group,
];

fn section_title(kind: EntityKind) -> &'static str {
    match kind {
        EntityKind::Level => "Levels",
        EntityKind::Node => "Nodes",
        EntityKind::Material => "Materials",
        EntityKind::Section => "Sections",
        EntityKind::Frame => "Frames",
        EntityKind::Shell => "Shells",
        EntityKind::Diaphragm => "Diaphragms",
        EntityKind::LoadCase => "Load cases",
        EntityKind::Combination => "Combinations",
        EntityKind::Group => "Groups",
    }
}

/// Ids and names of one kind, in table order.
pub fn rows_of(model: &Model, kind: EntityKind) -> Vec<(EntityId, String)> {
    fn rows<T: oa_model::model::Entity>(model: &Model) -> Vec<(EntityId, String)> {
        T::table(model)
            .iter()
            .map(|(id, e)| (*id, e.name().to_string()))
            .collect()
    }
    match kind {
        EntityKind::Level => rows::<oa_model::Level>(model),
        EntityKind::Node => rows::<oa_model::Node>(model),
        EntityKind::Material => rows::<oa_model::Material>(model),
        EntityKind::Section => rows::<oa_model::Section>(model),
        EntityKind::Frame => rows::<oa_model::Frame>(model),
        EntityKind::Shell => rows::<oa_model::Shell>(model),
        EntityKind::Diaphragm => rows::<oa_model::Diaphragm>(model),
        EntityKind::LoadCase => rows::<oa_model::LoadCase>(model),
        EntityKind::Combination => rows::<oa_model::Combination>(model),
        EntityKind::Group => rows::<oa_model::Group>(model),
    }
}

pub struct Explorer {
    document: Entity<Document>,
    expanded: BTreeSet<EntityKind>,
    _subscriptions: Vec<Subscription>,
}

impl Explorer {
    pub fn new(document: Entity<Document>, cx: &mut Context<Self>) -> Self {
        let subscription = cx.observe(&document, |_, _, cx| cx.notify());
        Self {
            document,
            expanded: [
                EntityKind::Level,
                EntityKind::Material,
                EntityKind::Section,
                EntityKind::LoadCase,
                EntityKind::Combination,
            ]
            .into_iter()
            .collect(),
            _subscriptions: vec![subscription],
        }
    }

    fn toggle_section(&mut self, kind: EntityKind, cx: &mut Context<Self>) {
        if !self.expanded.remove(&kind) {
            self.expanded.insert(kind);
        }
        cx.notify();
    }

    fn select(&mut self, id: EntityId, extend: bool, cx: &mut Context<Self>) {
        self.document.update(cx, |document, cx| {
            if extend {
                document.toggle_selected(id, cx);
            } else {
                document.set_selection(vec![id], cx);
            }
        });
    }

    fn render_section(&self, kind: EntityKind, cx: &mut Context<Self>) -> AnyElement {
        let document = self.document.read(cx);
        let rows = rows_of(document.model(), kind);
        let expanded = self.expanded.contains(&kind);
        let selected: Vec<bool> = rows
            .iter()
            .map(|(id, _)| document.is_selected(*id))
            .collect();
        let theme = cx.theme();
        let (hover, active, muted, border) = (
            theme.list_hover,
            theme.list_active,
            theme.muted_foreground,
            theme.border,
        );
        let count = rows.len();
        v_flex()
            .child(
                h_flex()
                    .id(("section", kind as usize))
                    .gap_1()
                    .px_2()
                    .py_1()
                    .border_b_1()
                    .border_color(border)
                    .text_sm()
                    .font_weight(FontWeight::MEDIUM)
                    .hover(|s| s.bg(hover))
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle_section(kind, cx)))
                    .child(
                        Icon::new(if expanded {
                            IconName::ChevronDown
                        } else {
                            IconName::ChevronRight
                        })
                        .size_3p5(),
                    )
                    .child(section_title(kind))
                    .child(
                        div()
                            .ml_auto()
                            .text_xs()
                            .text_color(muted)
                            .child(count.to_string()),
                    ),
            )
            .when(expanded, |this| {
                this.children(rows.into_iter().zip(selected).take(MAX_ROWS).map(
                    |((id, name), is_selected)| {
                        div()
                            .id(("entity", id.0))
                            .pl_6()
                            .pr_2()
                            .py_0p5()
                            .text_sm()
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_ellipsis()
                            .when(is_selected, |s| s.bg(active))
                            .hover(|s| s.bg(hover))
                            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                                this.select(id, event.modifiers().shift, cx);
                                if event.click_count() == 2 {
                                    window.dispatch_action(Box::new(ShowProperties), cx);
                                }
                            }))
                            .child(name)
                    },
                ))
                .when(count > MAX_ROWS, |this| {
                    this.child(
                        div()
                            .pl_6()
                            .py_0p5()
                            .text_xs()
                            .text_color(muted)
                            .child(format!("{} more not listed", count - MAX_ROWS)),
                    )
                })
            })
            .into_any_element()
    }
}

impl Render for Explorer {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (bg, fg) = (theme.sidebar, theme.sidebar_foreground);
        let sections: Vec<AnyElement> = KINDS
            .iter()
            .map(|kind| self.render_section(*kind, cx))
            .collect();
        v_flex()
            .size_full()
            .bg(bg)
            .text_color(fg)
            .child(
                div()
                    .px_3()
                    .py_2()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Model"),
            )
            .child(
                v_flex()
                    .id("explorer-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .children(sections),
            )
    }
}
