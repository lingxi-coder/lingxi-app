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
