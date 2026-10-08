//! TUI-owned transport and rendering for the first Mod UI site.
//!
//! This vertical slice draws terminal `AbovePrompt` trees made from `Box` and
//! `Text`. Other elements and callback controls are intentionally not exposed.
//! Its row budget is measured from the live bottom-pane layout, and a session
//! generation carries `$.ui.invalidate('ui.render')` through the draw loop.

#[cfg(test)]
use crossterm::event::KeyModifiers;
use crossterm::event::{KeyCode, KeyEvent};
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
    pub(crate) plugin: String,
    pub(crate) key: String,
    pub(crate) handle: u64,
    pub(crate) worker_epoch: String,
    pub(crate) render_revision: u64,
    pub(crate) site_revision: u64,
    pub(crate) label: String,
    pub(crate) hotkey: Option<char>,
    pub(crate) auto_focus: bool,
}

impl AbovePromptButtonAction {
    pub(crate) fn press_event(&self) -> Value {
        json!({
            "surface":"terminal",
            "component":"AbovePrompt",
            "requestId":ABOVE_PROMPT_REQUEST_ID,
            "plugin":self.plugin,
            "element":self.key,
            "press":{
                "handle":self.handle,
                "workerEpoch":self.worker_epoch,
                "renderRevision":self.render_revision,
            }
        })
    }
}

pub(crate) fn above_prompt_event(
    columns: u16,
    rows: u16,
    is_fullscreen: bool,
    is_working: bool,
    max_rows: u16,
) -> Value {
    json!({
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
    })
}

/// Turn the supported JSON tree into a small number of terminal rows.
pub(crate) fn tree_lines(tree: &Value, max_rows: u16) -> Option<Vec<String>> {
    let lines = lines_for(tree, 0)?;
    Some(lines.into_iter().take(usize::from(max_rows)).collect())
}

pub(crate) fn tree_buttons(
    tree: &Value,
    site_revision: u64,
) -> Option<Vec<AbovePromptButtonAction>> {
    if site_revision == 0 {
        return None;
    }
    let mut buttons = Vec::new();
    collect_buttons(tree, site_revision, &mut buttons)?;
    Some(buttons)
}

fn collect_buttons(
    tree: &Value,
    site_revision: u64,
    buttons: &mut Vec<AbovePromptButtonAction>,
) -> Option<()> {
    match tree.get("type")?.as_str()? {
        "Button" => {
            let props = tree.get("props")?.as_object()?;
            let press = tree.get("press")?.as_object()?;
            if props.keys().any(|key| {
                !matches!(
                    key.as_str(),
                    "key"
                        | "label"
                        | "hotkey"
                        | "action"
                        | "plain"
                        | "dimColor"
                        | "variant"
                        | "role"
                        | "autoFocus"
                )
            }) || press.keys().any(|key| {
                !matches!(
                    key.as_str(),
                    "plugin" | "handle" | "workerEpoch" | "renderRevision"
                )
            }) || !tree.get("children")?.as_array()?.is_empty()
            {
                return None;
            }
            let key = props.get("key")?.as_str()?.to_owned();
            let label = props.get("label")?.as_str()?.to_owned();
            let hotkey = props
                .get("hotkey")
                .and_then(Value::as_str)
                .and_then(|value| value.chars().next());
            if props.get("hotkey").is_some_and(|value| {
                value.as_str().is_none_or(|value| {
                    value.len() != 1
                        || !value
                            .bytes()
                            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
                })
            }) {
                return None;
            }
            let plugin = press.get("plugin")?.as_str()?.to_owned();
            let worker_epoch = press.get("workerEpoch")?.as_str()?.to_owned();
            let handle = press.get("handle")?.as_u64().filter(|value| *value > 0)?;
            let render_revision = press
                .get("renderRevision")?
                .as_u64()
                .filter(|value| *value > 0)?;
            if plugin.is_empty() || worker_epoch.is_empty() || key.is_empty() || label.is_empty() {
                return None;
            }
            buttons.push(AbovePromptButtonAction {
                plugin,
                key,
                handle,
                worker_epoch,
                render_revision,
                site_revision,
                label,
                hotkey,
                auto_focus: props.get("autoFocus").and_then(Value::as_bool) == Some(true),
            });
        }
        "Box" => {
            for child in tree.get("children")?.as_array()? {
                if !child.is_string() {
                    collect_buttons(child, site_revision, buttons)?;
                }
            }
        }
        "Text" => {}
        _ => return None,
    }
    Some(())
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
    let children = tree.get("children")?.as_array()?;
    let props = tree.get("props")?.as_object()?;
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
            let mut parsed = Vec::new();
            collect_buttons(tree, 1, &mut parsed)?;
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
                || props
                    .get("columnGap")
                    .is_some_and(|value| value.as_u64().is_none_or(|gap| gap > 16))
            {
                return None;
            }
            let row = props.get("flexDirection").and_then(Value::as_str) == Some("row");
            let gap = props
                .get("columnGap")
                .and_then(Value::as_u64)
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
        assert_eq!(event["component"], "AbovePrompt");
        assert_eq!(event["requestId"], ABOVE_PROMPT_REQUEST_ID);
        assert_eq!(event["props"]["bodyColumns"], 88);
        assert_eq!(event["props"]["maxRows"], 7);
        assert_eq!(event["props"]["scroll"]["offset"], 0);
        assert_eq!(event["props"]["scroll"]["bodyRows"], 6);
        assert_eq!(event["props"]["view"], json!({}));
        assert_eq!(event["viewport"]["rows"], 26);

        let lines = tree_lines(
            &json!({
                "type":"Box",
                "props":{"flexDirection":"row","columnGap":2},
                "children":[
                    {"type":"Text","props":{},"children":["Ready"]},
                    {"type":"Text","props":{},"children":["2 items"]}
                ]
            }),
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
        let tree = json!({
            "type":"Box",
            "props":{"flexDirection":"column"},
            "children":[button("first","First",1,false), button("second","Second",2,true)]
        });
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
            json!({
                "surface":"terminal",
                "component":"AbovePrompt",
                "requestId":ABOVE_PROMPT_REQUEST_ID,
                "plugin":"button-mod",
                "element":"second",
                "press":{"handle":2,"workerEpoch":"worker-epoch","renderRevision":7}
            })
        );
        let digit = KeyEvent::new(KeyCode::Char('1'), KeyModifiers::NONE);
        assert_eq!(
            button_for_key(&actions, digit, true, false).unwrap().key,
            "second",
            "later buttons win duplicate hotkeys"
        );
        let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(
            button_for_key(&actions, enter, true, false).unwrap().key,
            "second",
            "Enter activates the auto-focused button"
        );
        assert!(button_for_key(&actions, enter, false, false).is_none());
        assert!(button_for_key(&actions, enter, true, true).is_none());
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
