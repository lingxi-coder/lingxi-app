//! PowerShell command **path-containment** — byte-faithful port of claude-code's
//! `powershellPermissions.ts` `validatePowerShellCommandPaths` (`xgg`/`Z_u`,
//! binary 2.1.206 @218426477+). Sibling of the bash [`crate::command_path_containment`]
//! guard: it covers the path arguments of PowerShell cmdlets so a
//! `Get-Content`/`Set-Content`/`Out-File`/`Remove-Item`/… targeting a path
//! OUTSIDE the allowed working directories ASKS (or denies) instead of running.
//!
//! # Architecture (matches claude-code exactly)
//!
//! claude-code does NOT hand-parse PowerShell. It base64-encodes the command and
//! shells out to `pwsh` (`[System.Management.Automation.Language.Parser]::ParseInput`
//! via an embedded script), receives the AST as JSON, transforms it into a small
//! node model, then runs the pure validation [`validate_statements`] (`Z_u`) over
//! it. When `pwsh` is absent or the command does not parse, claude-code returns
//! `passthrough` — NO containment — so the guard is inert on hosts without
//! PowerShell (the port's macOS/Linux/mobile targets).
//!
//! This module is built bottom-up. THIS file is the pure, subprocess-free
//! foundation: the cmdlet maps + normalizers ([`normalize_cmdlet`] `y_`, the
//! alias table `hhe`, the read-only set `Rgg`, and the per-cmdlet path-param
//! config `FKn`). The `pwsh` subprocess + JSON-AST transform, the `X_u` path
//! extractor, and the `xgg` validation land in follow-up units against this base.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

/// Whether a cmdlet reads, writes, or creates — the `operationType` field of a
/// claude-code `FKn` entry (only `read`/`write` are used by the table; `Create`
/// is reserved for the `NKn`/`xgg` write-vs-create path branches).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PsOperation {
    /// A read/inspect cmdlet (e.g. `Get-Content`, `Test-Path`).
    Read,
    /// A write/modify/delete cmdlet (e.g. `Set-Content`, `Remove-Item`).
    Write,
    /// A create cmdlet — reserved for the `NKn`/`xgg` write-vs-create branches.
    Create,
}

/// Per-cmdlet path-parameter configuration — a claude-code `FKn[cmdlet]` entry.
///
/// `X_u` uses this to decide which of a command's arguments are FILE PATHS:
/// `path_params` (and `leaf_only_path_params`) name the parameters whose value
/// is a path; `known_switches` are boolean flags (no value); `known_value_params`
/// take a non-path value that must be skipped; `positional_skip` positional args
/// are ignored before positional paths begin (e.g. `Invoke-WebRequest`'s URI);
/// `optional_write` marks a write whose target path may legitimately be absent.
#[derive(Debug, Clone)]
pub struct FkEntry {
    /// Whether this cmdlet reads, writes, or creates.
    pub operation_type: PsOperation,
    /// Parameters whose value is a file path (e.g. `-Path`, `-LiteralPath`).
    pub path_params: &'static [&'static str],
    /// Boolean flags that take no value (e.g. `-Recurse`, `-Force`).
    pub known_switches: &'static [&'static str],
    /// Parameters that take a non-path value which must be skipped (e.g. `-Value`).
    pub known_value_params: &'static [&'static str],
    /// `leafOnlyPathParams` — parameters (e.g. `New-Item -Name`) whose value is a
    /// bare leaf name; a value containing a separator / `.` / `..` is treated as
    /// un-validatable (forces `hasUnvalidatablePathArg`) rather than a path.
    pub leaf_only_path_params: &'static [&'static str],
    /// `positionalSkip` — count of leading POSITIONAL args that are not paths.
    pub positional_skip: usize,
    /// `optionalWrite` — a write op whose path may be absent without asking.
    pub optional_write: bool,
}

impl FkEntry {
    const fn new(
        operation_type: PsOperation,
        path_params: &'static [&'static str],
        known_switches: &'static [&'static str],
        known_value_params: &'static [&'static str],
    ) -> Self {
        Self {
            operation_type,
            path_params,
            known_switches,
            known_value_params,
            leaf_only_path_params: &[],
            positional_skip: 0,
            optional_write: false,
        }
    }
    const fn leaf(mut self, leaf: &'static [&'static str]) -> Self {
        self.leaf_only_path_params = leaf;
        self
    }
    const fn skip(mut self, n: usize) -> Self {
        self.positional_skip = n;
        self
    }
    const fn opt_write(mut self) -> Self {
        self.optional_write = true;
        self
    }
}

use PsOperation::{Read, Write};

/// Parameters appended to EVERY cmdlet's `known_switches` (claude-code `WWi` —
/// the common `-Verbose`/`-Debug` switches).
pub const WWI: &[&str] = &["-verbose", "-debug"];

/// Parameters appended to EVERY cmdlet's `known_value_params` (claude-code `GWi`
/// — the common parameters, e.g. `-ErrorAction`, that take a non-path value).
pub const GWI: &[&str] = &[
    "-erroraction",
    "-warningaction",
    "-informationaction",
    "-progressaction",
    "-errorvariable",
    "-warningvariable",
    "-informationvariable",
    "-outvariable",
    "-outbuffer",
    "-pipelinevariable",
    "-ea",
    "-wa",
    "-infa",
    "-proga",
];

/// AST element types considered statically VALIDATABLE (claude-code `Agg`). A
/// path argument whose following element type is not in this set forces
/// `hasUnvalidatablePathArg` (→ ask).
pub static AGG: LazyLock<HashSet<&'static str>> =
    LazyLock::new(|| ["StringConstant", "Parameter"].into_iter().collect());

/// Characters that begin a PowerShell PARAMETER token (claude-code `EY`): ASCII
/// hyphen plus the en/em/horizontal-bar dashes PowerShell also accepts.
pub const EY: [char; 4] = ['-', '\u{2013}', '\u{2014}', '\u{2015}'];

/// Read-only cmdlets (claude-code `Rgg`) — used by `xgg` to track whether any
/// non-read-only command appeared upstream in a pipeline (path-source safety).
pub static RGG: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    [
        "get-childitem",
        "get-item",
        "get-itemproperty",
        "resolve-path",
        "convert-path",
        "get-filehash",
        "get-acl",
        "test-path",
    ]
    .into_iter()
    .collect()
});

/// PowerShell alias → canonical cmdlet name (claude-code `hhe`). Keys are the
/// lowercase alias; values are the canonical cmdlet in the binary's original
/// casing (lowercased at lookup time by [`normalize_cmdlet`]).
static HHE: LazyLock<HashMap<&'static str, &'static str>> = LazyLock::new(|| {
    [
        ("ls", "Get-ChildItem"),
        ("dir", "Get-ChildItem"),
        ("gci", "Get-ChildItem"),
        ("cat", "Get-Content"),
        ("type", "Get-Content"),
        ("gc", "Get-Content"),
        ("cd", "Set-Location"),
        ("sl", "Set-Location"),
        ("chdir", "Set-Location"),
        ("pushd", "Push-Location"),
        ("popd", "Pop-Location"),
        ("pwd", "Get-Location"),
        ("gl", "Get-Location"),
        ("gi", "Get-Item"),
        ("gp", "Get-ItemProperty"),
        ("ni", "New-Item"),
        ("mkdir", "New-Item"),
        ("md", "New-Item"),
        ("ri", "Remove-Item"),
        ("del", "Remove-Item"),
        ("rd", "Remove-Item"),
        ("rmdir", "Remove-Item"),
        ("rm", "Remove-Item"),
        ("erase", "Remove-Item"),
        ("mi", "Move-Item"),
        ("mv", "Move-Item"),
        ("move", "Move-Item"),
        ("ci", "Copy-Item"),
        ("cp", "Copy-Item"),
        ("copy", "Copy-Item"),
        ("cpi", "Copy-Item"),
        ("si", "Set-Item"),
        ("rni", "Rename-Item"),
        ("ren", "Rename-Item"),
        ("ps", "Get-Process"),
        ("gps", "Get-Process"),
        ("kill", "Stop-Process"),
        ("spps", "Stop-Process"),
        ("start", "Start-Process"),
        ("saps", "Start-Process"),
        ("sajb", "Start-Job"),
        ("ipmo", "Import-Module"),
        ("echo", "Write-Output"),
        ("write", "Write-Output"),
        ("sleep", "Start-Sleep"),
        ("help", "Get-Help"),
        ("man", "Get-Help"),
        ("gcm", "Get-Command"),
        ("gsv", "Get-Service"),
        ("gv", "Get-Variable"),
        ("sv", "Set-Variable"),
        ("h", "Get-History"),
        ("history", "Get-History"),
        ("iex", "Invoke-Expression"),
        ("iwr", "Invoke-WebRequest"),
        ("irm", "Invoke-RestMethod"),
        ("icm", "Invoke-Command"),
        ("ii", "Invoke-Item"),
        ("iwmi", "Invoke-WmiMethod"),
        ("icim", "Invoke-CimMethod"),
        ("nsn", "New-PSSession"),
        ("etsn", "Enter-PSSession"),
        ("exsn", "Exit-PSSession"),
        ("gsn", "Get-PSSession"),
        ("rsn", "Remove-PSSession"),
        ("cls", "Clear-Host"),
        ("clear", "Clear-Host"),
        ("select", "Select-Object"),
        ("where", "Where-Object"),
        ("foreach", "ForEach-Object"),
        ("%", "ForEach-Object"),
        ("?", "Where-Object"),
        ("measure", "Measure-Object"),
        ("ft", "Format-Table"),
        ("fl", "Format-List"),
        ("fw", "Format-Wide"),
        ("oh", "Out-Host"),
        ("ogv", "Out-GridView"),
        ("ac", "Add-Content"),
        ("clc", "Clear-Content"),
        ("tee", "Tee-Object"),
        ("epcsv", "Export-Csv"),
        ("sp", "Set-ItemProperty"),
        ("rp", "Remove-ItemProperty"),
        ("cli", "Clear-Item"),
        ("epal", "Export-Alias"),
        ("sls", "Select-String"),
    ]
    .into_iter()
    .collect()
});

/// Per-cmdlet path-param config (claude-code `FKn`) — 40 path-taking cmdlets.
/// Keyed by the lowercase canonical cmdlet name (post-[`normalize_cmdlet`]).
pub static FKN: LazyLock<HashMap<&'static str, FkEntry>> = LazyLock::new(|| {
    let pp: &[&str] = &["-path", "-literalpath", "-pspath", "-lp"];
    let mut m: HashMap<&'static str, FkEntry> = HashMap::new();
    m.insert(
        "set-content",
        FkEntry::new(
            Write,
            pp,
            &["-passthru", "-force", "-whatif", "-confirm", "-usetransaction", "-nonewline", "-asbytestream"],
            &["-value", "-filter", "-include", "-exclude", "-credential", "-encoding", "-stream"],
        ),
    );
    m.insert(
        "add-content",
        FkEntry::new(
            Write,
            pp,
            &["-passthru", "-force", "-whatif", "-confirm", "-usetransaction", "-nonewline", "-asbytestream"],
            &["-value", "-filter", "-include", "-exclude", "-credential", "-encoding", "-stream"],
        ),
    );
    m.insert(
        "remove-item",
        FkEntry::new(
            Write,
            pp,
            &["-recurse", "-force", "-whatif", "-confirm", "-usetransaction"],
            &["-filter", "-include", "-exclude", "-credential", "-stream"],
        ),
    );
    m.insert(
        "clear-content",
        FkEntry::new(
            Write,
            pp,
            &["-force", "-whatif", "-confirm", "-usetransaction"],
            &["-filter", "-include", "-exclude", "-credential", "-stream"],
        ),
    );
    m.insert(
        "out-file",
        FkEntry::new(
            Write,
            &["-filepath", "-path", "-literalpath", "-pspath", "-lp"],
            &["-append", "-force", "-noclobber", "-nonewline", "-whatif", "-confirm"],
            &["-inputobject", "-encoding", "-width"],
        ),
    );
    m.insert(
        "tee-object",
        FkEntry::new(
            Write,
            &["-filepath", "-path", "-literalpath", "-pspath", "-lp"],
            &["-append"],
            &["-inputobject", "-variable", "-encoding"],
        ),
    );
    m.insert(
        "export-csv",
        FkEntry::new(
            Write,
            pp,
            &["-append", "-force", "-noclobber", "-notypeinformation", "-includetypeinformation", "-useculture", "-noheader", "-whatif", "-confirm"],
            &["-inputobject", "-delimiter", "-encoding", "-quotefields", "-usequotes"],
        ),
    );
    m.insert(
        "export-clixml",
        FkEntry::new(
            Write,
            pp,
            &["-force", "-noclobber", "-whatif", "-confirm"],
            &["-inputobject", "-depth", "-encoding"],
        ),
    );
    m.insert(
        "new-item",
        FkEntry::new(
            Write,
            pp,
            &["-force", "-whatif", "-confirm", "-usetransaction"],
            &["-itemtype", "-value", "-credential", "-type"],
        )
        .leaf(&["-name"]),
    );
    m.insert(
        "copy-item",
        FkEntry::new(
            Write,
            &["-path", "-literalpath", "-pspath", "-lp", "-destination"],
            &["-container", "-force", "-passthru", "-recurse", "-whatif", "-confirm", "-usetransaction"],
            &["-filter", "-include", "-exclude", "-credential", "-fromsession", "-tosession"],
        ),
    );
    m.insert(
        "move-item",
        FkEntry::new(
            Write,
            &["-path", "-literalpath", "-pspath", "-lp", "-destination"],
            &["-force", "-passthru", "-whatif", "-confirm", "-usetransaction"],
            &["-filter", "-include", "-exclude", "-credential"],
        ),
    );
    m.insert(
        "rename-item",
        FkEntry::new(
            Write,
            pp,
            &["-force", "-passthru", "-whatif", "-confirm", "-usetransaction"],
            &["-newname", "-credential", "-filter", "-include", "-exclude"],
        ),
    );
    m.insert(
        "set-item",
        FkEntry::new(
            Write,
            pp,
            &["-force", "-passthru", "-whatif", "-confirm", "-usetransaction"],
            &["-value", "-credential", "-filter", "-include", "-exclude"],
        ),
    );
    m.insert(
        "get-content",
        FkEntry::new(
            Read,
            pp,
            &["-force", "-usetransaction", "-wait", "-raw", "-asbytestream"],
            &["-readcount", "-totalcount", "-tail", "-first", "-head", "-last", "-filter", "-include", "-exclude", "-credential", "-delimiter", "-encoding", "-stream"],
        ),
    );
    m.insert(
        "get-childitem",
        FkEntry::new(
            Read,
            pp,
            &["-recurse", "-force", "-name", "-usetransaction", "-followsymlink", "-directory", "-file", "-hidden", "-readonly", "-system"],
            &["-filter", "-include", "-exclude", "-depth", "-attributes", "-credential"],
        ),
    );
    m.insert(
        "get-item",
        FkEntry::new(
            Read,
            pp,
            &["-force", "-usetransaction"],
            &["-filter", "-include", "-exclude", "-credential", "-stream"],
        ),
    );
    m.insert(
        "get-itemproperty",
        FkEntry::new(
            Read,
            pp,
            &["-usetransaction"],
            &["-name", "-filter", "-include", "-exclude", "-credential"],
        ),
    );
    m.insert(
        "get-itempropertyvalue",
        FkEntry::new(
            Read,
            pp,
            &["-usetransaction"],
            &["-name", "-filter", "-include", "-exclude", "-credential"],
        ),
    );
    m.insert(
        "get-filehash",
        FkEntry::new(Read, pp, &[], &["-algorithm", "-inputstream"]),
    );
    m.insert(
        "get-acl",
        FkEntry::new(
            Read,
            pp,
            &["-audit", "-allcentralaccesspolicies", "-usetransaction"],
            &["-inputobject", "-filter", "-include", "-exclude"],
        ),
    );
    m.insert(
        "get-module",
        FkEntry::new(
            Read,
            &["-name", "-fullyqualifiedname"],
            &["-listavailable", "-all", "-refresh", "-skipeditioncheck"],
            &["-psedition", "-pssession", "-cimsession"],
        ),
    );
    m.insert(
        "format-hex",
        FkEntry::new(
            Read,
            pp,
            &["-raw"],
            &["-inputobject", "-encoding", "-count", "-offset"],
        ),
    );
    m.insert(
        "test-path",
        FkEntry::new(
            Read,
            pp,
            &["-isvalid", "-usetransaction"],
            &["-filter", "-include", "-exclude", "-pathtype", "-credential", "-olderthan", "-newerthan"],
        ),
    );
    m.insert(
        "resolve-path",
        FkEntry::new(
            Read,
            pp,
            &["-relative", "-usetransaction", "-force"],
            &["-credential", "-relativebasepath"],
        ),
    );
    m.insert(
        "convert-path",
        FkEntry::new(Read, pp, &["-usetransaction"], &[]),
    );
    m.insert(
        "select-string",
        FkEntry::new(
            Read,
            pp,
            &["-simplematch", "-casesensitive", "-quiet", "-list", "-notmatch", "-allmatches", "-noemphasis", "-raw"],
            &["-inputobject", "-pattern", "-include", "-exclude", "-encoding", "-context", "-culture"],
        ),
    );
    m.insert(
        "set-location",
        FkEntry::new(Read, pp, &["-passthru", "-usetransaction"], &["-stackname"]),
    );
    m.insert(
        "push-location",
        FkEntry::new(Read, pp, &["-passthru", "-usetransaction"], &["-stackname"]),
    );
    m.insert(
        "pop-location",
        FkEntry::new(Read, &[], &["-passthru", "-usetransaction"], &["-stackname"]),
    );
    m.insert(
        "select-xml",
        FkEntry::new(Read, pp, &[], &["-xml", "-content", "-xpath", "-namespace"]),
    );
    m.insert(
        "get-winevent",
        FkEntry::new(
            Read,
            &["-path"],
            &["-force", "-oldest"],
            &["-listlog", "-logname", "-listprovider", "-providername", "-maxevents", "-computername", "-credential", "-filterxpath", "-filterxml", "-filterhashtable"],
        ),
    );
    m.insert(
        "invoke-webrequest",
        FkEntry::new(
            Write,
            &["-outfile", "-infile"],
            &["-allowinsecureredirect", "-allowunencryptedauthentication", "-disablekeepalive", "-nobodyprogress", "-passthru", "-preservefileauthorizationmetadata", "-resume", "-skipcertificatecheck", "-skipheadervalidation", "-skiphttperrorcheck", "-usebasicparsing", "-usedefaultcredentials"],
            &["-uri", "-method", "-body", "-contenttype", "-headers", "-maximumredirection", "-maximumretrycount", "-proxy", "-proxycredential", "-retryintervalsec", "-sessionvariable", "-timeoutsec", "-token", "-transferencoding", "-useragent", "-websession", "-credential", "-authentication", "-certificate", "-certificatethumbprint", "-form", "-httpversion"],
        )
        .skip(1)
        .opt_write(),
    );
    m.insert(
        "invoke-restmethod",
        FkEntry::new(
            Write,
            &["-outfile", "-infile"],
            &["-allowinsecureredirect", "-allowunencryptedauthentication", "-disablekeepalive", "-followrellink", "-nobodyprogress", "-passthru", "-preservefileauthorizationmetadata", "-resume", "-skipcertificatecheck", "-skipheadervalidation", "-skiphttperrorcheck", "-usebasicparsing", "-usedefaultcredentials"],
            &["-uri", "-method", "-body", "-contenttype", "-headers", "-maximumfollowrellink", "-maximumredirection", "-maximumretrycount", "-proxy", "-proxycredential", "-responseheaderstvariable", "-retryintervalsec", "-sessionvariable", "-statuscodevariable", "-timeoutsec", "-token", "-transferencoding", "-useragent", "-websession", "-credential", "-authentication", "-certificate", "-certificatethumbprint", "-form", "-httpversion"],
        )
        .skip(1)
        .opt_write(),
    );
    m.insert(
        "expand-archive",
        FkEntry::new(
            Write,
            &["-path", "-literalpath", "-pspath", "-lp", "-destinationpath"],
            &["-force", "-passthru", "-whatif", "-confirm"],
            &[],
        ),
    );
    m.insert(
        "compress-archive",
        FkEntry::new(
            Write,
            &["-path", "-literalpath", "-pspath", "-lp", "-destinationpath"],
            &["-force", "-update", "-passthru", "-whatif", "-confirm"],
            &["-compressionlevel"],
        ),
    );
    m.insert(
        "set-itemproperty",
        FkEntry::new(
            Write,
            pp,
            &["-passthru", "-force", "-whatif", "-confirm", "-usetransaction"],
            &["-name", "-value", "-type", "-filter", "-include", "-exclude", "-credential", "-inputobject"],
        ),
    );
    m.insert(
        "new-itemproperty",
        FkEntry::new(
            Write,
            pp,
            &["-force", "-whatif", "-confirm", "-usetransaction"],
            &["-name", "-value", "-propertytype", "-type", "-filter", "-include", "-exclude", "-credential"],
        ),
    );
    m.insert(
        "remove-itemproperty",
        FkEntry::new(
            Write,
            pp,
            &["-force", "-whatif", "-confirm", "-usetransaction"],
            &["-name", "-filter", "-include", "-exclude", "-credential"],
        ),
    );
    m.insert(
        "clear-item",
        FkEntry::new(
            Write,
            pp,
            &["-force", "-whatif", "-confirm", "-usetransaction"],
            &["-filter", "-include", "-exclude", "-credential"],
        ),
    );
    m.insert(
        "export-alias",
        FkEntry::new(
            Write,
            pp,
            &["-append", "-force", "-noclobber", "-passthru", "-whatif", "-confirm"],
            &["-name", "-description", "-scope", "-as"],
        ),
    );
    m
});

/// Trailing-executable-extension regex characters (claude-code `dgg =
/// /\.(exe|cmd|bat|com)$/`) — stripped by [`normalize_cmdlet`] when the name has
/// no path separator.
fn strip_exe_ext(name: &str) -> &str {
    for ext in [".exe", ".cmd", ".bat", ".com"] {
        if name.len() > ext.len() && name[name.len() - ext.len()..].eq_ignore_ascii_case(ext) {
            return &name[..name.len() - ext.len()];
        }
    }
    name
}

/// Normalize a cmdlet/alias to its canonical lowercase cmdlet name (claude-code
/// `y_`): lowercase; if the name has no `/` or `\`, strip a trailing
/// `.exe/.cmd/.bat/.com`; resolve through the alias table `hhe`; return the
/// canonical name lowercased, else the (extension-stripped) name itself.
#[must_use]
pub fn normalize_cmdlet(name: &str) -> String {
    let lower = name.to_lowercase();
    let base = if !lower.contains('\\') && !lower.contains('/') {
        strip_exe_ext(&lower)
    } else {
        lower.as_str()
    };
    if let Some(canon) = HHE.get(base) {
        canon.to_lowercase()
    } else {
        base.to_string()
    }
}

/// A cmdlet argument is a PARAMETER token (claude-code `Rtt`): if the parsed AST
/// element type is known, it is `"Parameter"`; otherwise the raw token begins
/// with a parameter-prefix char ([`EY`]).
#[must_use]
pub fn is_parameter(arg: &str, element_type: Option<&str>) -> bool {
    if let Some(t) = element_type {
        return t == "Parameter";
    }
    arg.chars().next().is_some_and(|c| EY.contains(&c))
}

/// Prefix-match a normalized parameter `short` against a param list (claude-code
/// `LKn`): matches on exact equality, or (for a ≥2-char abbreviation) when a
/// list entry starts with `short` — PowerShell's unambiguous-prefix binding.
#[must_use]
pub fn param_in_list(short: &str, list: &[&str]) -> bool {
    list.iter()
        .any(|&r| r == short || (short.len() > 1 && r.starts_with(short)))
}

/// A single parsed PowerShell command (claude-code's transformed `CommandAst`
/// node). `element_types[0]` is the command NAME's simplified AST type;
/// `element_types[i + 1]` is the type of `args[i]` (`_Br`-mapped, e.g.
/// `"StringConstant"`, `"Parameter"`, `"SubExpression"`, `"Variable"`). A missing
/// entry (short vector) reads as "unknown", exactly like JS `a[p+1] === undefined`.
#[derive(Debug, Clone)]
pub struct PsCommand {
    /// The command name as written (alias or canonical cmdlet, any casing).
    pub name: String,
    /// The command's arguments, in order, as reconstructed token text.
    pub args: Vec<String>,
    /// Simplified AST element types: `[0]` is the name's type; `[i + 1]` is the
    /// type of `args[i]`. A short vector reads as "unknown" for missing entries.
    pub element_types: Vec<String>,
}

/// Result of [`extract_paths`] (claude-code `X_u`): the file-path arguments, the
/// cmdlet's operation, whether any path argument could not be statically
/// validated (→ the caller asks), and whether the write target is optional.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathExtraction {
    /// The file-path arguments discovered for the command.
    pub paths: Vec<String>,
    /// The cmdlet's read/write/create classification.
    pub operation_type: PsOperation,
    /// True when some path argument could not be statically validated (→ ask).
    pub has_unvalidatable_path_arg: bool,
    /// True when the write target may legitimately be absent (e.g. `Invoke-WebRequest`).
    pub optional_write: bool,
}

/// A quote character stripped by `L1`/tested by `eGi` (claude-code
/// `/['"‘-‟]/`): ASCII `'`/`"` plus the Unicode quote block
/// U+2018..U+201F (curly single/double quotes).
fn is_quote_char(c: char) -> bool {
    c == '\'' || c == '"' || ('\u{2018}'..='\u{201F}').contains(&c)
}

/// Leading whitespace stripped by `qWi` (`/^[\s᠎]+/`): JS `\s`
/// verbatim (incl. `﻿`, excl. Rust's ``) plus the explicit U+0085 and
/// U+180E.
fn is_ps_leading_ws(c: char) -> bool {
    matches!(
        c,
        '\u{0009}' | '\u{000A}' | '\u{000B}' | '\u{000C}' | '\u{000D}' | '\u{0020}'
            | '\u{0085}' | '\u{00A0}' | '\u{1680}' | '\u{180E}'
            | '\u{2000}'..='\u{200A}' | '\u{2028}' | '\u{2029}' | '\u{202F}'
            | '\u{205F}' | '\u{3000}' | '\u{FEFF}'
    )
}

/// Strip surrounding quote characters (claude-code `L1`: remove leading and
/// trailing runs of [`is_quote_char`]).
fn strip_surrounding_quotes(s: &str) -> &str {
    let start = s.find(|c| !is_quote_char(c)).unwrap_or(s.len());
    let t = &s[start..];
    let end = t.rfind(|c| !is_quote_char(c)).map_or(0, |i| i + t[i..].chars().next().unwrap().len_utf8());
    &t[..end]
}

/// Strip a leading whitespace run + any PowerShell comments (claude-code `Gee`):
/// leading `\s᠎`, then repeatedly a `<# … #>` block comment or a
/// `# … <newline>` line comment (each followed by another leading-ws strip).
fn strip_comments_and_leading_ws(s: &str) -> &str {
    let mut t = s.trim_start_matches(is_ps_leading_ws);
    loop {
        if let Some(rest) = t.strip_prefix("<#") {
            match rest.find("#>") {
                Some(i) => t = rest[i + 2..].trim_start_matches(is_ps_leading_ws),
                None => break,
            }
        } else if t.starts_with('#') {
            match t.find(['\r', '\n']) {
                // slice FROM the newline (claude-code `t.slice(r)`), then strip ws.
                Some(i) => t = t[i..].trim_start_matches(is_ps_leading_ws),
                None => break,
            }
        } else {
            break;
        }
    }
    t
}

/// `$Kn` — strip comments/leading-ws then surrounding quotes.
fn clean_value(s: &str) -> &str {
    strip_surrounding_quotes(strip_comments_and_leading_ws(s))
}

/// `eGi` — a parameter/positional VALUE that cannot be statically validated as a
/// single literal path: it contains a quote char, or (after [`clean_value`])
/// looks like an array / subexpression / variable / backtick-escape.
fn is_unvalidatable_value(s: &str) -> bool {
    if s.chars().any(is_quote_char) {
        return true;
    }
    let t = clean_value(s);
    t.contains(',')
        || t.starts_with('(')
        || t.starts_with('[')
        || t.contains('`')
        || t.contains("@(")
        || t.starts_with('@')
        || t.contains('$')
}

/// Extract the file-path arguments of a PowerShell command (claude-code `X_u`).
///
/// Cmdlets not in [`FKN`] take no path args (`{paths: [], read}`). For a
/// path-taking cmdlet, walk the args: parameters are matched against the
/// cmdlet's path / leaf-only / switch / value-param lists (with `WWi`/`GWi`
/// appended); a `-Param:value` value is quote-stripped, a `-Param value` value
/// consumes the next arg raw; positional args past `positional_skip` are paths.
/// Any arg whose following AST element type is not in [`AGG`], any array /
/// subexpression value, or any unknown parameter marks `has_unvalidatable_path_arg`.
#[must_use]
pub fn extract_paths(cmd: &PsCommand) -> PathExtraction {
    let canon = normalize_cmdlet(&cmd.name);
    let Some(entry) = FKN.get(canon.as_str()) else {
        return PathExtraction {
            paths: Vec::new(),
            operation_type: PsOperation::Read,
            has_unvalidatable_path_arg: false,
            optional_write: false,
        };
    };
    let switches: Vec<&str> = entry.known_switches.iter().chain(WWI).copied().collect();
    let value_params: Vec<&str> = entry.known_value_params.iter().chain(GWI).copied().collect();

    let s = &cmd.args;
    // element type of args[p] lives at element_types[p + 1] (index 0 = cmd name).
    let elem_type = |p: usize| -> Option<&str> { cmd.element_types.get(p + 1).map(String::as_str) };

    let mut paths: Vec<String> = Vec::new();
    let mut unvalidatable = false;
    let mut positional = 0usize;

    let mut p = 0usize;
    while p < s.len() {
        let f = &s[p];
        if f.is_empty() {
            p += 1;
            continue;
        }
        // `d(p)`: mark unvalidatable when the arg-at-p's element type is known and
        // not one of the statically-validatable types (StringConstant/Parameter).
        let peek_unvalidatable = |idx: usize| -> bool {
            matches!(elem_type(idx), Some(t) if !AGG.contains(t))
        };

        if is_parameter(f, elem_type(p)) {
            // Normalize the leading (possibly Unicode) dash to a single "-" and
            // split off an inline `-Param:value` value. Work on the tail after the
            // dash so a multi-byte dash never desyncs the colon offset.
            let first_len = f.chars().next().map_or(0, char::len_utf8);
            let rest = &f[first_len..];
            let (sname, colon_value): (String, Option<&str>) = match rest.find(':') {
                Some(ci) => (format!("-{}", &rest[..ci]).to_lowercase(), Some(&rest[ci + 1..])),
                None => (format!("-{rest}").to_lowercase(), None),
            };

            if param_in_list(&sname, entry.path_params) {
                let mut b: Option<String> = None;
                if let Some(v) = colon_value {
                    if is_unvalidatable_value(v) {
                        unvalidatable = true;
                    }
                    b = Some(clean_value(v).to_string());
                } else if let Some(v) = s.get(p + 1) {
                    // JS `if(v && !Rtt(v,w))` — an empty next-arg is falsy, not consumed.
                    if !v.is_empty() && !is_parameter(v, elem_type(p + 1)) {
                        b = Some(v.clone());
                        if peek_unvalidatable(p + 1) {
                            unvalidatable = true;
                        }
                        p += 1;
                    }
                }
                if let Some(b) = b {
                    if !b.is_empty() {
                        paths.push(b);
                    }
                }
            } else if !entry.leaf_only_path_params.is_empty()
                && param_in_list(&sname, entry.leaf_only_path_params)
            {
                let mut b: Option<String> = None;
                if let Some(v) = colon_value {
                    if is_unvalidatable_value(v) {
                        unvalidatable = true;
                    }
                    b = Some(clean_value(v).to_string());
                } else if let Some(v) = s.get(p + 1) {
                    // JS `if(v && !Rtt(v,w))` — an empty next-arg is falsy, not consumed.
                    if !v.is_empty() && !is_parameter(v, elem_type(p + 1)) {
                        b = Some(v.clone());
                        if peek_unvalidatable(p + 1) {
                            unvalidatable = true;
                        }
                        p += 1;
                    }
                }
                if let Some(b) = b {
                    if b.contains('/') || b.contains('\\') || b == "." || b == ".." {
                        unvalidatable = true;
                    } else {
                        paths.push(b);
                    }
                }
            } else if param_in_list(&sname, &switches) {
                // known switch → no value, ignore
            } else if param_in_list(&sname, &value_params) {
                // known value param → skip its (non-path) value
                if let Some(v) = colon_value {
                    if is_unvalidatable_value(v) {
                        unvalidatable = true;
                    }
                } else if let Some(v) = s.get(p + 1) {
                    // JS `if(v && !Rtt(v,w))` — an empty next-arg is falsy, not consumed.
                    if !v.is_empty() && !is_parameter(v, elem_type(p + 1)) {
                        if peek_unvalidatable(p + 1) {
                            unvalidatable = true;
                        }
                        p += 1;
                    }
                }
            } else {
                // unknown parameter → cannot validate; a `-X:value` form still
                // contributes its value as a path candidate.
                unvalidatable = true;
                if let Some(v) = colon_value {
                    paths.push(clean_value(v).to_string());
                }
            }
            p += 1;
            continue;
        }

        // positional argument
        if positional < entry.positional_skip {
            positional += 1;
            p += 1;
            continue;
        }
        positional += 1;
        if peek_unvalidatable(p) {
            unvalidatable = true;
        }
        paths.push(f.clone());
        p += 1;
    }

    PathExtraction {
        paths,
        operation_type: entry.operation_type,
        has_unvalidatable_path_arg: unvalidatable,
        optional_write: entry.optional_write,
    }
}

#[cfg(test)]
#[path = "powershell_containment_test.rs"]
mod powershell_containment_test;
