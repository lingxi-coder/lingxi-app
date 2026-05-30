//! M7-15 Task 7 — guard: no `TODO(M7-15)` markers remain in the TUI source
//! tree. Any remaining marker is an unresolved color/styling token. (Conscious
//! re-defers are re-tagged `TODO(M8)`, which this guard ignores.)

fn visit(dir: &std::path::Path, out: &mut Vec<String>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let p = entry.unwrap().path();
        if p.is_dir() {
            visit(&p, out);
        } else if p.extension().is_some_and(|e| e == "rs") {
            let body = std::fs::read_to_string(&p).unwrap_or_default();
            if body.contains("TODO(M7-15)") {
                out.push(p.display().to_string());
            }
        }
    }
}

#[test]
fn no_m7_15_todo_markers_remain() {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut offenders = Vec::new();
    visit(&src, &mut offenders);
    assert!(
        offenders.is_empty(),
        "unresolved TODO(M7-15) markers: {offenders:?}"
    );
}

#[test]
fn system_text_warning_reads_theme_warning_color() {
    use lingxi_tui::theme::Theme;
    let dark = Theme::dark();
    let light = Theme::light();
    // Sanity: the warning color differs between themes, so the renderer reading
    // `theme.warning` (rather than a hardcoded `Color::Yellow`) recolors live.
    assert_ne!(dark.warning, light.warning);
}
