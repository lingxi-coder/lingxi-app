//! Tests for the PowerShell containment maps + normalizers (claude-code `y_`,
//! `hhe`, `Rgg`, `FKn`, `Rtt`, `LKn`), byte-checked against binary 2.1.206.

use super::*;

#[test]
fn normalize_resolves_common_aliases() {
    // `hhe` alias → canonical cmdlet, lowercased (claude-code `y_`).
    assert_eq!(normalize_cmdlet("ls"), "get-childitem");
    assert_eq!(normalize_cmdlet("gc"), "get-content");
    assert_eq!(normalize_cmdlet("cat"), "get-content");
    assert_eq!(normalize_cmdlet("rm"), "remove-item");
    assert_eq!(normalize_cmdlet("del"), "remove-item");
    assert_eq!(normalize_cmdlet("cp"), "copy-item");
    assert_eq!(normalize_cmdlet("mv"), "move-item");
    assert_eq!(normalize_cmdlet("cd"), "set-location");
    assert_eq!(normalize_cmdlet("%"), "foreach-object");
    assert_eq!(normalize_cmdlet("?"), "where-object");
}

#[test]
fn normalize_is_case_insensitive_and_canonical_passthrough() {
    assert_eq!(normalize_cmdlet("Get-Content"), "get-content");
    assert_eq!(normalize_cmdlet("GET-CONTENT"), "get-content");
    // A canonical cmdlet not in the alias table returns itself lowercased.
    assert_eq!(normalize_cmdlet("Remove-Item"), "remove-item");
    // Unknown command → itself (lowercased), unchanged.
    assert_eq!(normalize_cmdlet("Some-CustomCmdlet"), "some-customcmdlet");
}

#[test]
fn normalize_strips_exe_extension_only_without_path_separator() {
    // `dgg = /\.(exe|cmd|bat|com)$/` strips when there is no `/` or `\`.
    assert_eq!(normalize_cmdlet("pwsh.exe"), "pwsh");
    assert_eq!(normalize_cmdlet("gc.cmd"), "get-content"); // strip then alias
    assert_eq!(normalize_cmdlet("foo.bat"), "foo");
    assert_eq!(normalize_cmdlet("bar.com"), "bar");
    // A name WITH a path separator keeps its extension (no strip).
    assert_eq!(normalize_cmdlet("C:\\tools\\foo.exe"), "c:\\tools\\foo.exe");
    assert_eq!(normalize_cmdlet("./foo.exe"), "./foo.exe");
    // A non-exe extension is not stripped.
    assert_eq!(normalize_cmdlet("data.txt"), "data.txt");
}

#[test]
fn fkn_has_all_40_cmdlets() {
    assert_eq!(FKN.len(), 40, "FKn must carry all 40 path-taking cmdlets");
}

#[test]
fn fkn_operation_types_and_path_params() {
    let gc = FKN.get("get-content").expect("get-content in FKn");
    assert_eq!(gc.operation_type, PsOperation::Read);
    assert_eq!(gc.path_params, &["-path", "-literalpath", "-pspath", "-lp"]);

    let rm = FKN.get("remove-item").expect("remove-item in FKn");
    assert_eq!(rm.operation_type, PsOperation::Write);
    assert!(rm.known_switches.contains(&"-recurse"));

    // Out-File / Tee-Object lead with -FilePath.
    assert_eq!(FKN.get("out-file").unwrap().path_params[0], "-filepath");
}

#[test]
fn fkn_special_shapes() {
    // New-Item's -Name is a leaf-only path param.
    let ni = FKN.get("new-item").unwrap();
    assert_eq!(ni.leaf_only_path_params, &["-name"]);
    assert_eq!(ni.positional_skip, 0);

    // Invoke-WebRequest/RestMethod skip 1 positional (the URI) and are optional writes.
    for c in ["invoke-webrequest", "invoke-restmethod"] {
        let e = FKN.get(c).unwrap();
        assert_eq!(e.operation_type, PsOperation::Write);
        assert_eq!(e.positional_skip, 1, "{c} skips the positional URI");
        assert!(e.optional_write, "{c} is an optional write");
        assert_eq!(e.path_params, &["-outfile", "-infile"]);
    }

    // Copy/Move-Item include -Destination as a path param.
    assert!(FKN.get("copy-item").unwrap().path_params.contains(&"-destination"));
    assert!(FKN.get("move-item").unwrap().path_params.contains(&"-destination"));

    // Pop-Location takes no path params.
    assert!(FKN.get("pop-location").unwrap().path_params.is_empty());
}

#[test]
fn read_only_set_and_common_params() {
    assert!(RGG.contains("get-childitem"));
    assert!(RGG.contains("test-path"));
    assert!(!RGG.contains("set-content"));
    assert_eq!(RGG.len(), 8);

    assert_eq!(WWI, &["-verbose", "-debug"]);
    assert!(GWI.contains(&"-erroraction"));
    assert!(GWI.contains(&"-ea"));
    assert_eq!(GWI.len(), 14);
}

#[test]
fn is_parameter_matches_rtt() {
    // Known element type wins.
    assert!(is_parameter("anything", Some("Parameter")));
    assert!(!is_parameter("-path", Some("StringConstant")));
    // Fallback: leading parameter-prefix char (hyphen or unicode dashes).
    assert!(is_parameter("-path", None));
    assert!(is_parameter("\u{2013}path", None)); // en dash
    assert!(is_parameter("\u{2015}path", None)); // horizontal bar
    assert!(!is_parameter("path", None));
    assert!(!is_parameter("", None));
}

/// Build a command whose arg element types are all unknown (empty vector ⇒
/// `is_parameter` falls back to the leading-dash heuristic, like a real
/// text-only reconstruction).
fn cmd(name: &str, args: &[&str]) -> PsCommand {
    PsCommand {
        name: name.to_string(),
        args: args.iter().map(|s| (*s).to_string()).collect(),
        element_types: Vec::new(),
    }
}

/// Build a command with explicit per-arg element types (index 0 = name type).
fn cmd_typed(name: &str, args: &[&str], types: &[&str]) -> PsCommand {
    PsCommand {
        name: name.to_string(),
        args: args.iter().map(|s| (*s).to_string()).collect(),
        element_types: types.iter().map(|s| (*s).to_string()).collect(),
    }
}

#[test]
fn extract_positional_and_named_paths() {
    // Positional read path.
    let e = extract_paths(&cmd("Get-Content", &["foo.txt"]));
    assert_eq!(e.paths, vec!["foo.txt"]);
    assert_eq!(e.operation_type, PsOperation::Read);
    assert!(!e.has_unvalidatable_path_arg);

    // Named -Path with a skipped -Value.
    let e = extract_paths(&cmd("Set-Content", &["-Path", "out.txt", "-Value", "hello"]));
    assert_eq!(e.paths, vec!["out.txt"]);
    assert_eq!(e.operation_type, PsOperation::Write);

    // Alias resolves + positional path.
    assert_eq!(extract_paths(&cmd("gc", &["a.log"])).paths, vec!["a.log"]);
}

#[test]
fn extract_colon_form_and_quote_strip() {
    assert_eq!(extract_paths(&cmd("Set-Content", &["-Path:out.txt"])).paths, vec!["out.txt"]);
    // Surrounding quotes stripped from a -Param:value value.
    assert_eq!(extract_paths(&cmd("Set-Content", &["-Path:\"my file.txt\""])).paths, vec!["my file.txt"]);
    // Unambiguous abbreviation binds to -Path.
    assert_eq!(extract_paths(&cmd("Get-Content", &["-pa", "z.txt"])).paths, vec!["z.txt"]);
}

#[test]
fn extract_switch_ignored_value_param_skipped() {
    // -Recurse is a switch (no value); the positional after it is the path.
    let e = extract_paths(&cmd("Get-ChildItem", &["-Recurse", "src"]));
    assert_eq!(e.paths, vec!["src"]);
    // -Filter is a value param → its value is skipped, not treated as a path.
    let e = extract_paths(&cmd("Get-Content", &["-Filter", "*.rs", "real.txt"]));
    assert_eq!(e.paths, vec!["real.txt"]);
}

#[test]
fn extract_positional_skip_and_optional_write() {
    // Invoke-WebRequest skips the positional URI (positional_skip=1); only -OutFile is a path.
    let e = extract_paths(&cmd("Invoke-WebRequest", &["https://example.com/x", "-OutFile", "dl.bin"]));
    assert_eq!(e.paths, vec!["dl.bin"]);
    assert_eq!(e.operation_type, PsOperation::Write);
    assert!(e.optional_write);
    // With no path at all, extraction yields none (caller decides via optional_write).
    let e = extract_paths(&cmd("Invoke-WebRequest", &["https://example.com/x"]));
    assert!(e.paths.is_empty());
    assert!(e.optional_write);
}

#[test]
fn extract_leaf_only_param() {
    // New-Item -Name takes a bare leaf → valid path.
    assert_eq!(extract_paths(&cmd("New-Item", &["-Name", "notes.md"])).paths, vec!["notes.md"]);
    // A leaf value containing a separator is un-validatable, NOT a path.
    let e = extract_paths(&cmd("New-Item", &["-Name", "sub/notes.md"]));
    assert!(e.paths.is_empty());
    assert!(e.has_unvalidatable_path_arg);
}

#[test]
fn extract_unknown_param_and_array_value_flag_unvalidatable() {
    // Unknown parameter → unvalidatable; a -X:value still contributes its value.
    let e = extract_paths(&cmd("Set-Content", &["-Bogus:val"]));
    assert!(e.has_unvalidatable_path_arg);
    assert_eq!(e.paths, vec!["val"]);
    // An array-literal value on a path param is flagged unvalidatable (eGi).
    let e = extract_paths(&cmd("Set-Content", &["-Path:@(a,b)"]));
    assert!(e.has_unvalidatable_path_arg);
}

#[test]
fn extract_element_type_marks_unvalidatable() {
    // A path value whose AST element type is a Variable (not StringConstant/Parameter)
    // is pushed but flags unvalidatable (the `d()` peek).
    // types: [0]=cmd name, [1]=-Path (Parameter), [2]=$var (Variable)
    let e = extract_paths(&cmd_typed(
        "Set-Content",
        &["-Path", "$var"],
        &["StringConstant", "Parameter", "Variable"],
    ));
    assert_eq!(e.paths, vec!["$var"]);
    assert!(e.has_unvalidatable_path_arg);
}

#[test]
fn extract_non_path_cmdlet_is_empty_read() {
    let e = extract_paths(&cmd("Write-Output", &["hello", "world"]));
    assert!(e.paths.is_empty());
    assert_eq!(e.operation_type, PsOperation::Read);
    assert!(!e.has_unvalidatable_path_arg);
}

fn dirs(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| (*s).to_string()).collect()
}

#[test]
fn format_dir_list_matches_mkn() {
    assert_eq!(format_dir_list(&dirs(&["/a", "/b"])), "'/a', '/b'");
    assert_eq!(format_dir_list(&dirs(&["/a"])), "'/a'");
    // Exactly 5 → all listed.
    assert_eq!(
        format_dir_list(&dirs(&["/1", "/2", "/3", "/4", "/5"])),
        "'/1', '/2', '/3', '/4', '/5'"
    );
    // 6 → first 5 + "and 1 more".
    assert_eq!(
        format_dir_list(&dirs(&["/1", "/2", "/3", "/4", "/5", "/6"])),
        "'/1', '/2', '/3', '/4', '/5', and 1 more"
    );
    assert_eq!(
        format_dir_list(&dirs(&["/1", "/2", "/3", "/4", "/5", "/6", "/7", "/8"])),
        "'/1', '/2', '/3', '/4', '/5', and 3 more"
    );
}

#[test]
fn containment_messages_use_lingxi_brand_and_correct_verb() {
    let m = cmdlet_containment_message("get-content", "/etc/passwd", &dirs(&["/work"]));
    assert_eq!(
        m,
        "get-content targeting '/etc/passwd' was blocked. For security, LingXi may only access files in the allowed working directories for this session: '/work'."
    );
    let r = redirection_containment_message("/etc/x", &dirs(&["/work"]));
    assert_eq!(
        r,
        "Output redirection to '/etc/x' was blocked. For security, LingXi may only write to files in the allowed working directories for this session: '/work'."
    );
    assert_eq!(
        remove_item_protected_message("/etc"),
        "Remove-Item on system path '/etc' is blocked. This path is protected from removal."
    );
}

#[test]
fn expand_tilde_matches_ukn() {
    assert_eq!(expand_tilde("~", Some("/home/u")), "/home/u");
    assert_eq!(expand_tilde("~/proj", Some("/home/u")), "/home/u/proj");
    assert_eq!(expand_tilde("~\\proj", Some("/home/u")), "/home/u\\proj");
    // A `~user` form is NOT expanded (not `~`/`~/`/`~\`).
    assert_eq!(expand_tilde("~bob/x", Some("/home/u")), "~bob/x");
    // No home available → unchanged.
    assert_eq!(expand_tilde("~/proj", None), "~/proj");
    // Non-tilde unchanged.
    assert_eq!(expand_tilde("/abs/path", Some("/home/u")), "/abs/path");
}

#[test]
fn traversal_after_segment_matches_eur() {
    assert!(has_traversal_after_segment("a/../b", false));
    assert!(has_traversal_after_segment("./a/../b", false));
    assert!(has_traversal_after_segment("dir/..", false));
    // A leading `..` (before any real segment) does NOT count.
    assert!(!has_traversal_after_segment("../a", false));
    assert!(!has_traversal_after_segment("../../x", false));
    assert!(!has_traversal_after_segment("a/b/c", false));
    // Windows separators.
    assert!(has_traversal_after_segment("a\\..\\b", true));
    assert!(!has_traversal_after_segment("a\\..\\b", false)); // backslash not a sep off Windows
}

#[test]
fn glob_index_matches_uxe() {
    assert_eq!(glob_index("a*b"), Some(1));
    assert_eq!(glob_index("a?b"), Some(1));
    assert_eq!(glob_index("a[bc]d"), Some(1));
    assert_eq!(glob_index("plain.txt"), None);
    // `[` with no closing `]` is not a glob.
    assert_eq!(glob_index("a[bc"), None);
}

#[test]
fn dotdot_segment_matches_ote() {
    assert!(has_dotdot_segment("a/../b"));
    assert!(has_dotdot_segment("a/.."));
    assert!(has_dotdot_segment(".."));
    assert!(has_dotdot_segment("../a"));
    assert!(has_dotdot_segment("a\\..\\b"));
    // `..` embedded in a name is NOT a segment.
    assert!(!has_dotdot_segment("a..b"));
    assert!(!has_dotdot_segment("...."));
    assert!(!has_dotdot_segment("a/b"));
}

#[test]
fn glob_base_dir_matches_wgg() {
    assert_eq!(glob_base_dir("src/*.rs"), "src/");
    assert_eq!(glob_base_dir("*.rs"), ".");
    assert_eq!(glob_base_dir("/a/b/*.txt"), "/a/b/");
    // No glob → unchanged.
    assert_eq!(glob_base_dir("plain/path.txt"), "plain/path.txt");
}

#[test]
fn casefold_matches_hg() {
    assert_eq!(casefold_path("/Etc/PASSWD"), "/etc/passwd");
    assert_eq!(casefold_path("\u{0131}"), "i"); // dotless i → i
    assert_eq!(casefold_path("\u{017F}"), "s"); // long s → s
}

fn classify(raw: &str, op: PsOperation) -> PsPathClass {
    classify_ps_path(raw, op, false, Some("/home/u"))
}

fn reason_of(c: &PsPathClass) -> &str {
    match c {
        PsPathClass::Blocked { reason, .. } => reason,
        PsPathClass::Proceed { .. } => panic!("expected Blocked, got Proceed"),
    }
}

#[test]
fn classify_string_guards_block_with_exact_reasons() {
    use ps_path_reasons as R;
    assert_eq!(reason_of(&classify("~bob/x", PsOperation::Read)), R::TILDE_USER);
    assert_eq!(reason_of(&classify("a`b", PsOperation::Read)), R::BACKTICK);
    assert_eq!(reason_of(&classify("Registry::HKLM", PsOperation::Read)), R::PROVIDER_QUALIFIED);
    assert_eq!(reason_of(&classify("//server/share", PsOperation::Read)), R::UNC);
    assert_eq!(reason_of(&classify("$env:TEMP/x", PsOperation::Read)), R::VARIABLE_EXPANSION);
    assert_eq!(reason_of(&classify("dir/../escape", PsOperation::Read)), R::TRAVERSAL);
}

#[test]
fn classify_glob_reason_depends_on_operation() {
    use ps_path_reasons as R;
    assert_eq!(reason_of(&classify("out*.txt", PsOperation::Write)), R::GLOB_WRITE);
    assert_eq!(reason_of(&classify("out*.txt", PsOperation::Create)), R::GLOB_WRITE);
    assert_eq!(reason_of(&classify("in*.txt", PsOperation::Read)), R::GLOB_READ);
}

#[test]
fn classify_provider_and_drive_relative() {
    // Off Windows, a `C:foo` drive-relative is caught by the provider guard.
    let r = reason_of(&classify("C:foo", PsOperation::Read)).to_string();
    assert!(r.contains("uses a non-filesystem provider"));
    assert!(r.contains("C:foo"));
    // A URL-like provider prefix.
    assert!(reason_of(&classify("http:notafile", PsOperation::Read)).contains("non-filesystem provider"));

    // On Windows, `C:foo` is drive-relative (distinct reason).
    let w = classify_ps_path("C:foo", PsOperation::Read, true, Some("C:\\Users\\u"));
    assert!(reason_of(&w).contains("is drive-relative"));
}

#[test]
fn classify_clean_paths_proceed_normalized() {
    // Backslashes normalized, surrounding quotes stripped.
    assert_eq!(
        classify("src\\file.txt", PsOperation::Read),
        PsPathClass::Proceed { normalized: "src/file.txt".to_string() }
    );
    assert_eq!(
        classify("'quoted name.txt'", PsOperation::Read),
        PsPathClass::Proceed { normalized: "quoted name.txt".to_string() }
    );
    // `~/x` (tilde followed by '/') is expanded, NOT a ~user block.
    assert_eq!(
        classify("~/proj/a.txt", PsOperation::Read),
        PsPathClass::Proceed { normalized: "/home/u/proj/a.txt".to_string() }
    );
    // A leading `../` is not traversal-after-segment → proceeds.
    assert_eq!(
        classify("../sibling.txt", PsOperation::Read),
        PsPathClass::Proceed { normalized: "../sibling.txt".to_string() }
    );
    // A plain relative path proceeds.
    assert_eq!(
        classify("logs/today.txt", PsOperation::Read),
        PsPathClass::Proceed { normalized: "logs/today.txt".to_string() }
    );
}

#[test]
fn classify_guard_ordering_var_before_provider() {
    // `$` variable-expansion fires before the provider-prefix guard.
    use ps_path_reasons as R;
    assert_eq!(reason_of(&classify("env:$x", PsOperation::Read)), R::VARIABLE_EXPANSION);
}

#[test]
fn param_in_list_matches_lkn() {
    let list = &["-path", "-literalpath", "-pspath", "-lp"];
    // Exact match.
    assert!(param_in_list("-path", list));
    // Unambiguous ≥2-char prefix (PowerShell abbreviation): "-pa" → "-path".
    assert!(param_in_list("-pa", list));
    assert!(param_in_list("-li", list));
    // A single-char "-" is length 1 → prefix matching disabled, no exact "-".
    assert!(!param_in_list("-", list));
    // Non-matching prefix.
    assert!(!param_in_list("-zz", list));
}
