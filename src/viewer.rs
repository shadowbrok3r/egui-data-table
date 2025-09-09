use std::borrow::Cow;

use egui::{Key, KeyboardShortcut, Modifiers};
pub use egui_extras::Column as TableColumnConfig;
use tap::prelude::Pipe;

/// A snapshot of the current selection and context for use by custom callbacks.
#[derive(Debug, Clone)]
pub struct SelectionSnapshot<'a, R> {
    /// Selected row ids and their values.
    pub selected_rows: Vec<(usize, &'a R)>,
    /// Selected cells as pairs of (row_id, column_index).
    pub selected_cells: Vec<(usize, usize)>,
    /// The interactive cell if any, as (row_id, column_index).
    pub interactive_cell: Option<(usize, usize)>,
    /// Number of currently visible columns.
    pub visible_columns: usize,
}

/// A menu item contributed by the RowViewer for the context menu.
#[derive(Debug, Clone)]
pub struct CustomMenuItem {
    /// A stable identifier used to dispatch the action.
    pub id: &'static str,
    /// Display label for the menu item.
    pub label: String,
    /// Optional icon or emoji prefix.
    pub icon: Option<&'static str>,
    /// Whether the item is enabled in the current context.
    pub enabled: bool,
}

impl CustomMenuItem {
    pub fn new(id: &'static str, label: impl Into<String>) -> Self {
        Self { id, label: label.into(), icon: None, enabled: true }
    }
    pub fn icon(mut self, icon: &'static str) -> Self { self.icon = Some(icon); self }
    pub fn enabled(mut self, enabled: bool) -> Self { self.enabled = enabled; self }
}

/// A user-issued command returned by custom actions. This will be translated into
/// internal commands and integrated with undo/redo.
#[derive(Debug, Clone)]
pub enum UserCommand<R> {
    /// Set multiple cells using a slab of row templates.
    /// values: (row_id, column_index, slab_index). If `context` is Some, it will be
    /// treated as a UI-originated write (e.g., Paste/Clear).
    SetCells {
        slab: Box<[R]>,
        values: Box<[(usize, usize, usize)]>,
        context: Option<CellWriteContext>,
    },
    /// Replace an entire row value.
    SetRowValue(usize, Box<R>),
    /// Insert rows before the given position (by row id).
    InsertRows(usize, Box<[R]>),
    /// Remove the given rows by row ids.
    RemoveRows(Vec<usize>),
}

/// Context provided to a custom action, including the origin cell (where the menu was opened)
/// and a selection snapshot.
#[derive(Debug, Clone)]
pub struct CustomActionContext<'a, R> {
    pub selection: SelectionSnapshot<'a, R>,
    /// The cell where the action originates (row_id, column), if applicable.
    pub origin_cell: Option<(usize, usize)>,
}

/// A builder-style editor that lets custom actions queue undoable changes in an ergonomic way.
#[derive(Debug)]
pub struct CustomActionEditor<R> {
    cmds: Vec<UserCommand<R>>,
    slab: Vec<R>,
    values: Vec<(usize, usize, usize)>,
}

impl<R> CustomActionEditor<R> {
    pub fn new() -> Self { Self { cmds: Vec::new(), slab: Vec::new(), values: Vec::new() } }

    /// Queue a single cell write. Provide a row object whose relevant column is set.
    /// The write will be applied to only the given column via RowViewer::set_cell_value.
    pub fn set_cell(&mut self, row_id: usize, column: usize, src_row: R) -> &mut Self {
        self.slab.push(src_row);
        let idx = self.slab.len() - 1;
        self.values.push((row_id, column, idx));
        self
    }

    /// Queue a full-row replacement.
    pub fn set_row(&mut self, row_id: usize, row: R) -> &mut Self {
        self.cmds.push(UserCommand::SetRowValue(row_id, Box::new(row)));
        self
    }

    /// Queue insertion of rows at a position.
    pub fn insert_rows<I: IntoIterator<Item = R>>(&mut self, pos: usize, rows: I) -> &mut Self {
        self.cmds.push(UserCommand::InsertRows(pos, rows.into_iter().collect::<Vec<_>>().into_boxed_slice()));
        self
    }

    /// Queue removal of the given row ids.
    pub fn remove_rows<I: IntoIterator<Item = usize>>(&mut self, rows: I) -> &mut Self {
        self.cmds.push(UserCommand::RemoveRows(rows.into_iter().collect()));
        self
    }

    /// Finalize any pending cell writes into a single SetCells command. Call with Some(ctx)
    /// to mark writes as UI-originated (e.g., Paste/Clear), or None for programmatic.
    pub fn commit_cells(&mut self, context: Option<CellWriteContext>) -> &mut Self {
        if !self.values.is_empty() {
            let slab = std::mem::take(&mut self.slab).into_boxed_slice();
            let values = std::mem::take(&mut self.values).into_boxed_slice();
            self.cmds.push(UserCommand::SetCells { slab, values, context });
        }
        self
    }

    /// Consume the editor and return all queued commands (auto-commits pending cells without context).
    pub fn into_commands(mut self) -> Vec<UserCommand<R>> {
        self.commit_cells(None);
        self.cmds
    }
}

#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeErrorBehavior {
    /// Skip the cell and continue decoding.
    SkipCell,

    /// Skip the whole row
    SkipRow,

    /// Stop decoding and return error.
    #[default]
    Abort,
}

/// A trait for encoding/decoding row data. Any valid UTF-8 string can be used for encoding,
/// however, as csv is used for clipboard operations, it is recommended to serialize data in simple
/// string format as possible.
pub trait RowCodec<R> {
    type DeserializeError;

    /// Creates a new empty row for decoding
    fn create_empty_decoded_row(&mut self) -> R;

    /// Tries encode column data of given row into a string. As the cell for CSV row is already
    /// occupied, if any error or unsupported data is found for that column, just empty out the
    /// destination string buffer.
    fn encode_column(&mut self, src_row: &R, column: usize, dst: &mut String);

    /// Tries decode column data from a string into a row.
    fn decode_column(
        &mut self,
        src_data: &str,
        column: usize,
        dst_row: &mut R,
    ) -> Result<(), DecodeErrorBehavior>;
}

/// A placeholder codec for row viewers that not require serialization.
impl<R> RowCodec<R> for () {
    type DeserializeError = ();

    fn create_empty_decoded_row(&mut self) -> R {
        unimplemented!()
    }

    fn encode_column(&mut self, src_row: &R, column: usize, dst: &mut String) {
        let _ = (src_row, column, dst);
        unimplemented!()
    }

    fn decode_column(
        &mut self,
        src_data: &str,
        column: usize,
        dst_row: &mut R,
    ) -> Result<(), DecodeErrorBehavior> {
        let _ = (src_data, column, dst_row);
        unimplemented!()
    }
}

/// The primary trait for the spreadsheet viewer.
// TODO: When lifetime for `'static` is stabilized; remove the `static` bound.
pub trait RowViewer<R>: 'static {
    /// Number of columns. Changing this will completely invalidate the table rendering status,
    /// including undo histories. Therefore, frequently changing this value is discouraged.
    fn num_columns(&mut self) -> usize;

    /// Name of the column. This can be dynamically changed.
    fn column_name(&mut self, column: usize) -> Cow<'static, str> {
        Cow::Borrowed(
            &" 0 1 2 3 4 5 6 7 8 91011121314151617181920212223242526272829303132"
                [(column % 10 * 2).pipe(|x| x..x + 2)],
        )
    }

    /// Tries to create a codec for the row (de)serialization. If this returns `Some`, it'll use
    /// the system clipboard for copy/paste operations.
    ///
    /// `is_encoding` parameter is provided to determine if we're creating the codec as encoding
    /// mode or decoding mode.
    ///
    /// It is just okay to choose not to implement both encoding and decoding; returning `None`
    /// conditionally based on `is_encoding` parameter is also valid. It is guaranteed that created
    /// codec will be used only for the same mode during its lifetime.
    fn try_create_codec(&mut self, is_encoding: bool) -> Option<impl RowCodec<R>> {
        let _ = is_encoding;
        None::<()>
    }

    /// Returns the rendering configuration for the column.
    fn column_render_config(
        &mut self,
        column: usize,
        is_last_visible_column: bool,
    ) -> TableColumnConfig {
        let _ = column;
        if is_last_visible_column {
            TableColumnConfig::remainder().at_least(24.0)
        } else {
            TableColumnConfig::auto().resizable(true)
        }
    }

    /// Returns if given column is 'sortable'
    fn is_sortable_column(&mut self, column: usize) -> bool {
        let _ = column;
        false
    }

    /// Returns if a given cell is 'editable'.
    /// 
    /// i.e.
    /// * true to allow editing of a cell
    /// * false to disable editing of a cell
    fn is_editable_cell(&mut self, column: usize, row: usize, row_value: &R) -> bool {
        let _ = column;
        let _ = row;
        let _ = row_value;
        true
    }

    /// Returns if row insertions are allowed.
    fn allow_row_insertions(&mut self) -> bool {
        true
    }

    /// Returns if row deletions are allowed.
    fn allow_row_deletions(&mut self) -> bool {
        true
    }

    /// Compare two column contents for sort.
    fn compare_cell(&self, row_a: &R, row_b: &R, column: usize) -> std::cmp::Ordering {
        let _ = (row_a, row_b, column);
        std::cmp::Ordering::Equal
    }

    /// Get hash value of a filter. This is used to determine if the filter has changed.
    fn row_filter_hash(&mut self) -> &impl std::hash::Hash {
        &()
    }

    /// Filter single row. If this returns false, the row will be hidden.
    fn filter_row(&mut self, row: &R) -> bool {
        let _ = row;
        true
    }

    /// Display values of the cell. Any input will be consumed before table renderer;
    /// therefore any widget rendered inside here is read-only.
    ///
    /// To deal with input, use `cell_edit` method. If you need to deal with drag/drop,
    /// see [`RowViewer::on_cell_view_response`] which delivers resulting response of
    /// containing cell.
    fn show_cell_view(&mut self, ui: &mut egui::Ui, row: &R, column: usize);

    /// Whether the cell's view (shown via `show_cell_view`) contains interactive widgets
    /// that should receive pointer input directly without requiring the cell to enter
    /// edit mode.
    ///
    /// Default is `false` which preserves the legacy behavior where the table consumes
    /// clicks for selection and you must enter edit mode first. Override and return `true`
    /// for cells that render buttons, toggles, links, etc., and you want them to be
    /// clickable directly in view-mode.
    ///
    /// Note: when this returns `true`, the table will still allow drag-selection starting
    /// from such a cell, but only when an actual drag is detected (i.e. moving the pointer).
    /// A simple click will be passed to the inner widget instead of starting a selection
    /// or entering edit mode.
    fn is_interactive_in_view(&mut self, _row: &R, _column: usize) -> bool { false }

    /// Use this to check if given cell is going to take any dropped payload / use as drag
    /// source.
    fn on_cell_view_response(
        &mut self,
        row: &R,
        column: usize,
        resp: &egui::Response,
    ) -> Option<Box<R>> {
        let _ = (row, column, resp);
        None
    }

    /// Edit values of the cell.
    fn show_cell_editor(
        &mut self,
        ui: &mut egui::Ui,
        row: &mut R,
        column: usize,
    ) -> Option<egui::Response>;

    /// Set the value of a column in a row.
    fn set_cell_value(&mut self, src: &R, dst: &mut R, column: usize);

    /// In the write context that happens outside of `show_cell_editor`, this method is
    /// called on every cell value editions.
    fn confirm_cell_write_by_ui(
        &mut self,
        current: &R,
        next: &R,
        column: usize,
        context: CellWriteContext,
    ) -> bool {
        let _ = (current, next, column, context);
        true
    }

    /// Before removing each row, this method is called to confirm the deletion from the
    /// viewer. This won't be called during the undo/redo operation!
    fn confirm_row_deletion_by_ui(&mut self, row: &R) -> bool {
        let _ = row;
        true
    }

    /// Create a new empty row.
    fn new_empty_row(&mut self) -> R;

    /// Create a new empty row under the given context.
    fn new_empty_row_for(&mut self, context: EmptyRowCreateContext) -> R {
        let _ = context;
        self.new_empty_row()
    }

    /// Create duplication of existing row.
    ///
    /// You may want to override this method for more efficient duplication.
    fn clone_row(&mut self, row: &R) -> R {
        let mut dst = self.new_empty_row();
        for i in 0..self.num_columns() {
            self.set_cell_value(row, &mut dst, i);
        }
        dst
    }

    /// Create duplication of existing row for insertion.
    fn clone_row_for_insertion(&mut self, row: &R) -> R {
        self.clone_row(row)
    }

    /// Create duplication of existing row for clipboard. Useful when you need to specify
    /// different behavior for clipboard duplication. (e.g. unset transient flag)
    fn clone_row_as_copied_base(&mut self, row: &R) -> R {
        self.clone_row(row)
    }

    /// Called when a cell is selected/highlighted.
    fn on_highlight_cell(&mut self, row: &R, column: usize) {
        let _ = (row, column);
    }

    /// Called when a row selected/highlighted status changes.
    fn on_highlight_change(&mut self, highlighted: &[&R], unhighlighted: &[&R]) {
        let (_, _) = (highlighted, unhighlighted);
    }
    
    /// Called when a row is updated, including when undoing/redoing
    fn on_row_updated(&mut self, row_index: usize, new_row: &R, old_row: &R) {
        let (_, _, _) = (row_index, new_row, old_row);
    }

    /// Called when a row has been inserted, including when undoing/redoing
    fn on_row_inserted(&mut self, row_index: usize, row: &R) {
        let (_, _) = (row_index, row);
    }

    /// Called when a row has been removed, including when undoing/redoing
    fn on_row_removed(&mut self, row_index: usize, row: &R) {
        let (_, _) = (row_index, row);
    }

    /// Return hotkeys for the current context.
    fn hotkeys(&mut self, context: &UiActionContext) -> Vec<(egui::KeyboardShortcut, UiAction)> {
        self::default_hotkeys(context)
    }

    /// If you want to keep UI state on storage(i.e. persist over sessions), return true from this
    /// function.
    #[cfg(feature = "persistency")]
    fn persist_ui_state(&self) -> bool {
        false
    }

    /// Provide custom context menu items based on the current UI state and selection.
    /// Return an empty Vec to contribute nothing.
    fn custom_context_menu_items(
        &mut self,
        _context: &UiActionContext,
        _selection: &SelectionSnapshot<'_, R>,
    ) -> Vec<CustomMenuItem> {
        Vec::new()
    }

    /// Handle a custom action invoked from the context menu or other triggers.
    /// Return high-level user commands which will be translated into internal commands
    /// and integrated with undo/redo.
    fn on_custom_action(
        &mut self,
        _action_id: &'static str,
        _selection: &SelectionSnapshot<'_, R>,
    ) -> Vec<UserCommand<R>> {
        Vec::new()
    }

    /// Ergonomic variant: handle a custom action with a context and a builder-style editor.
    /// Default bridges to `on_custom_action` for backward compatibility.
    fn on_custom_action_ex(
        &mut self,
        action_id: &'static str,
        ctx: &CustomActionContext<'_, R>,
        editor: &mut CustomActionEditor<R>,
    ) {
        let cmds = self.on_custom_action(action_id, &ctx.selection);
        for c in cmds { editor.cmds.push(c); }
    }
}

/* ------------------------------------------- Context ------------------------------------------ */

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum CellWriteContext {
    /// Value is being pasted/duplicated from different row.
    Paste,

    /// Value is being cleared by cut/delete operation.
    Clear,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum EmptyRowCreateContext {
    /// Row is created to be used as simple default template.
    Default,

    /// Row is created to be used as explicit `empty` value when deletion
    DeletionDefault,

    /// Row is created to be inserted as a new row.
    InsertNewLine,
}

/* ------------------------------------------- Hotkeys ------------------------------------------ */

/// Base context for determining current input state.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct UiActionContext {
    pub cursor: UiCursorState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiCursorState {
    Idle,
    Editing,
    SelectOne,
    SelectMany,
}

impl UiCursorState {
    pub fn is_idle(&self) -> bool {
        matches!(self, Self::Idle)
    }

    pub fn is_editing(&self) -> bool {
        matches!(self, Self::Editing)
    }

    pub fn is_selecting(&self) -> bool {
        matches!(self, Self::SelectOne | Self::SelectMany)
    }
}

/* ----------------------------------------- Ui Actions ----------------------------------------- */

/// Represents a user interaction, calculated from the UI input state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum UiAction {
    SelectionStartEditing,

    CancelEdition,
    CommitEdition,

    CommitEditionAndMove(MoveDirection),

    Undo,
    Redo,

    MoveSelection(MoveDirection),
    CopySelection,
    CutSelection,

    PasteInPlace,
    PasteInsert,

    DuplicateRow,
    DeleteSelection,
    DeleteRow,

    NavPageDown,
    NavPageUp,
    NavTop,
    NavBottom,

    SelectionDuplicateValues,
    SelectAll,

    /// Custom action contributed by the RowViewer. Carries a stable action id.
    Custom(&'static str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MoveDirection {
    Up,
    Down,
    Left,
    Right,
}

pub fn default_hotkeys(context: &UiActionContext) -> Vec<(KeyboardShortcut, UiAction)> {
    let c = context.cursor;

    fn shortcut(actions: &[(Modifiers, Key, UiAction)]) -> Vec<(egui::KeyboardShortcut, UiAction)> {
        actions
            .iter()
            .map(|(m, k, a)| (egui::KeyboardShortcut::new(*m, *k), *a))
            .collect()
    }

    let none = Modifiers::NONE;
    let ctrl = Modifiers::CTRL;
    let alt = Modifiers::ALT;
    let shift = Modifiers::SHIFT;

    use UiAction::CommitEditionAndMove;
    type MD = MoveDirection;

    if c.is_editing() {
        shortcut(&[
            (none, Key::Escape, UiAction::CommitEdition),
            (ctrl, Key::Escape, UiAction::CancelEdition),
            (shift, Key::Enter, CommitEditionAndMove(MD::Up)),
            (ctrl, Key::Enter, CommitEditionAndMove(MD::Down)),
            (shift, Key::Tab, CommitEditionAndMove(MD::Left)),
            (none, Key::Tab, CommitEditionAndMove(MD::Right)),
        ])
    } else {
        shortcut(&[
            (ctrl, Key::X, UiAction::CutSelection),
            (ctrl, Key::C, UiAction::CopySelection),
            (ctrl | shift, Key::V, UiAction::PasteInsert),
            (ctrl, Key::V, UiAction::PasteInPlace),
            (ctrl, Key::Y, UiAction::Redo),
            (ctrl, Key::Z, UiAction::Undo),
            (none, Key::Enter, UiAction::SelectionStartEditing),
            (none, Key::ArrowUp, UiAction::MoveSelection(MD::Up)),
            (none, Key::ArrowDown, UiAction::MoveSelection(MD::Down)),
            (none, Key::ArrowLeft, UiAction::MoveSelection(MD::Left)),
            (none, Key::ArrowRight, UiAction::MoveSelection(MD::Right)),
            (shift, Key::V, UiAction::PasteInsert),
            (alt, Key::V, UiAction::PasteInsert),
            (ctrl | shift, Key::D, UiAction::DuplicateRow),
            (ctrl, Key::D, UiAction::SelectionDuplicateValues),
            (ctrl, Key::A, UiAction::SelectAll),
            (ctrl, Key::Delete, UiAction::DeleteRow),
            (none, Key::Delete, UiAction::DeleteSelection),
            (none, Key::Backspace, UiAction::DeleteSelection),
            (none, Key::PageUp, UiAction::NavPageUp),
            (none, Key::PageDown, UiAction::NavPageDown),
            (none, Key::Home, UiAction::NavTop),
            (none, Key::End, UiAction::NavBottom),
        ])
    }
}
