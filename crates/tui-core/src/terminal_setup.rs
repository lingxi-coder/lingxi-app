//! `/terminal-setup` host logic: detect the active terminal and install the
//! Shift+Enter (Apple Terminal: Option+Enter) newline keybinding by writing the
//! terminal's OWN config. 1:1 port of claude-code
//! `src/commands/terminalSetup/{index.ts,terminalSetup.tsx}`.
//!
//! Pure synchronous host I/O (std::fs + std::process::Command); no async / no
//! engine handle, so the TUI slash path can call [`run`] directly on its
//! blocking render loop with no reactor hazard (contrast `/compact`). Coloring
//! is collapsed to the transcript's single `is_error` flag: claude-code's
//! `success`/`warning`/`dim` styling becomes plain text, and only the reference
//! `throw new Error(...)` (hard-failure) branches return `is_error = true`.

/// Detect the active terminal, returning claude-code's `env.terminal` token
/// (e.g. `"Apple_Terminal"`, `"vscode"`, `"iTerm.app"`, `"ghostty"`). Faithful
/// subset of claude-code `utils/env.ts::detectTerminal` covering every token
/// this command branches on; the long Linux/Windows fallback tail collapses to
/// the raw `$TERM_PROGRAM`/`$TERM` passthrough, which is all that is needed here.
#[must_use]
pub fn detect_terminal() -> Option<String> {
    let get = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());

    if get("CURSOR_TRACE_ID").is_some() {
        return Some("cursor".to_string());
    }
    if let Some(v) = get("VSCODE_GIT_ASKPASS_MAIN") {
        if v.contains("cursor") {
            return Some("cursor".to_string());
        }
        if v.contains("windsurf") {
            return Some("windsurf".to_string());
        }
        if v.contains("antigravity") {
            return Some("antigravity".to_string());
        }
    }
    if let Some(bundle) = get("__CFBundleIdentifier").map(|s| s.to_lowercase()) {
        if bundle.contains("vscodium") {
            return Some("codium".to_string());
        }
        if bundle.contains("windsurf") {
            return Some("windsurf".to_string());
        }
    }
    // TERM is checked before TERM_PROGRAM for the CSI-u terminals whose TERM and
    // TERM_PROGRAM can disagree.
    if let Some(term) = get("TERM") {
        if term == "xterm-ghostty" {
            return Some("ghostty".to_string());
        }
        if term.contains("kitty") {
            return Some("kitty".to_string());
        }
    }
    if let Some(tp) = get("TERM_PROGRAM") {
        return Some(tp);
    }
    if get("ALACRITTY_LOG").is_some() {
        return Some("alacritty".to_string());
    }
    if let Some(term) = get("TERM") {
        if term.contains("alacritty") {
            return Some("alacritty".to_string());
        }
        return Some(term);
    }
    None
}

/// Terminals whose *palette row* claude-code hides via
/// `isHidden: env.terminal in NATIVE_CSIU_TERMINALS` — the **index.ts** map,
/// which does NOT include Warp. Returns the display name (unused by the gate but
/// shared with [`native_csiu_display_name`]).
fn hidden_terminal_display(terminal: Option<&str>) -> Option<&'static str> {
    match terminal? {
        "ghostty" => Some("Ghostty"),
        "kitty" => Some("Kitty"),
        "iTerm.app" => Some("iTerm2"),
        "WezTerm" => Some("WezTerm"),
        _ => None,
    }
}

/// `true` when the palette/`/help` row must be hidden — the active terminal is
/// in claude-code's index.ts `NATIVE_CSIU_TERMINALS` gate set (Ghostty, Kitty,
/// iTerm2, WezTerm; **not** Warp). Consulted by `command::is_runtime_hidden`.
#[must_use]
pub fn terminal_natively_supports_csiu() -> bool {
    hidden_terminal_display(detect_terminal().as_deref()).is_some()
}

/// Terminals for which the command reports "natively supported" — the
/// **terminalSetup.tsx** map, which DOES include Warp. (Deliberate asymmetry
/// with the palette gate above, preserved from the reference.)
fn native_csiu_display_name(terminal: Option<&str>) -> Option<&'static str> {
    match terminal? {
        "WarpTerminal" => Some("Warp"),
        other => hidden_terminal_display(Some(other)),
    }
}

/// The 2.1.205 `get description()` display-name map (includes Warp AND
/// Windows Terminal, unlike the two CSI-u maps above).
fn check_setup_display_name(terminal: &str) -> Option<&'static str> {
    match terminal {
        "ghostty" => Some("Ghostty"),
        "kitty" => Some("Kitty"),
        "iTerm.app" => Some("iTerm2"),
        "WezTerm" => Some("WezTerm"),
        "WarpTerminal" => Some("Warp"),
        "windows-terminal" => Some("Windows Terminal"),
        _ => None,
    }
}

/// `/terminal-setup`'s live palette description — 1:1 port of claude-code
/// 2.1.205's `get description()` (branch order preserved; 2.1.205 dropped the
/// old `isHidden` CSI-u gate in favor of this per-terminal text):
/// 1. Apple Terminal → the Option+Enter variant.
/// 2. Terminals with native Shift+Enter → "Check terminal setup (…)".
/// 3. Running under the iTerm2 app bundle but a nested/undetected terminal
///    (tmux/screen; the literal `"iTerm.app"` arm is unreachable after branch
///    2 — dead in the reference too, kept for fidelity) → clipboard-access
///    variant.
/// 4. Everything else → the install variant.
#[must_use]
pub fn dynamic_description() -> String {
    let terminal = detect_terminal();
    if terminal.as_deref() == Some("Apple_Terminal") {
        return "Enable Option+Enter key binding for newlines and visual bell".to_string();
    }
    if let Some(label) = terminal.as_deref().and_then(check_setup_display_name) {
        return format!("Check terminal setup (Shift+Enter is natively supported in {label})");
    }
    let bundle_is_iterm =
        std::env::var("__CFBundleIdentifier").ok().as_deref() == Some("com.googlecode.iterm2");
    if bundle_is_iterm
        && matches!(
            terminal.as_deref(),
            Some("iTerm.app") | Some("tmux") | Some("screen") | None
        )
    {
        return "Enable iTerm2 clipboard access for /copy".to_string();
    }
    "Install Shift+Enter key binding for newlines".to_string()
}

/// Port of `shouldOfferTerminalSetup()`: which terminals this command can
/// actually configure. Note claude-code's `&&`-binds-tighter precedence:
/// `(darwin && Apple_Terminal) || vscode || cursor || windsurf || alacritty || zed`.
fn should_offer_terminal_setup(terminal: Option<&str>) -> bool {
    let is = |name: &str| terminal == Some(name);
    (cfg!(target_os = "macos") && is("Apple_Terminal"))
        || is("vscode")
        || is("cursor")
        || is("windsurf")
        || is("alacritty")
        || is("zed")
}

/// Entry point (port of `call()`): returns `(message, is_error)` for the
/// transcript. Never panics; all I/O failures fold into the returned message.
#[must_use]
pub fn run() -> (String, bool) {
    let terminal = detect_terminal();

    if let Some(name) = native_csiu_display_name(terminal.as_deref()) {
        return (
            format!(
                "Shift+Enter is natively supported in {name}.\n\nNo configuration needed. Just use Shift+Enter to add newlines."
            ),
            false,
        );
    }

    if !should_offer_terminal_setup(terminal.as_deref()) {
        let terminal_name = terminal
            .clone()
            .unwrap_or_else(|| "your current terminal".to_string());
        let mut platform_terminals = String::new();
        if cfg!(target_os = "macos") {
            platform_terminals.push_str("   \u{2022} macOS: Apple Terminal\n");
        } else if cfg!(target_os = "windows") {
            platform_terminals.push_str("   \u{2022} Windows: Windows Terminal\n");
        }
        let message = format!(
            "Terminal setup cannot be run from {terminal_name}.\n\n\
This command configures a convenient Shift+Enter shortcut for multi-line prompts.\n\
Note: You can already use backslash (\\) + return to add newlines.\n\n\
To set up the shortcut (optional):\n\
1. Exit tmux/screen temporarily\n\
2. Run /terminal-setup directly in one of these terminals:\n\
{platform_terminals}   \u{2022} IDE: VSCode, Cursor, Devin Desktop, Zed\n\
   \u{2022} Other: Alacritty\n\
3. Return to tmux/screen - settings will persist\n\n\
Note: iTerm2, WezTerm, Ghostty, Kitty, Warp, and Windows Terminal support Shift+Enter natively."
        );
        return (message, false);
    }

    match setup_terminal(terminal.as_deref()) {
        Ok(message) => (message, false),
        Err(message) => (message, true),
    }
}

/// Dispatch to the per-terminal installer (port of `setupTerminal`). `Ok` =
/// normal result (success or advisory warning), `Err` = hard failure
/// (`is_error`).
fn setup_terminal(terminal: Option<&str>) -> Result<String, String> {
    match terminal {
        Some("Apple_Terminal") => enable_option_as_meta_for_terminal(),
        Some("vscode") => install_bindings_for_vscode("VSCode", "Code"),
        Some("cursor") => install_bindings_for_vscode("Cursor", "Cursor"),
        Some("windsurf") => install_bindings_for_vscode("Windsurf", "Windsurf"),
        Some("alacritty") => install_bindings_for_alacritty(),
        Some("zed") => install_bindings_for_zed(),
        _ => Ok(String::new()),
    }
}

/// `<path>.<8-hex>.bak` sibling backup path. claude-code uses `randomBytes(4)`;
/// we derive the suffix from the wall-clock nanos — cosmetic-only difference.
fn backup_path(path: &std::path::Path) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let mut raw = path.as_os_str().to_os_string();
    raw.push(format!(".{nanos:08x}.bak"));
    std::path::PathBuf::from(raw)
}

/// Port of `isVSCodeRemoteSSH()`: keybindings must be installed on the LOCAL
/// machine, so a remote session is refused with instructions.
fn is_vscode_remote_ssh() -> bool {
    let askpass = std::env::var("VSCODE_GIT_ASKPASS_MAIN").unwrap_or_default();
    let path = std::env::var("PATH").unwrap_or_default();
    [".vscode-server", ".cursor-server", ".windsurf-server"]
        .iter()
        .any(|marker| askpass.contains(marker) || path.contains(marker))
}

/// Port of `installBindingsForVSCodeTerminal`. `editor` is the display name;
/// `editor_dir` is the user-config subdir (`Code`/`Cursor`/`Windsurf`).
fn install_bindings_for_vscode(editor: &str, editor_dir: &str) -> Result<String, String> {
    let fail = || format!("Failed to install {editor} terminal Shift+Enter key binding");

    if is_vscode_remote_ssh() {
        return Ok(format!(
            "Cannot install keybindings from a remote {editor} session.\n\n\
{editor} keybindings must be installed on your local machine, not the remote server.\n\n\
To install the Shift+Enter keybinding:\n\
1. Open {editor} on your local machine (not connected to remote)\n\
2. Open the Command Palette (Cmd/Ctrl+Shift+P) \u{2192} \"Preferences: Open Keyboard Shortcuts (JSON)\"\n\
3. Add this keybinding (the file must be a JSON array):\n\n\
[\n  {{\n    \"key\": \"shift+enter\",\n    \"command\": \"workbench.action.terminal.sendSequence\",\n    \"args\": {{ \"text\": \"\\\\u001b\\\\r\" }},\n    \"when\": \"terminalFocus\"\n  }}\n]"
        ));
    }

    let Some(config_dir) = dirs::config_dir() else {
        return Err(fail());
    };
    let user_dir = config_dir.join(editor_dir).join("User");
    let keybindings_path = user_dir.join("keybindings.json");

    if std::fs::create_dir_all(&user_dir).is_err() {
        return Err(fail());
    }

    let (content, file_exists) = match std::fs::read_to_string(&keybindings_path) {
        Ok(content) => (content, true),
        Err(_) => ("[]".to_string(), false),
    };

    if file_exists {
        let backup = backup_path(&keybindings_path);
        if std::fs::copy(&keybindings_path, &backup).is_err() {
            return Ok(format!(
                "Error backing up existing {editor} terminal keybindings. Bailing out.\nSee {}\nBackup path: {}",
                keybindings_path.display(),
                backup.display()
            ));
        }
    }

    let mut bindings: Vec<serde_json::Value> = serde_json::from_str::<serde_json::Value>(&content)
        .ok()
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default();

    let already = bindings.iter().any(|b| {
        b.get("key").and_then(|v| v.as_str()) == Some("shift+enter")
            && b.get("command").and_then(|v| v.as_str())
                == Some("workbench.action.terminal.sendSequence")
            && b.get("when").and_then(|v| v.as_str()) == Some("terminalFocus")
    });
    if already {
        return Ok(format!(
            "Found existing {editor} terminal Shift+Enter key binding. Remove it to continue.\nSee {}",
            keybindings_path.display()
        ));
    }

    bindings.push(serde_json::json!({
        "key": "shift+enter",
        "command": "workbench.action.terminal.sendSequence",
        "args": { "text": "\u{001b}\r" },
        "when": "terminalFocus"
    }));

    let Ok(rendered) = serde_json::to_string_pretty(&bindings) else {
        return Err(fail());
    };
    if std::fs::write(&keybindings_path, rendered).is_err() {
        return Err(fail());
    }
    Ok(format!(
        "Installed {editor} terminal Shift+Enter key binding\nSee {}",
        keybindings_path.display()
    ))
}

/// Port of `installBindingsForAlacritty`.
fn install_bindings_for_alacritty() -> Result<String, String> {
    const KEYBINDING: &str =
        "[[keyboard.bindings]]\nkey = \"Return\"\nmods = \"Shift\"\nchars = \"\\u001B\\r\"";
    let fail = || "Failed to install Alacritty Shift+Enter key binding".to_string();

    let base = std::env::var("XDG_CONFIG_HOME")
        .ok()
        .filter(|v| !v.is_empty())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".config"));
    let config_path = base.join("alacritty").join("alacritty.toml");

    let (mut content, exists) = match std::fs::read_to_string(&config_path) {
        Ok(content) => (content, true),
        Err(_) => (String::new(), false),
    };

    if exists {
        if content.contains("mods = \"Shift\"") && content.contains("key = \"Return\"") {
            return Ok(format!(
                "Found existing Alacritty Shift+Enter key binding. Remove it to continue.\nSee {}",
                config_path.display()
            ));
        }
        let backup = backup_path(&config_path);
        if std::fs::copy(&config_path, &backup).is_err() {
            return Ok(format!(
                "Error backing up existing Alacritty config. Bailing out.\nSee {}\nBackup path: {}",
                config_path.display(),
                backup.display()
            ));
        }
    } else if let Some(parent) = config_path.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            return Err(fail());
        }
    }

    if !content.is_empty() && !content.ends_with('\n') {
        content.push('\n');
    }
    content.push('\n');
    content.push_str(KEYBINDING);
    content.push('\n');

    if std::fs::write(&config_path, content).is_err() {
        return Err(fail());
    }
    Ok(format!(
        "Installed Alacritty Shift+Enter key binding\nYou may need to restart Alacritty for changes to take effect\nSee {}",
        config_path.display()
    ))
}

/// Port of `installBindingsForZed` (always `~/.config/zed/keymap.json`, even on
/// macOS).
fn install_bindings_for_zed() -> Result<String, String> {
    let fail = || "Failed to install Zed Shift+Enter key binding".to_string();
    let zed_dir = dirs::home_dir()
        .unwrap_or_default()
        .join(".config")
        .join("zed");
    let keymap_path = zed_dir.join("keymap.json");

    if std::fs::create_dir_all(&zed_dir).is_err() {
        return Err(fail());
    }

    let (content, exists) = match std::fs::read_to_string(&keymap_path) {
        Ok(content) => (content, true),
        Err(_) => ("[]".to_string(), false),
    };

    if exists {
        if content.contains("shift-enter") {
            return Ok(format!(
                "Found existing Zed Shift+Enter key binding. Remove it to continue.\nSee {}",
                keymap_path.display()
            ));
        }
        let backup = backup_path(&keymap_path);
        if std::fs::copy(&keymap_path, &backup).is_err() {
            return Ok(format!(
                "Error backing up existing Zed keymap. Bailing out.\nSee {}\nBackup path: {}",
                keymap_path.display(),
                backup.display()
            ));
        }
    }

    let mut keymap: Vec<serde_json::Value> = serde_json::from_str::<serde_json::Value>(&content)
        .ok()
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default();
    keymap.push(serde_json::json!({
        "context": "Terminal",
        "bindings": { "shift-enter": ["terminal::SendText", "\u{001b}\r"] }
    }));

    let Ok(rendered) = serde_json::to_string_pretty(&keymap) else {
        return Err(fail());
    };
    if std::fs::write(&keymap_path, rendered + "\n").is_err() {
        return Err(fail());
    }
    Ok(format!(
        "Installed Zed Shift+Enter key binding\nSee {}",
        keymap_path.display()
    ))
}

/// Port of `enableOptionAsMetaForTerminal`: on macOS, set `useOptionAsMetaKey`
/// and disable the audio bell on the default (and, if different, startup)
/// Terminal.app profile, then flush cfprefsd. Backs the plist up first and
/// restores it on failure.
#[cfg(target_os = "macos")]
fn enable_option_as_meta_for_terminal() -> Result<String, String> {
    use std::process::Command;

    let plist = dirs::home_dir()
        .unwrap_or_default()
        .join("Library/Preferences/com.apple.Terminal.plist");
    let plist_arg = plist.to_string_lossy().to_string();
    let backup = backup_path(&plist);
    let had_backup = std::fs::copy(&plist, &backup).is_ok();
    let restore = || {
        if had_backup {
            let _ = Command::new("defaults")
                .arg("import")
                .arg("com.apple.Terminal")
                .arg(&backup)
                .status();
        }
    };

    let read_profile = |key: &str| -> Option<String> {
        let out = Command::new("defaults")
            .args(["read", "com.apple.Terminal", key])
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let value = String::from_utf8_lossy(&out.stdout).trim().to_string();
        (!value.is_empty()).then_some(value)
    };

    // Add first (creates the key), then Set if Add failed (key already present).
    let set_key = |profile: &str, key: &str, value: &str| -> bool {
        let add = Command::new("/usr/libexec/PlistBuddy")
            .arg("-c")
            .arg(format!(
                "Add :'Window Settings':'{profile}':{key} bool {value}"
            ))
            .arg(&plist_arg)
            .status();
        if add.map(|s| s.success()).unwrap_or(false) {
            return true;
        }
        Command::new("/usr/libexec/PlistBuddy")
            .arg("-c")
            .arg(format!("Set :'Window Settings':'{profile}':{key} {value}"))
            .arg(&plist_arg)
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    };

    let error_no_backup =
        "Failed to enable Option as Meta key for Terminal.app. No backup was available to restore from.";
    let error_restored =
        "Failed to enable Option as Meta key for Terminal.app. Your settings have been restored from backup.";

    if !had_backup {
        return Err(error_no_backup.to_string());
    }
    let Some(default_profile) = read_profile("Default Window Settings") else {
        restore();
        return Err(error_restored.to_string());
    };
    let Some(startup_profile) = read_profile("Startup Window Settings") else {
        restore();
        return Err(error_restored.to_string());
    };

    let mut updated = false;
    updated |= set_key(&default_profile, "useOptionAsMetaKey", "true");
    updated |= set_key(&default_profile, "Bell", "false");
    if startup_profile != default_profile {
        updated |= set_key(&startup_profile, "useOptionAsMetaKey", "true");
        updated |= set_key(&startup_profile, "Bell", "false");
    }

    if !updated {
        restore();
        return Err(error_restored.to_string());
    }

    let _ = Command::new("killall").arg("cfprefsd").status();
    Ok("Configured Terminal.app settings:\n- Enabled \"Use Option as Meta key\"\n- Switched to visual bell\nOption+Enter will now enter a newline.\nYou must restart Terminal.app for changes to take effect.".to_string())
}

#[cfg(not(target_os = "macos"))]
fn enable_option_as_meta_for_terminal() -> Result<String, String> {
    // `Apple_Terminal` is only reachable on macOS (guarded by
    // `should_offer_terminal_setup`); this stub keeps the module cross-compiling.
    Ok(String::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Serializes the env-mutating tests in this module (process env is global).
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn with_terminal<T>(term_program: Option<&str>, f: impl FnOnce() -> T) -> T {
        let _guard = ENV_LOCK.lock().unwrap();
        for k in [
            "CURSOR_TRACE_ID",
            "VSCODE_GIT_ASKPASS_MAIN",
            "__CFBundleIdentifier",
            "TERM",
            "TERM_PROGRAM",
            "ALACRITTY_LOG",
        ] {
            std::env::remove_var(k);
        }
        if let Some(tp) = term_program {
            std::env::set_var("TERM_PROGRAM", tp);
        }
        let out = f();
        std::env::remove_var("TERM_PROGRAM");
        out
    }

    #[test]
    fn iterm_is_native_and_palette_hidden() {
        with_terminal(Some("iTerm.app"), || {
            assert!(terminal_natively_supports_csiu());
            let (msg, is_err) = run();
            assert!(msg.contains("natively supported in iTerm2"));
            assert!(!is_err);
        });
    }

    #[test]
    fn dynamic_description_branches_per_terminal() {
        with_terminal(Some("Apple_Terminal"), || {
            assert_eq!(
                dynamic_description(),
                "Enable Option+Enter key binding for newlines and visual bell"
            );
        });
        with_terminal(Some("iTerm.app"), || {
            assert_eq!(
                dynamic_description(),
                "Check terminal setup (Shift+Enter is natively supported in iTerm2)"
            );
        });
        with_terminal(Some("WarpTerminal"), || {
            assert_eq!(
                dynamic_description(),
                "Check terminal setup (Shift+Enter is natively supported in Warp)"
            );
        });
        with_terminal(Some("tmux"), || {
            // Not under the iTerm2 bundle in the test env ⇒ install variant.
            assert_eq!(
                dynamic_description(),
                "Install Shift+Enter key binding for newlines"
            );
        });
        with_terminal(Some("tmux"), || {
            std::env::set_var("__CFBundleIdentifier", "com.googlecode.iterm2");
            let desc = dynamic_description();
            std::env::remove_var("__CFBundleIdentifier");
            assert_eq!(desc, "Enable iTerm2 clipboard access for /copy");
        });
    }

    #[test]
    fn warp_reports_native_but_is_not_palette_hidden() {
        // Deliberate index.ts vs terminalSetup.tsx asymmetry.
        with_terminal(Some("WarpTerminal"), || {
            assert!(!terminal_natively_supports_csiu());
            assert!(run().0.contains("natively supported in Warp"));
        });
    }

    #[test]
    fn unsupported_terminal_shows_guidance_not_error() {
        with_terminal(Some("tmux"), || {
            assert!(!terminal_natively_supports_csiu());
            let (msg, is_err) = run();
            assert!(msg.starts_with("Terminal setup cannot be run from tmux."));
            assert!(msg.contains("support Shift+Enter natively"));
            assert!(!is_err);
        });
    }
}
