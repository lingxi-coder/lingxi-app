//! TUI-owned transport and rendering for the first Mod UI site.
//!
//! This vertical slice draws terminal `AbovePrompt` trees made from `Box` and
//! `Text`. Other elements and callback controls are intentionally not exposed.
//! Its row budget is measured from the live bottom-pane layout, and a session
//! generation carries `$.ui.invalidate('ui.render')` through the draw loop.

use crossterm::event::{KeyCode, KeyEvent};
use lingxi_core::types::utf16_json::{JsonValueView, Utf16JsonProjection};
use serde_json::{json, Value};

pub(crate) const ABOVE_PROMPT_REQUEST_ID: &str = "above-prompt";

/// Revoke actions from the currently displayed tree before starting its
/// replacement, and keep the two async host calls ordered as one task.
pub(crate) async fn clear_then_render<C, CFut, R, RFut, T, E>(clear: C, render: R) -> Result<T, E>
where
    C: FnOnce() -> CFut,
    CFut: std::future::Future<Output = Result<(), E>>,
    R: FnOnce() -> RFut,
    RFut: std::future::Future<Output = Result<T, E>>,
{
    let _ = clear().await;
    render().await
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AbovePromptButtonAction {
    pub(crate) plugin: Utf16JsonProjection,
    pub(crate) key: Utf16JsonProjection,
    pub(crate) key_display: String,
    pub(crate) handle: u64,
    pub(crate) worker_epoch: Utf16JsonProjection,
    pub(crate) render_revision: u64,
    pub(crate) site_revision: u64,
    pub(crate) label: String,
    pub(crate) hotkey: Option<char>,
    pub(crate) auto_focus: bool,
}

impl AbovePromptButtonAction {
    pub(crate) fn press_event(&self) -> Utf16JsonProjection {
        let press = Utf16JsonProjection::object(vec![
            ("handle".to_string(), json_u64(self.handle)),
            ("workerEpoch".to_string(), self.worker_epoch.clone()),
            ("renderRevision".to_string(), json_u64(self.render_revision)),
        ])
        .expect("press fields form an object");
        Utf16JsonProjection::object(vec![
            ("surface".to_string(), "terminal".into()),
            ("component".to_string(), "AbovePrompt".into()),
            ("requestId".to_string(), ABOVE_PROMPT_REQUEST_ID.into()),
            ("plugin".to_string(), self.plugin.clone()),
            ("element".to_string(), self.key.clone()),
            ("press".to_string(), press),
        ])
        .expect("press event fields form an object")
    }
}

pub(crate) fn above_prompt_event(
    columns: u16,
    rows: u16,
    is_fullscreen: bool,
    is_working: bool,
    max_rows: u16,
) -> Utf16JsonProjection {
    let event = json!({
        "surface":"terminal",
        "component":"AbovePrompt",
        "requestId":ABOVE_PROMPT_REQUEST_ID,
        "props":{
            "hasSurvey":false,
            "isWorking":is_working,
            "maxRows":max_rows,
            "bodyColumns":columns.max(1),
            "scroll":{"offset":0,"bodyRows":max_rows.saturating_sub(1)},
            "view":{},
        },
        "viewport":{
            "columns":columns.max(1),
            "rows":rows.max(1),
            "isFullscreen":is_fullscreen,
        }
    });
    Utf16JsonProjection::from_serde_value(&event)
        .expect("generated AbovePrompt render request is valid JSON")
}

/// Turn the supported JSON tree into a small number of terminal rows.
pub(crate) fn tree_lines(tree: &Utf16JsonProjection, max_rows: u16) -> Option<Vec<String>> {
    tree_buttons(tree, 1)?;
    let display_tree = display_value(tree.at_pointer("")?, 0)?;
    let lines = lines_for(&display_tree, 0)?;
    Some(lines.into_iter().take(usize::from(max_rows)).collect())
}

pub(crate) fn tree_buttons(
    tree: &Utf16JsonProjection,
    site_revision: u64,
) -> Option<Vec<AbovePromptButtonAction>> {
    if site_revision == 0 {
        return None;
    }
    let mut buttons = Vec::new();
    collect_buttons(tree, "", site_revision, &mut buttons, 0)?;
    Some(buttons)
}

fn collect_buttons(
    tree: &Utf16JsonProjection,
    pointer: &str,
    site_revision: u64,
    buttons: &mut Vec<AbovePromptButtonAction>,
    depth: usize,
) -> Option<()> {
    if depth > 32 {
        return None;
    }
    let field = |name: &str| format!("{pointer}/{name}");
    let kind = tree.string_at(&field("type"))?;
    match kind {
        "Button" => {
            let props = field("props");
            let press = field("press");
            if !object_has_only_fields(
                tree,
                &props,
                &[
                    "key",
                    "label",
                    "hotkey",
                    "action",
                    "plain",
                    "dimColor",
                    "variant",
                    "role",
                    "autoFocus",
                ],
            ) || !object_has_only_fields(
                tree,
                &press,
                &["plugin", "handle", "workerEpoch", "renderRevision"],
            ) || valid_terminal_button_children(tree, &field("children")).is_none()
            {
                return None;
            }
            let key_path = format!("{props}/key");
            let label = tree.string_at(&format!("{props}/label"))?.to_owned();
            let key = tree.subprojection(&key_path).ok()?;
            let key_display = key.string_at("")?.to_owned();
            let hotkey_path = format!("{props}/hotkey");
            let hotkey_source = tree.subprojection(&hotkey_path).ok();
            let hotkey = match hotkey_source {
                Some(value) => {
                    let units = value.string_units("")?;
                    let [unit] = units.as_slice() else {
                        return None;
                    };
                    let byte = u8::try_from(*unit).ok()?;
                    if !byte.is_ascii_lowercase() && !byte.is_ascii_digit() {
                        return None;
                    }
                    Some(char::from(byte))
                }
                None => None,
            };
            let plugin_path = format!("{press}/plugin");
            let worker_epoch_path = format!("{press}/workerEpoch");
            let plugin = tree.subprojection(&plugin_path).ok()?;
            let worker_epoch = tree.subprojection(&worker_epoch_path).ok()?;
            let handle = number_as_u64(tree.number_at(&format!("{press}/handle"))?)
                .filter(|value| *value > 0)?;
            let render_revision =
                number_as_u64(tree.number_at(&format!("{press}/renderRevision"))?)
                    .filter(|value| *value > 0)?;
            if plugin.string_units("")?.is_empty()
                || worker_epoch.string_units("")?.is_empty()
                || key.string_units("")?.is_empty()
                || label.is_empty()
            {
                return None;
            }
            buttons.push(AbovePromptButtonAction {
                plugin,
                key,
                key_display,
                handle,
                worker_epoch,
                render_revision,
                site_revision,
                label,
                hotkey,
                auto_focus: tree
                    .at_pointer(&format!("{props}/autoFocus"))
                    .and_then(JsonValueView::as_bool)
                    == Some(true),
            });
        }
        "Box" => {
            let children = field("children");
            for index in 0..tree.array_len(&children)? {
                let child = format!("{children}/{index}");
                if tree.string_at(&child).is_none() {
                    collect_buttons(tree, &child, site_revision, buttons, depth + 1)?;
                }
            }
        }
        "Text" => validate_terminal_text(tree, pointer)?,
        _ => return None,
    }
    Some(())
}

fn valid_terminal_button_children(tree: &Utf16JsonProjection, pointer: &str) -> Option<()> {
    let Some(_) = tree.at_pointer(pointer) else {
        return Some(());
    };
    for index in 0..tree.array_len(pointer)? {
        let child = format!("{pointer}/{index}");
        if let Some(units) = tree.string_units(&child) {
            if units.len() > 10_000 {
                return None;
            }
            continue;
        }
        validate_terminal_text(tree, &child)?;
    }
    Some(())
}

fn validate_terminal_text(tree: &Utf16JsonProjection, pointer: &str) -> Option<()> {
    if tree.string_at(&format!("{pointer}/type"))? != "Text"
        || !object_has_only_fields(tree, pointer, &["type", "props", "children"])
    {
        return None;
    }
    let props = format!("{pointer}/props");
    if tree.at_pointer(&props).is_some()
        && (!object_has_only_fields(tree, &props, &["bold"])
            || tree
                .at_pointer(&format!("{props}/bold"))
                .is_some_and(|value| value.as_bool().is_none()))
    {
        return None;
    }
    let children = format!("{pointer}/children");
    for index in 0..tree.array_len(&children)? {
        if tree.string_units(&format!("{children}/{index}"))?.len() > 10_000 {
            return None;
        }
    }
    Some(())
}

fn object_has_only_fields(tree: &Utf16JsonProjection, pointer: &str, allowed: &[&str]) -> bool {
    tree.at_pointer(pointer)
        .and_then(JsonValueView::object_entries)
        .is_some_and(|entries| {
            entries
                .iter()
                .all(|entry| allowed.contains(&entry.display_key))
        })
}

fn json_u64(value: u64) -> Utf16JsonProjection {
    Utf16JsonProjection::parse(&value.to_string()).expect("u64 is valid JSON number")
}

fn number_as_u64(value: f64) -> Option<u64> {
    (value.is_finite() && value >= 0.0 && value.fract() == 0.0 && value <= u64::MAX as f64)
        .then_some(value as u64)
}

/// Convert only at the terminal rendering boundary. Exact source stays in the
/// event and button-action path; this display projection may replace lone
/// UTF-16 units because Ratatui renders Unicode scalar text.
fn display_value(view: JsonValueView<'_>, depth: usize) -> Option<Value> {
    if depth > 128 {
        return None;
    }
    if let Some(text) = view.as_str() {
        return Some(Value::String(text.to_owned()));
    }
    if let Some(value) = view.as_bool() {
        return Some(Value::Bool(value));
    }
    if view.is_null() {
        return Some(Value::Null);
    }
    if let Some(value) = view.as_number() {
        return Some(serde_json::Number::from_f64(value).map_or(Value::Null, Value::Number));
    }
    if let Some(length) = view.array_len() {
        return (0..length)
            .map(|index| display_value(view.get_index(index)?, depth + 1))
            .collect::<Option<Vec<_>>>()
            .map(Value::Array);
    }
    if let Some(entries) = view.object_entries() {
        let mut object = serde_json::Map::with_capacity(entries.len());
        for entry in entries {
            object.insert(
                entry.display_key.to_owned(),
                display_value(entry.value, depth + 1)?,
            );
        }
        return Some(Value::Object(object));
    }
    None
}

pub(crate) fn button_for_key(
    buttons: &[AbovePromptButtonAction],
    key: KeyEvent,
    composer_empty: bool,
    has_active_view: bool,
) -> Option<AbovePromptButtonAction> {
    if !composer_empty || has_active_view || key.kind != crossterm::event::KeyEventKind::Press {
        return None;
    }
    match key.code {
        KeyCode::Enter if key.modifiers.is_empty() => buttons
            .iter()
            .find(|button| button.auto_focus)
            .or_else(|| buttons.first())
            .cloned(),
        KeyCode::Char(ch) if key.modifiers.is_empty() && ch.is_ascii_digit() => buttons
            .iter()
            .rev()
            .find(|button| button.hotkey == Some(ch))
            .cloned(),
        _ => None,
    }
}

/// Coalesce any number of invalidation increments observed between TUI polls
/// into one redraw. The first zero is the initial state; a non-zero initial
/// value means an invalidate preceded TUI startup and needs one refresh.
#[derive(Default)]
pub(crate) struct RenderGenerationTracker {
    observed: Option<u64>,
}

impl RenderGenerationTracker {
    pub(crate) fn observe(&mut self, generation: u64) -> bool {
        let changed = match self.observed {
            Some(previous) => previous != generation,
            None => generation != 0,
        };
        self.observed = Some(generation);
        changed
    }

    pub(crate) fn generation(&self) -> Option<u64> {
        self.observed
    }
}

fn lines_for(tree: &Value, depth: usize) -> Option<Vec<String>> {
    if depth > 32 {
        return None;
    }
    let kind = tree.get("type")?.as_str()?;
    let children = tree
        .get("children")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let empty_props = serde_json::Map::new();
    let props = tree
        .get("props")
        .and_then(Value::as_object)
        .unwrap_or(&empty_props);
    match kind {
        "Text" => {
            if !props.is_empty() {
                return None;
            }
            let text = children
                .iter()
                .map(Value::as_str)
                .collect::<Option<Vec<_>>>()?
                .join("");
            Some(vec![text])
        }
        "Button" => {
            let props = tree.get("props")?.as_object()?;
            let label = props.get("label")?.as_str()?;
            let hotkey = props.get("hotkey").and_then(Value::as_str);
            Some(vec![match hotkey {
                Some(hotkey) => format!("[{hotkey}] {label}"),
                None => format!("[{label}]"),
            }])
        }
        "Box" => {
            if props
                .keys()
                .any(|key| key != "flexDirection" && key != "columnGap")
                || props
                    .get("flexDirection")
                    .is_some_and(|value| !matches!(value.as_str(), Some("row" | "column")))
                || props.get("columnGap").is_some_and(|value| {
                    value
                        .as_f64()
                        .and_then(number_as_u64)
                        .is_none_or(|gap| gap > 16)
                })
            {
                return None;
            }
            let row = props.get("flexDirection").and_then(Value::as_str) == Some("row");
            let gap = props
                .get("columnGap")
                .and_then(Value::as_f64)
                .and_then(number_as_u64)
                .unwrap_or(1)
                .min(16) as usize;
            let mut child_lines = Vec::with_capacity(children.len());
            for child in children {
                child_lines.push(if let Some(text) = child.as_str() {
                    vec![text.to_owned()]
                } else {
                    lines_for(child, depth + 1)?
                });
            }
            if row {
                let height = child_lines.iter().map(Vec::len).max().unwrap_or(0);
                let separator = " ".repeat(gap);
                Some(
                    (0..height)
                        .map(|row_index| {
                            child_lines
                                .iter()
                                .map(|lines| lines.get(row_index).map(String::as_str).unwrap_or(""))
                                .collect::<Vec<_>>()
                                .join(&separator)
                        })
                        .collect(),
                )
            } else {
                Some(child_lines.into_iter().flatten().collect())
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

    fn source(value: &Value) -> Utf16JsonProjection {
        Utf16JsonProjection::from_serde_value(value).expect("test fixture is valid JSON")
    }

    #[tokio::test]
    async fn clear_then_render_awaits_same_revision_operations_in_order() {
        let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let clear_calls = calls.clone();
        let render_calls = calls.clone();
        let result = clear_then_render(
            move || async move {
                clear_calls.lock().unwrap().push(("clear", 12));
                Ok::<_, &'static str>(())
            },
            move || async move {
                render_calls.lock().unwrap().push(("render", 12));
                Ok::<_, &'static str>("tree")
            },
        )
        .await;

        assert_eq!(result, Ok("tree"));
        assert_eq!(*calls.lock().unwrap(), vec![("clear", 12), ("render", 12)]);
    }

    #[test]
    fn creates_viewport_props_and_draws_text_inside_boxes() {
        let event = above_prompt_event(88, 26, true, false, 7);
        assert_eq!(event.string_at("/component"), Some("AbovePrompt"));
        assert_eq!(event.string_at("/requestId"), Some(ABOVE_PROMPT_REQUEST_ID));
        assert_eq!(event.number_at("/props/bodyColumns"), Some(88.0));
        assert_eq!(event.number_at("/props/maxRows"), Some(7.0));
        assert_eq!(event.number_at("/props/scroll/offset"), Some(0.0));
        assert_eq!(event.number_at("/props/scroll/bodyRows"), Some(6.0));
        assert_eq!(event.object_entries().unwrap().len(), 5);
        assert_eq!(event.number_at("/viewport/rows"), Some(26.0));

        let lines = tree_lines(
            &source(&json!({
                "type":"Box",
                "props":{"flexDirection":"row","columnGap":2},
                "children":[
                    {"type":"Text","props":{},"children":["Ready"]},
                    {"type":"Text","props":{},"children":["2 items"]}
                ]
            })),
            2,
        )
        .unwrap();
        assert_eq!(lines, vec!["Ready  2 items"]);
    }

    #[test]
    fn tree_lines_respect_the_measured_above_prompt_budget() {
        let tree = json!({
            "type":"Box",
            "props":{"flexDirection":"column"},
            "children":[
                {"type":"Text","props":{},"children":["one"]},
                {"type":"Text","props":{},"children":["two"]},
                {"type":"Text","props":{},"children":["three"]}
            ]
        });
        let tree = source(&tree);
        assert_eq!(tree_lines(&tree, 2), Some(vec!["one".into(), "two".into()]));
        assert_eq!(tree_lines(&tree, 0), Some(Vec::new()));
    }

    #[test]
    fn button_render_and_keypress_keep_the_rendered_action_identity() {
        let button = |key: &str, label: &str, handle: u64, auto_focus: bool| {
            json!({
                "type":"Button",
                "props":{"key":key,"label":label,"hotkey":"1","autoFocus":auto_focus},
                "children":[],
                "press":{"plugin":"button-mod","handle":handle,
                    "workerEpoch":"worker-epoch","renderRevision":7}
            })
        };
        let tree = source(&json!({
            "type":"Box",
            "props":{"flexDirection":"column"},
            "children":[button("first","First",1,false), button("second","Second",2,true)]
        }));
        assert_eq!(
            tree_lines(&tree, 3),
            Some(vec!["[1] First".into(), "[1] Second".into()])
        );
        let actions = tree_buttons(&tree, 19).unwrap();
        assert_eq!(actions.len(), 2);
        assert_eq!(actions[1].site_revision, 19);
        assert_eq!(actions[1].render_revision, 7);
        assert_eq!(
            actions[1].press_event(),
            source(&json!({
                "surface":"terminal",
                "component":"AbovePrompt",
                "requestId":ABOVE_PROMPT_REQUEST_ID,
                "plugin":"button-mod",
                "element":"second",
                "press":{"handle":2,"workerEpoch":"worker-epoch","renderRevision":7}
            }))
        );
        let digit = KeyEvent::new(KeyCode::Char('1'), KeyModifiers::NONE);
        assert_eq!(
            button_for_key(&actions, digit, true, false)
                .unwrap()
                .key_display,
            "second",
            "later buttons win duplicate hotkeys"
        );
        let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(
            button_for_key(&actions, enter, true, false)
                .unwrap()
                .key_display,
            "second",
            "Enter activates the auto-focused button"
        );
        assert!(button_for_key(&actions, enter, false, false).is_none());
        assert!(button_for_key(&actions, enter, true, true).is_none());
    }

    #[test]
    fn button_children_are_validated_and_display_uses_the_native_label() {
        let tree = source(&json!({
            "type":"Button",
            "props":{"key":"mixed","label":"ABC"},
            "press":{"plugin":"button-mod","handle":4,
                "workerEpoch":"worker-epoch","renderRevision":7},
            "children":["A",{"type":"Text","props":{"bold":true},"children":["B","C"]}]
        }));
        assert_eq!(tree_lines(&tree, 1), Some(vec!["[ABC]".into()]));
        assert_eq!(tree_buttons(&tree, 9).unwrap().len(), 1);

        let folded = source(&json!({
            "type":"Button",
            "props":{"key":"folded","label":"Folded"},
            "press":{"plugin":"button-mod","handle":5,
                "workerEpoch":"worker-epoch","renderRevision":7}
        }));
        assert_eq!(tree_lines(&folded, 1), Some(vec!["[Folded]".into()]));
        assert_eq!(tree_buttons(&folded, 9).unwrap().len(), 1);
    }

    #[test]
    fn multiple_ui_render_invalidations_coalesce_to_one_redraw_generation() {
        let mut tracker = RenderGenerationTracker::default();
        assert!(
            !tracker.observe(0),
            "initial generation is already rendered"
        );
        assert!(!tracker.observe(0), "unchanged generation does not redraw");
        assert!(
            tracker.observe(3),
            "three unseen invalidations need one refresh"
        );
        assert!(!tracker.observe(3), "the same generation is consumed once");
        assert_eq!(tracker.generation(), Some(3));
    }
}
