use std::mem::{replace, take};

use egui::{Align, Color32, Event, Label, Layout, PointerButton, PopupAnchor, Rect, Response, RichText, Sense, Stroke, StrokeKind, Tooltip, Vec2b};
use egui_extras::Column;
use tap::prelude::{Pipe, Tap};

use crate::{
    viewer::{EmptyRowCreateContext, RowViewer},
    DataTable, UiAction,
};

use self::state::*;

use format as f;
use std::sync::Arc;
use egui::scroll_area::ScrollBarVisibility;

pub(crate) mod state;
mod tsv;

/* -------------------------------------------- Style ------------------------------------------- */

/// Style configuration for the table.
// TODO: Implement more style configurations.
#[derive(Default, Debug, Clone, Copy)]
#[non_exhaustive]
pub struct Style {
    /// Background color override for selection. Default uses `visuals.selection.bg_fill`.
    pub bg_selected_cell: Option<egui::Color32>,

    /// Background color override for selected cell. Default uses `visuals.selection.bg_fill`.
    pub bg_selected_highlight_cell: Option<egui::Color32>,

    /// Foreground color override for selected cell. Default uses `visuals.strong_text_colors`.
    pub fg_selected_highlight_cell: Option<egui::Color32>,

    /// Foreground color for cells that are going to be selected when mouse is dropped.
    pub fg_drag_selection: Option<egui::Color32>,

    /* ·························································································· */
    /// Maximum number of undo history. This is applied when actual action is performed.
    ///
    /// Setting value '0' results in kinda appropriate default value.
    pub max_undo_history: usize,

    /// If specify this as [`None`], the heterogeneous row height will be used.
    pub table_row_height: Option<f32>,

    /// When enabled, single click on a cell will start editing mode. Default is `false` where
    /// double action(click 1: select, click 2: edit) is required.
    pub single_click_edit_mode: bool,

    /// How to align cell contents. Default is left-aligned.
    pub cell_align: egui::Align,

    /// Color to use for the stroke above/below focused row.
    /// If `None`, defaults to a darkened `warn_fg_color`.
    pub focused_row_stroke: Option<egui::Color32>,

    /// See [`ScrollArea::auto_shrink`] for details.
    pub auto_shrink: Vec2b,

    /// See ['ScrollArea::ScrollBarVisibility`] for details.
    pub scroll_bar_visibility: ScrollBarVisibility,
}

/* ------------------------------------------ Rendering ----------------------------------------- */

pub struct Renderer<'a, R, V: RowViewer<R>> {
    table: &'a mut DataTable<R>,
    viewer: &'a mut V,
    state: Option<Box<UiState<R>>>,
    style: Style,
    translator: Arc<dyn Translator>
}

impl<R, V: RowViewer<R>> egui::Widget for Renderer<'_, R, V> {
    fn ui(self, ui: &mut egui::Ui) -> Response {
        self.show(ui)
    }
}

impl<'a, R, V: RowViewer<R>> Renderer<'a, R, V> {
    pub fn new(table: &'a mut DataTable<R>, viewer: &'a mut V) -> Self {
        if table.rows.is_empty() && viewer.allow_row_insertions() {
            table.push(viewer.new_empty_row_for(EmptyRowCreateContext::InsertNewLine));
        }

        Self {
            state: Some(table.ui.take().unwrap_or_default().tap_mut(|state| {
                state.validate_identity(viewer);
            })),
            table,
            viewer,
            style: Default::default(),
            translator: Arc::new(EnglishTranslator::default()),
        }
    }

    pub fn with_style(mut self, style: Style) -> Self {
        self.style = style;
        self
    }

    pub fn with_style_modify(mut self, f: impl FnOnce(&mut Style)) -> Self {
        f(&mut self.style);
        self
    }

    pub fn with_table_row_height(mut self, height: f32) -> Self {
        self.style.table_row_height = Some(height);
        self
    }

    pub fn with_max_undo_history(mut self, max_undo_history: usize) -> Self {
        self.style.max_undo_history = max_undo_history;
        self
    }

    /// Sets a custom translator for the instance.
    /// # Example
    ///
    /// ```
    /// // Define a simple translator
    /// struct EsEsTranslator;
    /// impl Translator for EsEsTranslator {
    ///     fn translate(&self, key: &str) -> String {
    ///         match key {
    ///             "hello" => "Hola".to_string(),
    ///             "world" => "Mundo".to_string(),
    ///             _ => key.to_string(),
    ///         }
    ///     }
    /// }
    ///
    /// let renderer = Renderer::new(&mut table, &mut viewer)
    ///     .with_translator(Arc::new(EsEsTranslator));
    /// ```
    #[cfg(not(doctest))]
    pub fn with_translator(mut self, translator: Arc<dyn Translator>) -> Self {
        self.translator = translator;
        self
    }

    pub fn show(self, ui: &mut egui::Ui) -> Response {
        egui::ScrollArea::horizontal()
            .show(ui, |ui| self.impl_show(ui))
            .inner
    }

    fn impl_show(mut self, ui: &mut egui::Ui) -> Response {
        let ctx = &ui.ctx().clone();
        let ui_id = ui.id();
        let style = ui.style().clone();
        let painter = ui.painter().clone();
        let visual = &style.visuals;
        let viewer = &mut *self.viewer;
        let s = self.state.as_mut().unwrap();
        let mut resp_total = None::<Response>;
        let mut resp_ret = None::<Response>;
        let mut commands = Vec::<Command<R>>::new();
        let ui_layer_id = ui.layer_id();

        // NOTE: unlike RED and YELLOW which can be acquirable through 'error_bg_color' and
        // 'warn_bg_color', there's no 'green' color which can be acquired from inherent theme.
        // Following logic simply gets 'green' color from current background's brightness.
        let green = if visual.window_fill.g() > 128 {
            Color32::DARK_GREEN
        } else {
            Color32::GREEN
        };

        let mut builder = egui_extras::TableBuilder::new(ui).column(Column::auto());

        let iter_vis_cols_with_flag = s
            .vis_cols()
            .iter()
            .enumerate()
            .map(|(index, column)| (column, index + 1 == s.vis_cols().len()));

        for (column, flag) in iter_vis_cols_with_flag {
            builder = builder.column(viewer.column_render_config(column.0, flag));
        }

        if replace(&mut s.cci_want_move_scroll, false) {
            let interact_row = s.interactive_cell().0;
            builder = builder.scroll_to_row(interact_row.0, None);
        }

        builder
            .columns(Column::auto(), s.num_columns() - s.vis_cols().len())
            .drag_to_scroll(false) // Drag is used for selection;
            .striped(true)
            .cell_layout(egui::Layout::default().with_cross_align(self.style.cell_align))
            .max_scroll_height(f32::MAX)
            .auto_shrink(self.style.auto_shrink)
            .scroll_bar_visibility(self.style.scroll_bar_visibility)
            .sense(Sense::click_and_drag().tap_mut(|s| s.set(Sense::FOCUSABLE, true)))
            .header(20., |mut h| {
                h.col(|_ui| {
                    // TODO: Add `Configure Sorting` button
                });

                let has_any_hidden_col = s.vis_cols().len() != s.num_columns();

                for (vis_col, &col) in s.vis_cols().iter().enumerate() {
                    let vis_col = VisColumnPos(vis_col);
                    let mut painter = None;
                    let (col_rect, resp) = h.col(|ui| {
                        egui::Sides::new().show(ui, |ui| {
                            ui.add(Label::new(viewer.column_name(col.0))
                                .selectable(false)
                            );
                        }, |ui|{
                            if let Some(pos) = s.sort().iter().position(|(c, ..)| c == &col) {
                                let is_asc = s.sort()[pos].1 .0 as usize;

                                ui.colored_label(
                                    [green, Color32::RED][is_asc],
                                    RichText::new(format!("{}{}", ["↘", "↗"][is_asc], pos + 1,))
                                        .monospace(),
                                );
                            } else {
                                // calculate the maximum width for the sort indicator
                                let max_sort_indicator_width = (s.num_columns() + 1).to_string().len() + 1;
                                // when the sort indicator is present, create a label the same size as the sort indicator
                                // so that the columns don't resize when sorted.
                                ui.add(Label::new(RichText::new(" ".repeat(max_sort_indicator_width)).monospace()).selectable(false));
                            }
                        });

                        painter = Some(ui.painter().clone());
                    });

                    // Set drag payload for column reordering.
                    resp.dnd_set_drag_payload(vis_col);

                    if resp.dragged() {
                        Tooltip::always_open(ctx.clone(), ui_layer_id, "_EGUI_DATATABLE__COLUMN_MOVE__".into(), PopupAnchor::Pointer)
                            .gap(12.0)
                            .show(|ui|{
                                let colum_name = viewer.column_name(col.0);
                                ui.label(colum_name);
                            });
                    }

                    if resp.hovered() && viewer.is_sortable_column(col.0) {
                        if let Some(p) = &painter {
                            p.rect_filled(
                                col_rect,
                                egui::CornerRadius::ZERO,
                                visual.selection.bg_fill.gamma_multiply(0.2),
                            );
                        }
                    }

                    if viewer.is_sortable_column(col.0) && resp.clicked_by(PointerButton::Primary) {
                        let mut sort = s.sort().to_owned();
                        match sort.iter_mut().find(|(c, ..)| c == &col) {
                            Some((_, asc)) => match asc.0 {
                                true => asc.0 = false,
                                false => sort.retain(|(c, ..)| c != &col),
                            },
                            None => {
                                sort.push((col, IsAscending(true)));
                            }
                        }

                        commands.push(Command::SetColumnSort(sort));
                    }

                    if resp.dnd_hover_payload::<VisColumnPos>().is_some() {
                        if let Some(p) = &painter {
                            p.rect_filled(
                                col_rect,
                                egui::CornerRadius::ZERO,
                                visual.selection.bg_fill.gamma_multiply(0.5),
                            );
                        }
                    }

                    if let Some(payload) = resp.dnd_release_payload::<VisColumnPos>() {
                        commands.push(Command::CcReorderColumn {
                            from: *payload,
                            to: vis_col
                                .0
                                .pipe(|v| v + (payload.0 < v) as usize)
                                .pipe(VisColumnPos),
                        })
                    }

                    resp.context_menu(|ui| {
                        if ui.button(self.translator.translate("context-menu-hide")).clicked() {
                            commands.push(Command::CcHideColumn(col));
                        }

                        if !s.sort().is_empty() && ui.button(self.translator.translate("context-menu-clear-sort")).clicked() {
                            commands.push(Command::SetColumnSort(Vec::new()));
                        }

                        if has_any_hidden_col {
                            ui.separator();
                            ui.label(self.translator.translate("context-menu-hidden"));

                            for col in (0..s.num_columns()).map(ColumnIdx) {
                                if !s.vis_cols().contains(&col)
                                    && ui.button(viewer.column_name(col.0)).clicked()
                                {
                                    commands.push(Command::CcShowColumn {
                                        what: col,
                                        at: vis_col,
                                    });
                                }
                            }
                        }
                    });
                }

                // Account for header response to calculate total response.
                resp_total = Some(h.response());
            })
            .tap_mut(|table| {
                table.ui_mut().separator();
            })
            .body(|body: egui_extras::TableBody<'_>| {
                resp_ret = Some(
                    self.impl_show_body(body, painter, commands, ctx, &style, ui_id, resp_total),
                );
            });

        resp_ret.unwrap_or_else(|| ui.label("??"))
    }

    #[allow(clippy::too_many_arguments)]
    fn impl_show_body(
        &mut self,
        body: egui_extras::TableBody<'_>,
        mut _painter: egui::Painter,
        mut commands: Vec<Command<R>>,
        ctx: &egui::Context,
        style: &egui::Style,
        ui_id: egui::Id,
        mut resp_total: Option<Response>,
    ) -> Response {
        let viewer = &mut *self.viewer;
        let s = self.state.as_mut().unwrap();
        let table = &mut *self.table;
        let visual = &style.visuals;
        let visible_cols = s.vis_cols().clone();
        let no_rounding = egui::CornerRadius::ZERO;

        let mut actions = Vec::<UiAction>::new();
        let mut edit_started = false;
        let hotkeys = viewer.hotkeys(&s.ui_action_context());

        // Preemptively consume all hotkeys.
        'detect_hotkey: {
            // Detect hotkey inputs only when the table has focus. While editing, let the
            // editor consume input.
            if !s.cci_has_focus {
                break 'detect_hotkey;
            }

            if !s.is_editing() {
                ctx.input_mut(|i| {
                    i.events.retain(|x| {
                        match x {
                            Event::Copy => actions.push(UiAction::CopySelection),
                            Event::Cut => actions.push(UiAction::CutSelection),

                            // Try to parse clipboard contents and detect if it's compatible
                            // with cells being pasted.
                            Event::Paste(clipboard) => {
                                if !clipboard.is_empty() {
                                    // If system clipboard is not empty, try to update the internal
                                    // clipboard with system clipboard content before applying
                                    // paste operation.
                                    s.try_update_clipboard_from_string(viewer, clipboard);
                                }

                                if i.modifiers.shift {
                                    if viewer.allow_row_insertions() {
                                        actions.push(UiAction::PasteInsert)
                                    }
                                } else {
                                    actions.push(UiAction::PasteInPlace)
                                }
                            }

                            _ => return true,
                        }
                        false
                    })
                });
            }

            for (hotkey, action) in &hotkeys {
                ctx.input_mut(|inp| {
                    if inp.consume_shortcut(hotkey) {
                        actions.push(*action);
                    }
                })
            }
        }

        // Validate persistency state.
        #[cfg(feature = "persistency")]
        if viewer.persist_ui_state() {
            s.validate_persistency(ctx, ui_id, viewer);
        }

        // Validate ui state. Defer this as late as possible; since it may not be
        // called if the table area is out of the visible space.
        s.validate_cc(&mut table.rows, viewer);

        // Checkout `cc_rows` to satisfy borrow checker. We need to access to
        // state mutably within row rendering; therefore, we can't simply borrow
        // `cc_rows` during the whole logic!
        let cc_row_heights = take(&mut s.cc_row_heights);

        let mut row_height_updates = Vec::new();
        let vis_row_digits = s.cc_rows.len().max(1).ilog10();
        let row_id_digits = table.len().max(1).ilog10();

        let body_max_rect = body.max_rect();
        let has_any_sort = !s.sort().is_empty();

        let pointer_interact_pos = ctx.input(|i| i.pointer.latest_pos().unwrap_or_default());
        let pointer_primary_down = ctx.input(|i| i.pointer.button_down(PointerButton::Primary));

        s.cci_page_row_count = 0;

        /* ----------------------------- Primary Rendering Function ----------------------------- */
        // - Extracted as a closure to differentiate behavior based on row height
        //   configuration. (heterogeneous or homogeneous row heights)

        let render_fn = |mut row: egui_extras::TableRow| {
            s.cci_page_row_count += 1;

            let vis_row = VisRowPos(row.index());
            let row_id = s.cc_rows[vis_row.0];
            let prev_row_height = cc_row_heights[vis_row.0];

            let mut row_elem_start = Default::default();

            // Check if current row is edition target
            let edit_state = s.row_editing_cell(row_id);
            let mut editing_cell_rect = Rect::NOTHING;
            let interactive_row = s.is_interactive_row(vis_row);

            let check_mouse_dragging_selection = |has_selection: bool, rect: &Rect, resp: &egui::Response| {
                // Geometry-based: while primary is down, update selection for the hovered cell.
                // Start selection on simple hover+down when there is no selection yet.
                let drop_area_rect = rect.with_max_x(resp.rect.right());
                if pointer_primary_down {
                    if !has_selection && resp.hovered() { return true; }
                    if drop_area_rect.contains(pointer_interact_pos) { return true; }
                }
                false
            };

            /* -------------------------------- Header Rendering -------------------------------- */

            // Mark row background filled if being edited.
            row.set_selected(edit_state.is_some());

            // Render row header button
            let (head_rect, head_resp) = row.col(|ui| {
                // Calculate the position where values start.
                row_elem_start = ui.max_rect().right_top();

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.separator();

                    if has_any_sort {
                        ui.monospace(
                            RichText::from(f!(
                                "{:·>width$}",
                                row_id.0,
                                width = row_id_digits as usize
                            ))
                            .strong(),
                        );
                    } else {
                        ui.monospace(
                            RichText::from(f!("{:>width$}", "", width = row_id_digits as usize))
                                .strong(),
                        );
                    }

                    ui.monospace(
                        RichText::from(f!(
                            "{:·>width$}",
                            vis_row.0 + 1,
                            width = vis_row_digits as usize
                        ))
                        .weak(),
                    );
                });
            });

            if check_mouse_dragging_selection(s.has_cci_selection(), &head_rect, &head_resp) {
                s.cci_sel_update_row(vis_row);
            }

            /* -------------------------------- Columns Rendering ------------------------------- */

            // Overridable maximum height
            let mut new_maximum_height = 0.;

            // Render cell contents regardless of the edition state.
            for (vis_col, col) in visible_cols.iter().enumerate() {
                let vis_col = VisColumnPos(vis_col);
                let linear_index = vis_row.linear_index(visible_cols.len(), vis_col);
                let selected = s.is_selected(vis_row, vis_col);
                let cci_selected = s.is_selected_cci(vis_row, vis_col);
                let is_editing = edit_state.is_some();
                let is_interactive_cell = interactive_row.is_some_and(|x| x == vis_col);
                let mut response_consumed = s.is_editing();
                // Opt-in: allow the cell view to be interactive without entering edit mode
                // (e.g., buttons, checkboxes, links). This is queried per cell.
                let interactive_in_view = viewer.is_interactive_in_view(&table.rows[row_id.0], col.0);

                // Collect response from an interaction blocker overlay (added inside the cell)
                // and union it with the column response so the table still receives clicks/drags.
                let mut cell_blocker_resp: Option<egui::Response> = None;
                let (rect, resp_inner) = row.col(|ui| {
                    let ui_max_rect = ui.max_rect();

                    if cci_selected {
                        ui.painter().rect_stroke(
                            ui_max_rect,
                            no_rounding,
                            Stroke {
                                width: 2.,
                                color: self
                                    .style
                                    .fg_drag_selection
                                    .unwrap_or(visual.selection.bg_fill),
                            },
                            StrokeKind::Inside,
                        );
                    }

                    if is_interactive_cell {
                        ui.painter().rect_filled(
                            ui_max_rect.expand(2.),
                            no_rounding,
                            self.style
                                .bg_selected_highlight_cell
                                .unwrap_or(visual.selection.bg_fill),
                        );
                    } else if selected {
                        ui.painter().rect_filled(
                            ui_max_rect.expand(1.),
                            no_rounding,
                            self.style
                                .bg_selected_cell
                                .unwrap_or(visual.selection.bg_fill.gamma_multiply(0.5)),
                        );
                    }

                    // Actual widget rendering happens within this line.

                    // ui.set_enabled(false);
                    ui.style_mut()
                        .visuals
                        .widgets
                        .noninteractive
                        .fg_stroke
                        .color = if is_interactive_cell {
                        self.style
                            .fg_selected_highlight_cell
                            .unwrap_or(visual.strong_text_color())
                    } else {
                        visual.strong_text_color()
                    };

                    // Show the cell view without dimming visuals (do NOT disable the UI).
                    // In view mode, always place an invisible blocker on top so the table
                    // handles click+drag selection consistently (Excel-like). For interactive
                    // cells, we'll switch into edit mode on hover to enable interaction.
                    if !(is_editing && is_interactive_cell) {
                        viewer.show_cell_view(ui, &table.rows[row_id.0], col.0);

                        let mut sense = Sense::click_and_drag();
                        sense.set(Sense::FOCUSABLE, false);
                        let block_id = ui.id().with(("egui_data_table_cell_block", row_id.0, col.0));
                        let r = ui.interact(ui_max_rect, block_id, sense);
                        cell_blocker_resp = Some(r);
                    }


                    #[cfg(any())]
                    if selected {
                        ui.painter().rect_stroke(
                            ui_max_rect,
                            no_rounding,
                            Stroke {
                                width: 1.,
                                color: visual.weak_text_color(),
                            },
                        );
                    }

                    if interactive_row.is_some() && !is_editing {
                        let st = Stroke {
                            width: 1.,
                            color: self
                                .style
                                .focused_row_stroke
                                .unwrap_or(visual.warn_fg_color.gamma_multiply(0.5)),
                        };

                        let xr = ui_max_rect.x_range();
                        let yr = ui_max_rect.y_range();
                        ui.painter().hline(xr, yr.min, st);
                        ui.painter().hline(xr, yr.max, st);
                    }

                    if edit_state.is_some_and(|(_, vis)| vis == vis_col) {
                        editing_cell_rect = ui_max_rect;
                    }
                });

                let resp = if let Some(br) = cell_blocker_resp {
                    resp_inner.union(br)
                } else {
                    resp_inner
                };

                // Ensure overall table response observes per-cell interactions so focus and
                // drag state are updated correctly (required for drag-to-select to work).
                if let Some(rt) = &mut resp_total {
                    *rt = rt.union(resp.clone());
                } else {
                    resp_total = Some(resp.clone());
                }

                new_maximum_height = rect.height().max(new_maximum_height);

                // -- Hover & Mouse Actions --
                // Keep interactive row highlight in sync with pointer hover when not editing.
                if !s.is_editing() && rect.contains(pointer_interact_pos) {
                    s.set_interactive_cell(vis_row, vis_col);
                }

                // Hover-to-edit: if this cell is interactive-in-view, editable, not already
                // editing, hovered, and we are NOT dragging selection, switch to edit mode.
                let editable = viewer.is_editable_cell(vis_col.0, vis_row.0, &table.rows[row_id.0]);
                if editable
                    && interactive_in_view
                    && !s.is_editing()
                    && resp.hovered()
                    && !pointer_primary_down
                {
                    commands.push(Command::CcEditStart(
                        row_id,
                        vis_col,
                        viewer.clone_row(&table.rows[row_id.0]).into(),
                    ));
                    edit_started = true;
                }

                // Drag-select and click-select using the existing helper, now that our blocker
                // consistently captures interactions in view mode.
                if check_mouse_dragging_selection(s.has_cci_selection(), &rect, &resp) {
                    response_consumed = true;
                    s.cci_sel_update(linear_index);
                }

                if editable
                    && (resp.clicked_by(PointerButton::Primary)
                        && (self.style.single_click_edit_mode || is_interactive_cell))
                    && !interactive_in_view // for view-interactive cells, hover will enter edit
                {
                    response_consumed = true;
                    commands.push(Command::CcEditStart(
                        row_id,
                        vis_col,
                        viewer.clone_row(&table.rows[row_id.0]).into(),
                    ));
                    edit_started = true;
                }

                /* --------------------------- Context Menu Rendering --------------------------- */

                (resp.clone() | head_resp.clone()).context_menu(|ui| {
                    response_consumed = true;
                    ui.set_min_size(egui::vec2(250., 10.));

                    if !selected {
                        commands.push(Command::CcSetSelection(vec![VisSelection(
                            linear_index,
                            linear_index,
                        )]));
                    } else if !is_interactive_cell {
                        s.set_interactive_cell(vis_row, vis_col);
                    }

                    let sel_multi_row = s.cursor_as_selection().is_some_and(|sel| {
                        let mut min = usize::MAX;
                        let mut max = usize::MIN;

                        for sel in sel {
                            min = min.min(sel.0 .0);
                            max = max.max(sel.1 .0);
                        }

                        let (r_min, _) = VisLinearIdx(min).row_col(s.vis_cols().len());
                        let (r_max, _) = VisLinearIdx(max).row_col(s.vis_cols().len());

                        r_min != r_max
                    });

                    let cursor_x = ui.cursor().min.x;
                    let clip = s.has_clipboard_contents();
                    let b_undo = s.has_undo();
                    let b_redo = s.has_redo();
                    let mut n_sep_menu = 0;
                    let mut draw_sep = false;

                    let context_menu_items = [
                        Some((selected, "🖻", "context-menu-selection-copy", UiAction::CopySelection)),
                        Some((selected, "🖻", "context-menu-selection-cut", UiAction::CutSelection)),
                        Some((selected, "🗙", "context-menu-selection-clear", UiAction::DeleteSelection)),
                        Some((
                            sel_multi_row,
                            "🗐",
                            "context-menu-selection-fill",
                            UiAction::SelectionDuplicateValues,
                        )),
                        None,
                        Some((clip, "➿", "context-menu-clipboard-paste", UiAction::PasteInPlace)),
                        Some((
                            clip && viewer.allow_row_insertions(),
                            "🛠",
                            "context-menu-clipboard-insert",
                            UiAction::PasteInsert,
                        )),
                        None,
                        Some((
                            viewer.allow_row_insertions(),
                            "🗐",
                            "context-menu-row-duplicate",
                            UiAction::DuplicateRow,
                        )),
                        Some((
                            viewer.allow_row_deletions(),
                            "🗙",
                            "context-menu-row-delete",
                            UiAction::DeleteRow,
                        )),
                        None,
                        Some((b_undo, "⎗", "context-menu-undo", UiAction::Undo)),
                        Some((b_redo, "⎘", "context-menu-redo", UiAction::Redo)),
                    ];
                    
                    // Render built-in items
                    for opt in context_menu_items {
                        if let Some((icon, key, action)) =
                            opt.filter(|x| x.0).map(|x| (x.1, x.2, x.3))
                        {
                            if draw_sep {
                                draw_sep = false;
                                ui.separator();
                            }

                            let hotkey = hotkeys
                                .iter()
                                .find_map(|(k, a)| (a == &action).then(|| ctx.format_shortcut(k)));

                            ui.horizontal(|ui| {
                                ui.monospace(icon);
                                ui.add_space(cursor_x + 20. - ui.cursor().min.x);

                                let label = self.translator.translate(key);
                                let btn = egui::Button::new(label)
                                    .shortcut_text(hotkey.unwrap_or_else(|| "🗙".into()));
                                let r = ui.centered_and_justified(|ui| ui.add(btn)).inner;

                                if r.clicked() {
                                    actions.push(action);
                                }
                            });

                            n_sep_menu += 1;
                        } else if n_sep_menu > 0 {
                            n_sep_menu = 0;
                            draw_sep = true;
                        }
                    }

                    // Render custom items contributed by the viewer
                    let ui_ctx = s.ui_action_context();
                    let selection_snapshot = {
                        // Build a lightweight snapshot to pass into the callback
                        let mut selected_rows = Vec::new();
                        if let Some(sels) = s.cursor_as_selection() {
                            let mut rows = std::collections::BTreeSet::new();
                            for sel in sels.iter() {
                                let (top, _) = sel.0.row_col(s.vis_cols().len());
                                let (bottom, _) = sel.1.row_col(s.vis_cols().len());
                                for r in top.0..=bottom.0 { rows.insert(r); }
                            }
                            for r in rows { let row_id = s.cc_rows[r].0; selected_rows.push((row_id, &table.rows[row_id])); }
                        }

                        let mut selected_cells = Vec::new();
                        if let Some(sels) = s.cursor_as_selection() {
                            for sel in sels.iter() {
                                let (top, left) = sel.0.row_col(s.vis_cols().len());
                                let (bottom, right) = sel.1.row_col(s.vis_cols().len());
                                for r in top.0..=bottom.0 {
                                    for c in left.0..=right.0 {
                                        let row_id = s.cc_rows[r].0;
                                        let col = s.vis_cols()[c].0;
                                        selected_cells.push((row_id, col));
                                    }
                                }
                            }
                        }

                        let (ic_r, ic_c) = s.interactive_cell();
                        let interactive_cell = if s.cc_rows.is_empty() { None } else { Some((s.cc_rows[ic_r.0].0, s.vis_cols()[ic_c.0].0)) };

                        crate::viewer::SelectionSnapshot {
                            selected_rows,
                            selected_cells,
                            interactive_cell,
                            visible_columns: s.vis_cols().len(),
                        }
                    };
                    // origin_cell is passed during dispatch from state; nothing to do here.

                    let custom_items = viewer.custom_context_menu_items(&ui_ctx, &selection_snapshot);
                    if !custom_items.is_empty() {
                        ui.separator();
                        for item in custom_items {
                            if !item.enabled { continue; }
                            ui.horizontal(|ui| {
                                if let Some(icon) = item.icon { ui.monospace(icon); }
                                let btn = egui::Button::new(item.label);
                                let r = ui.centered_and_justified(|ui| ui.add(btn)).inner;
                                if r.clicked() { actions.push(UiAction::Custom(item.id)); }
                            });
                        }
                    }
                });

                // Forward DnD event if not any event was consumed by the response.

                // FIXME: Upgrading egui 0.29 make interaction rectangle of response object
                // larger(in y axis) than actually visible column cell size. To deal with this,
                // I've used returned content area rectangle instead, expanding its width to
                // response size.

                let drop_area_rect = rect.with_max_x(resp.rect.max.x);
                let contains_pointer = ctx
                    .pointer_hover_pos()
                    .is_some_and(|pos| drop_area_rect.contains(pos));

                if !response_consumed && contains_pointer {
                    if let Some(new_value) =
                        viewer.on_cell_view_response(&table.rows[row_id.0], col.0, &resp)
                    {
                        let mut values = vec![(row_id, *col, RowSlabIndex(0))];

                        values.retain(|(row, col, _slab_id)| {
                            viewer.is_editable_cell(col.0, row.0, &table.rows[row.0])
                        });

                        commands.push(Command::SetCells {
                            slab: vec![*new_value].into_boxed_slice(),
                            values: values.into_boxed_slice(),
                        });
                    }
                }
            }

            /* -------------------------------- Editor Rendering -------------------------------- */
            if let Some((should_focus, vis_column)) = edit_state {
                let column = s.vis_cols()[vis_column.0];

                egui::Window::new("")
                    .id(ui_id.with(row_id).with(column))
                    .constrain_to(body_max_rect)
                    .fixed_pos(editing_cell_rect.min)
                    .auto_sized()
                    .min_size(editing_cell_rect.size())
                    .max_width(editing_cell_rect.width())
                    .title_bar(false)
                    .frame(egui::Frame::NONE.corner_radius(egui::CornerRadius::same(3)))
                    .show(ctx, |ui| {
                        ui.with_layout(Layout::top_down_justified(Align::LEFT), |ui| {
                            if let Some(resp) =
                                viewer.show_cell_editor(ui, s.unwrap_editing_row_data(), column.0)
                            {
                                if should_focus {
                                    resp.request_focus()
                                }

                                new_maximum_height = resp.rect.height().max(new_maximum_height);
                            } else {
                                commands.push(Command::CcCommitEdit);
                            }
                        });
                    });
            }

            // Accumulate response
            if let Some(resp) = &mut resp_total {
                *resp = resp.union(row.response());
            } else {
                resp_total = Some(row.response());
            }

            // Update row height cache if necessary.
            if self.style.table_row_height.is_none() && prev_row_height != new_maximum_height {
                row_height_updates.push((vis_row, new_maximum_height));
            }
        }; // ~ render_fn

        // Actual rendering
        if let Some(height) = self.style.table_row_height {
            body.rows(height, cc_row_heights.len(), render_fn);
        } else {
            body.heterogeneous_rows(cc_row_heights.iter().cloned(), render_fn);
        }

        /* ----------------------------------- Event Handling ----------------------------------- */

        if ctx.input(|i| i.pointer.button_released(PointerButton::Primary)) {
            let mods = ctx.input(|i| i.modifiers);
            if let Some(sel) = s.cci_take_selection(mods).filter(|_| !edit_started) {
                commands.push(Command::CcSetSelection(sel));
            }
        }

        // Control overall focus status.
        if let Some(resp) = resp_total.clone() {

            let clicked_elsewhere = resp.clicked_elsewhere();
            // IMPORTANT: cannot use `resp.contains_pointer()` here
            let response_rect_contains_pointer = resp.rect.contains(pointer_interact_pos);

            if resp.clicked() | resp.dragged() {
                s.cci_has_focus = true;
            } else if clicked_elsewhere && !response_rect_contains_pointer {
                s.cci_has_focus = false;
                if s.is_editing() {
                    commands.push(Command::CcCommitEdit)
                }
            }
        }

        // Check in borrowed `cc_rows` back to state.
        s.cc_row_heights = cc_row_heights.tap_mut(|values| {
            if !row_height_updates.is_empty() {
                ctx.request_repaint();
            }

            for (row_index, row_height) in row_height_updates {
                values[row_index.0] = row_height;
            }
        });

        // Handle queued actions
        commands.extend(
            actions
                .into_iter()
                .flat_map(|action| s.try_apply_ui_action(table, viewer, action)),
        );

        // Handle queued commands
        for cmd in commands {
            match cmd {
                Command::CcUpdateSystemClipboard(new_content) => {
                    ctx.copy_text(new_content);
                }
                cmd => {
                    if matches!(cmd, Command::CcCommitEdit) {
                        // If any commit action is detected, release any remaining focus.
                        ctx.memory_mut(|x| {
                            if let Some(fc) = x.focused() {
                                x.surrender_focus(fc)
                            }
                        });
                    }

                    s.push_new_command(
                        table,
                        viewer,
                        cmd,
                        if self.style.max_undo_history == 0 {
                            100
                        } else {
                            self.style.max_undo_history
                        },
                    );
                }
            }
        }

        // Total response
        resp_total.unwrap()
    }
}

impl<R, V: RowViewer<R>> Drop for Renderer<'_, R, V> {
    fn drop(&mut self) {
        self.table.ui = self.state.take();
    }
}

/* ------------------------------------------- Translations ------------------------------------- */

pub trait Translator {

    /// Translates a given key into its corresponding string representation.
    ///
    /// If the translation key is unknown, return the key as a [`String`]
    fn translate(&self, key: &str) -> String;
}

#[derive(Default)]
pub struct EnglishTranslator {}

impl Translator for EnglishTranslator {
    fn translate(&self, key: &str) -> String {
        match key {
            // cell context menu
            "context-menu-selection-copy" => "Selection: Copy",
            "context-menu-selection-cut" => "Selection: Cut",
            "context-menu-selection-clear" => "Selection: Clear",
            "context-menu-selection-fill" => "Selection: Fill",
            "context-menu-clipboard-paste" => "Clipboard: Paste",
            "context-menu-clipboard-insert" => "Clipboard: Insert",
            "context-menu-row-duplicate" => "Row: Duplicate",
            "context-menu-row-delete" => "Row: Delete",
            "context-menu-undo" => "Undo",
            "context-menu-redo" => "Redo",

            // column header context menu
            "context-menu-hide" => "Hide",
            "context-menu-hidden" => "Hidden",
            "context-menu-clear-sort" => "Clear sort",
            _ => key,
        }.to_string()
    }
}
