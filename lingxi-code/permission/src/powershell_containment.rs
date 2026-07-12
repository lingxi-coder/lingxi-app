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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PsCommand {
    /// The command name as written (alias or canonical cmdlet, any casing).
    pub name: String,
    /// The command's arguments, in order, as reconstructed token text.
    pub args: Vec<String>,
    /// Simplified AST element types: `[0]` is the name's type; `[i + 1]` is the
    /// type of `args[i]`. A short vector reads as "unknown" for missing entries.
    pub element_types: Vec<String>,
    /// This command's output redirections (claude-code `l.redirections`).
    pub redirections: Vec<PsRedirection>,
}

/// A PowerShell output redirection (claude-code transformed `RedirectionAst`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PsRedirection {
    /// The redirection target path (empty / merging redirections are skipped).
    pub target: String,
    /// A `2>&1`-style stream merge (no file target → skipped by `xgg`).
    pub is_merging: bool,
}

/// One element of a pipeline statement (claude-code `e.commands` entry).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PsElement {
    /// A `CommandAst` — a cmdlet/command invocation to validate.
    Command(PsCommand),
    /// A non-`CommandAst` pipeline element (subexpression, script block, …) whose
    /// output could feed a downstream command's path — a pipeline source.
    Expression {
        /// The element's source text (used for the pipeline deny-rule pre-check).
        text: String,
    },
}

/// A parsed PowerShell statement / pipeline (claude-code `xgg`'s input `e`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PsStatement {
    /// The pipeline's elements, in order.
    pub commands: Vec<PsElement>,
    /// Flattened nested commands (inside script blocks / control flow).
    pub nested_commands: Vec<PsCommand>,
    /// Statement-level output redirections.
    pub redirections: Vec<PsRedirection>,
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

// ───────────────────────────────────────────────────────────────────────────
// Message builders + `NKn` string-guard reasons + leaf path helpers.
//
// The model-facing brand is "LingXi" (the product rebrand — claude-code says
// "Claude Code"), matching the port's existing containment messages in
// `command_path_containment` / `path_constraints`.
// ───────────────────────────────────────────────────────────────────────────

/// `MKn` dir-list truncation threshold (claude-code `ZWi = 5`).
const DIR_LIST_MAX: usize = 5;

/// Format the allowed-working-directory list (claude-code `MKn`): up to
/// [`DIR_LIST_MAX`] quoted dirs joined by `", "`, else the first five plus
/// `", and N more"`.
#[must_use]
pub fn format_dir_list(dirs: &[String]) -> String {
    let quoted = |d: &str| format!("'{d}'");
    if dirs.len() <= DIR_LIST_MAX {
        dirs.iter().map(|d| quoted(d)).collect::<Vec<_>>().join(", ")
    } else {
        let head = dirs[..DIR_LIST_MAX]
            .iter()
            .map(|d| quoted(d))
            .collect::<Vec<_>>()
            .join(", ");
        format!("{head}, and {} more", dirs.len() - DIR_LIST_MAX)
    }
}

/// The cmdlet path-containment deny/ask message (claude-code template B). Note
/// the verb is the fixed `"access files in"` for ALL cmdlets (read AND write) —
/// only output-redirection targets use `"write to files in"`.
#[must_use]
pub fn cmdlet_containment_message(cmdlet: &str, path: &str, dirs: &[String]) -> String {
    format!(
        "{cmdlet} targeting '{path}' was blocked. For security, LingXi may only access files in the allowed working directories for this session: {}.",
        format_dir_list(dirs)
    )
}

/// The output-redirection containment message (claude-code, `"write to files in"`
/// variant). Identical wording to [`crate::path_constraints`]'s redirection block.
#[must_use]
pub fn redirection_containment_message(path: &str, dirs: &[String]) -> String {
    format!(
        "Output redirection to '{path}' was blocked. For security, LingXi may only write to files in the allowed working directories for this session: {}.",
        format_dir_list(dirs)
    )
}

/// `Remove-Item` protected-system-path deny message (claude-code `Hwt`).
#[must_use]
pub fn remove_item_protected_message(path: &str) -> String {
    format!("Remove-Item on system path '{path}' is blocked. This path is protected from removal.")
}

/// The `NKn` string-guard "other"-type reasons — the manual-approval messages a
/// PowerShell path triggers when it cannot be statically validated. Ported
/// verbatim from claude-code `NKn` (binary 2.1.206).
pub mod ps_path_reasons {
    /// A `~user` (tilde not followed by `/`) home reference.
    pub const TILDE_USER: &str =
        "Paths beginning with ~user cannot be statically validated and require manual approval";
    /// A backtick escape in the path.
    pub const BACKTICK: &str =
        "Backtick escape characters in paths cannot be statically validated and require manual approval";
    /// A `::` module-qualified provider path.
    pub const PROVIDER_QUALIFIED: &str =
        "Module-qualified provider paths (::) cannot be statically validated and require manual approval";
    /// A UNC / WebDAV / SSL path.
    pub const UNC: &str =
        "UNC paths are blocked because they can trigger network requests and credential leakage";
    /// A `$`/`%` variable-expansion path.
    pub const VARIABLE_EXPANSION: &str =
        "Variable expansion syntax in paths requires manual approval";
    /// A `..` traversal after a real directory segment.
    pub const TRAVERSAL: &str =
        "Path contains '..' traversal after a directory segment, which may follow a symlink outside the working directory";
    /// A glob in a write/create operation.
    pub const GLOB_WRITE: &str =
        "Glob patterns are not allowed in write operations. Please specify an exact file path.";
    /// A glob in a read operation.
    pub const GLOB_READ: &str =
        "Glob patterns in paths cannot be statically validated \u{2014} symlinks inside the glob expansion are not examined. Requires manual approval.";
}

/// Reason for a Windows drive-relative path (claude-code interpolates the path).
#[must_use]
pub fn drive_relative_reason(path: &str) -> String {
    format!("Path '{path}' is drive-relative (resolves against the per-drive current directory, which cannot be statically validated) and requires manual approval")
}

/// Reason for a non-filesystem provider path (claude-code interpolates the path).
#[must_use]
pub fn non_fs_provider_reason(path: &str) -> String {
    format!("Path '{path}' uses a non-filesystem provider and requires manual approval")
}

/// Expand a leading `~`/`~/`/`~\` to the home directory (claude-code `UKn`).
/// With no home available the path is returned unchanged.
#[must_use]
pub fn expand_tilde(path: &str, home: Option<&str>) -> String {
    let is_tilde = path == "~" || path.starts_with("~/") || path.starts_with("~\\");
    match (is_tilde, home) {
        (true, Some(h)) => format!("{h}{}", &path[1..]),
        _ => path.to_string(),
    }
}

/// True when a `..` segment appears AFTER a real directory segment (claude-code
/// `eUr`) — i.e. a traversal that could escape via a symlinked component. Empty
/// and `.` segments are ignored; a leading run of `..` (before any real segment)
/// does not count.
#[must_use]
pub fn has_traversal_after_segment(path: &str, is_windows: bool) -> bool {
    let segs: Vec<&str> = if is_windows {
        path.split(['\\', '/']).collect()
    } else {
        path.split('/').collect()
    };
    let mut real_seen = false;
    for seg in segs {
        if seg.is_empty() || seg == "." {
            continue;
        }
        if seg == ".." {
            if real_seen {
                return true;
            }
        } else {
            real_seen = true;
        }
    }
    false
}

/// First glob-metacharacter index (claude-code `Uxe`): the position of `*`, `?`,
/// or a `[` that has a later matching `]`; `None` when the path has no glob.
#[must_use]
pub fn glob_index(path: &str) -> Option<usize> {
    let bytes: Vec<char> = path.chars().collect();
    for (t, &r) in bytes.iter().enumerate() {
        if r == '*' || r == '?' {
            return Some(t);
        }
        if r == '[' && bytes[t + 1..].contains(&']') {
            return Some(t);
        }
    }
    None
}

/// True when `..` appears as a full path segment (claude-code `OTe`:
/// `/(?:^|[\\/])\.\.(?:[\\/]|$)/`).
#[must_use]
pub fn has_dotdot_segment(path: &str) -> bool {
    let is_sep = |c: char| c == '/' || c == '\\';
    let chars: Vec<char> = path.chars().collect();
    let n = chars.len();
    let mut i = 0;
    while i + 1 < n {
        if chars[i] == '.' && chars[i + 1] == '.' {
            let before_ok = i == 0 || is_sep(chars[i - 1]);
            let after_ok = i + 2 >= n || is_sep(chars[i + 2]);
            if before_ok && after_ok {
                return true;
            }
        }
        i += 1;
    }
    false
}

/// The base directory of a glob path (claude-code `wgg`): the portion up to the
/// last separator before the first glob char; `"."` when the glob has no
/// leading directory; the glob path unchanged when it has no glob.
#[must_use]
pub fn glob_base_dir(path: &str) -> String {
    let Some(t) = glob_index(path) else {
        return path.to_string();
    };
    let prefix = &path[..path.char_indices().nth(t).map_or(path.len(), |(b, _)| b)];
    let last_sep = prefix.rfind(['/', '\\']);
    match last_sep {
        None => ".".to_string(),
        Some(n) => {
            let dir = &prefix[..n + 1];
            if dir.is_empty() {
                "/".to_string()
            } else {
                dir.to_string()
            }
        }
    }
}

/// Case-fold a path for comparison (claude-code `Hg`): lowercase, then map the
/// dotless-i (U+0131) → `i` and long-s (U+017F) → `s`.
#[must_use]
pub fn casefold_path(path: &str) -> String {
    path.to_lowercase().replace('\u{0131}', "i").replace('\u{017F}', "s")
}

/// Classification of a single PowerShell path argument by the `NKn` string-guard
/// sequence — the pure, roots-independent half of containment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PsPathClass {
    /// A guard (or glob) fired: block with this fixed `"other"`-type reason,
    /// which the caller surfaces as an ASK. `resolved` is the reported blocked
    /// path (metadata; the ask message is `reason`, not a template).
    Blocked {
        /// The reported blocked path.
        resolved: String,
        /// The manual-approval reason (used verbatim as the ask message).
        reason: String,
    },
    /// No guard fired; `normalized` (quotes stripped, tilde-expanded, backslashes
    /// normalized) proceeds to working-directory containment.
    Proceed {
        /// The normalized path to check against the allowed working directories.
        normalized: String,
    },
}

/// A Windows drive-relative path (claude-code `/^[a-z]:(?![/\\])/i`): a single
/// letter, a colon, and NOT immediately a separator.
fn is_drive_relative(i: &str) -> bool {
    let b = i.as_bytes();
    b.len() >= 2
        && b[0].is_ascii_alphabetic()
        && b[1] == b':'
        && !matches!(b.get(2), Some(b'/' | b'\\'))
}

/// A non-filesystem provider prefix (claude-code `/^[a-z0-9]{2,}:/i` on Windows,
/// `/^[a-z0-9]+:/i` elsewhere): an alphanumeric run then a colon.
fn is_provider_prefix(i: &str, is_windows: bool) -> bool {
    let run: usize = i.bytes().take_while(u8::is_ascii_alphanumeric).count();
    let min = if is_windows { 2 } else { 1 };
    run >= min && i.as_bytes().get(run) == Some(&b':')
}

/// Windows 8.3 short-name expansion (claude-code `tGi`) — a no-op off Windows,
/// which is the only platform the port validates PowerShell on.
fn short_name_expand(i: &str, _is_windows: bool) -> String {
    // `tGi` returns the input unchanged on every non-Windows platform; the
    // per-segment `PKn` 8.3 expansion is Windows-registry-backed and not modeled.
    i.to_string()
}

/// Classify a PowerShell path argument through the `NKn` string-guard sequence
/// (claude-code `NKn`, binary 2.1.206). Returns an early [`PsPathClass::Blocked`]
/// for a path that cannot be statically validated (`~user`, backtick, `::`,
/// drive-relative, UNC, `$`/`%`, non-fs provider, `..`-traversal, glob), else
/// [`PsPathClass::Proceed`] with the normalized path for working-dir containment.
///
/// NOTE: claude-code additionally pre-checks a deny RULE inside the backtick /
/// `::` / traversal / glob branches (returning a `deny` instead of an ask when a
/// rule matches). Those rule pre-checks are intentionally omitted here: the port
/// runs its deny-rule walk UPSTREAM of containment (in `PermissionPolicy`), so a
/// rule-matched command is already denied before this runs; the residual case
/// (a rule targeting a transformed sub-path) degrades to ASK, which still blocks
/// auto-execution.
#[must_use]
pub fn classify_ps_path(
    raw: &str,
    op: PsOperation,
    is_windows: bool,
    home: Option<&str>,
) -> PsPathClass {
    use ps_path_reasons as R;
    // i = UKn(L1(e)).replaceAll("\\","/")
    let mut i = expand_tilde(strip_surrounding_quotes(raw), home).replace('\\', "/");

    let blocked = |resolved: &str, reason: String| PsPathClass::Blocked {
        resolved: resolved.to_string(),
        reason,
    };

    // ~user (tilde NOT followed by '/'): `/^~[^/]/`
    if i.starts_with('~') && i.as_bytes().get(1).is_some_and(|&c| c != b'/') {
        return blocked(&i, R::TILDE_USER.to_string());
    }
    // backtick escape
    if i.contains('`') {
        return blocked(&i, R::BACKTICK.to_string());
    }
    // `::` module-qualified provider
    if i.contains("::") {
        return blocked(&i, R::PROVIDER_QUALIFIED.to_string());
    }
    // Windows drive-relative
    if is_windows && is_drive_relative(&i) {
        return blocked(&i, drive_relative_reason(&i));
    }
    // tGi short-name expansion, then UNC / WebDAV / SSL
    i = short_name_expand(&i, is_windows);
    if i.starts_with("//")
        || i.to_ascii_lowercase().contains("davwwwroot")
        || i.to_ascii_uppercase().contains("@SSL@")
    {
        return blocked(&i, R::UNC.to_string());
    }
    // `$`/`%` variable expansion
    if i.contains('$') || i.contains('%') {
        return blocked(&i, R::VARIABLE_EXPANSION.to_string());
    }
    // non-filesystem provider prefix
    if is_provider_prefix(&i, is_windows) {
        return blocked(&i, non_fs_provider_reason(&i));
    }
    // `..` traversal after a real segment
    if has_traversal_after_segment(&i, is_windows) {
        return blocked(&i, R::TRAVERSAL.to_string());
    }
    // glob
    if glob_index(&i).is_some() {
        let reason = if matches!(op, PsOperation::Write | PsOperation::Create) {
            R::GLOB_WRITE
        } else {
            R::GLOB_READ
        };
        return blocked(&i, reason.to_string());
    }
    PsPathClass::Proceed { normalized: i }
}

/// The outcome of the complete per-path check (claude-code `NKn`): the string
/// guards ([`classify_ps_path`]) followed by working-directory containment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PsPathOutcome {
    /// The path resolves inside an allowed working directory — no constraint.
    /// Carries the resolved path so `xgg`'s `Remove-Item` `TKt` guard (which
    /// fires even on an allowed path) can inspect it.
    Allowed {
        /// The resolved path (inside the allowed working dirs).
        resolved: String,
    },
    /// Blocked by a string guard — ASK with `reason` used verbatim.
    AskReason {
        /// The reported blocked path.
        resolved: String,
        /// The manual-approval reason (the ask message).
        reason: String,
    },
    /// Blocked by working-directory containment — the caller builds the template-B
    /// `"<cmdlet> targeting '<resolved>' … access files in …"` message (or the
    /// redirection variant) from `resolved`.
    AskContainment {
        /// The resolved path outside the allowed working directories.
        resolved: String,
    },
}

/// The complete `NKn` per-path check: run the [`classify_ps_path`] string guards,
/// then (for a clean path) resolve it and test membership in the allowed working
/// directories, reusing the port's [`crate::filesystem::path_in_allowed_working_path`]
/// (the same containment `check_command_path_containment` uses for bash).
///
/// `additional` are the extra allowed working dirs (TS `additionalWorkingDirectories`);
/// `is_windows` selects the Windows-specific guard variants (drive-relative,
/// provider min length). Deny-RULE resolution is handled upstream in the policy
/// gate (see [`classify_ps_path`]).
#[must_use]
pub fn check_ps_path(
    raw: &str,
    op: PsOperation,
    roots: &crate::filesystem::FsRoots,
    additional: &[std::path::PathBuf],
    is_windows: bool,
) -> PsPathOutcome {
    let home = roots.home.as_deref().map(|p| p.to_string_lossy().into_owned());
    match classify_ps_path(raw, op, is_windows, home.as_deref()) {
        PsPathClass::Blocked { resolved, reason } => PsPathOutcome::AskReason { resolved, reason },
        PsPathClass::Proceed { normalized } => {
            let resolved = crate::filesystem::expand_path(&normalized, roots);
            let mut work_dirs = Vec::with_capacity(1 + additional.len());
            work_dirs.push(roots.cwd.clone());
            work_dirs.extend(additional.iter().cloned());
            if crate::filesystem::path_in_allowed_working_path(&resolved, &work_dirs, roots) {
                PsPathOutcome::Allowed {
                    resolved: resolved.to_string_lossy().into_owned(),
                }
            } else {
                PsPathOutcome::AskContainment {
                    resolved: resolved.to_string_lossy().into_owned(),
                }
            }
        }
    }
}

/// A protected-system-path predicate for `Remove-Item` targets (claude-code
/// `TKt`): the `*`/`/*` globs, filesystem root, the home dir, ANY top-level
/// `/X` directory, and (on Windows) a drive root/child. `home` is the home dir
/// string; `is_macos` enables the `/private/(etc|var|tmp|home)` normalization.
fn is_protected_removal_resolved(path: &str, home: Option<&str>, is_macos: bool) -> bool {
    // collapse runs of separators to a single '/'
    let mut t = String::with_capacity(path.len());
    let mut prev_sep = false;
    for c in path.chars() {
        let sep = c == '/' || c == '\\';
        if sep {
            if !prev_sep {
                t.push('/');
            }
        } else {
            t.push(c);
        }
        prev_sep = sep;
    }
    if t == "*" || t.ends_with("/*") {
        return true;
    }
    let normalize_private = |c: &str| -> String {
        if !is_macos {
            return c.to_string();
        }
        // /private/(etc|var|tmp|home)(/|$) → /$1$2
        for base in ["etc", "var", "tmp", "home"] {
            let pfx = format!("/private/{base}");
            if c == pfx {
                return format!("/{base}");
            }
            if let Some(rest) = c.strip_prefix(&format!("{pfx}/")) {
                return format!("/{base}/{rest}");
            }
        }
        c.to_string()
    };
    let o = normalize_private(&t);
    let i = if o == "/" { o.clone() } else { o.trim_end_matches('/').to_string() };
    if i == "/" {
        return true;
    }
    // XOy: ^[A-Za-z]:/?$  (Windows drive root)
    if is_drive_root(&i) {
        return true;
    }
    if let Some(h) = home {
        let mut hs = String::with_capacity(h.len());
        let mut ps = false;
        for c in h.chars() {
            let sep = c == '/' || c == '\\';
            if sep {
                if !ps {
                    hs.push('/');
                }
            } else {
                hs.push(c);
            }
            ps = sep;
        }
        let hs = normalize_private(&hs);
        let hs = hs.trim_end_matches('/');
        if casefold_path(&i) == casefold_path(hs) {
            return true;
        }
    }
    // dirname(i) === "/"  →  any single top-level dir (/etc, /foo, …)
    if i.starts_with('/') && i.len() > 1 && !i[1..].contains('/') {
        return true;
    }
    // QOy: ^[A-Za-z]:/[^/]+$  (Windows drive child)
    is_drive_child(&i)
}

/// `XOy = /^[A-Za-z]:\/?$/` — a Windows drive root (`C:` / `C:/`).
fn is_drive_root(i: &str) -> bool {
    let b = i.as_bytes();
    (b.len() == 2 && b[0].is_ascii_alphabetic() && b[1] == b':')
        || (b.len() == 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'/')
}

/// `QOy = /^[A-Za-z]:\/[^/]+$/` — a Windows drive child (`C:/Users`).
fn is_drive_child(i: &str) -> bool {
    let b = i.as_bytes();
    b.len() > 3
        && b[0].is_ascii_alphabetic()
        && b[1] == b':'
        && b[2] == b'/'
        && !i[3..].contains('/')
}

/// The raw-path protected check (claude-code `ZBr`): strip quotes, drop a `::`
/// prefix, expand `~`, normalize backslashes, then [`is_protected_removal_resolved`].
fn is_protected_removal_raw(path: &str, home: Option<&str>, is_macos: bool) -> bool {
    let mut t = strip_surrounding_quotes(path).to_string();
    if let Some(idx) = t.find("::") {
        t = t[idx + 2..].to_string();
    }
    let t = expand_tilde(&t, home).replace('\\', "/");
    is_protected_removal_resolved(&t, home, is_macos)
}

/// Whether an argument is a `-Recurse` parameter (claude-code's inline test in
/// `xgg`): the dash-normalized, lowercased, colon-stripped token is a ≥2-char
/// prefix of `-recurse`.
fn is_recurse_flag(arg: &str) -> bool {
    if arg.is_empty() {
        return false;
    }
    let first_len = arg.chars().next().map_or(0, char::len_utf8);
    let normalized = format!("-{}", &arg[first_len..]).to_lowercase();
    let w = match normalized.find(':') {
        Some(i) => &normalized[..i],
        None => &normalized[..],
    };
    w.len() >= 2 && "-recurse".starts_with(w)
}

/// The result of PowerShell path containment (claude-code `Z_u`/`xgg` return).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PsContainmentResult {
    /// No constraint fired — proceed to the normal permission flow.
    Passthrough,
    /// A path could not be validated — prompt the user (the first ask wins).
    Ask {
        /// The model-/user-facing ask message.
        message: String,
        /// The decision reason (equals `message` for the branches with no distinct reason).
        reason: String,
    },
    /// A `Remove-Item` targets a protected system path — hard deny (`Hwt`).
    Deny {
        /// The deny message.
        message: String,
        /// The decision reason.
        reason: String,
    },
}

/// Allowed-working-directory string list for containment messages (cwd + the
/// additional dirs, deduped) — the port analog of `allWorkingDirectories`/`vY`.
fn ps_working_dir_list(
    roots: &crate::filesystem::FsRoots,
    additional: &[std::path::PathBuf],
) -> Vec<String> {
    let mut out = vec![roots.cwd.to_string_lossy().into_owned()];
    for d in additional {
        let s = d.to_string_lossy().into_owned();
        if !out.contains(&s) {
            out.push(s);
        }
    }
    out
}

/// Context for [`validate_ps_statements`] — the roots + extra dirs + platform.
pub struct PsCtx<'a> {
    /// Filesystem roots (cwd/home) for path resolution + containment.
    pub roots: &'a crate::filesystem::FsRoots,
    /// The additional allowed working directories.
    pub additional: &'a [std::path::PathBuf],
    /// Windows guard variants (drive-relative, provider min length).
    pub is_windows: bool,
    /// macOS `/private` normalization for the removal-protected check.
    pub is_macos: bool,
}

/// Validate a parsed PowerShell command (claude-code `Z_u`): run [`xgg`-style]
/// [`validate_ps_statement`] over every statement, returning the first `deny`
/// immediately and otherwise the first `ask`, else `passthrough`.
///
/// `compound_cd` mirrors claude-code's compound-`cd` flag: a compound command
/// that changes the working directory makes relative paths unvalidatable.
#[must_use]
pub fn validate_ps_statements(
    statements: &[PsStatement],
    ctx: &PsCtx,
    compound_cd: bool,
) -> PsContainmentResult {
    let mut first_ask: Option<PsContainmentResult> = None;
    for stmt in statements {
        match validate_ps_statement(stmt, ctx, compound_cd) {
            deny @ PsContainmentResult::Deny { .. } => return deny,
            ask @ PsContainmentResult::Ask { .. } => {
                if first_ask.is_none() {
                    first_ask = Some(ask);
                }
            }
            PsContainmentResult::Passthrough => {}
        }
    }
    first_ask.unwrap_or(PsContainmentResult::Passthrough)
}

/// Set the first-ask accumulator if empty (claude-code `o ??= …`).
fn set_first_ask(ask: &mut Option<PsContainmentResult>, message: String) {
    if ask.is_none() {
        *ask = Some(PsContainmentResult::Ask {
            reason: message.clone(),
            message,
        });
    }
}

/// Validate a single PowerShell statement/pipeline (claude-code `xgg`).
#[must_use]
pub fn validate_ps_statement(
    stmt: &PsStatement,
    ctx: &PsCtx,
    compound_cd: bool,
) -> PsContainmentResult {
    let dirs = ps_working_dir_list(ctx.roots, ctx.additional);
    let mut ask: Option<PsContainmentResult> = None;

    if compound_cd {
        set_first_ask(&mut ask, "Compound command changes working directory (Set-Location/Push-Location/Pop-Location/New-PSDrive) \u{2014} relative paths cannot be validated against the original cwd and require manual approval".to_string());
    }

    // Main pipeline elements.
    let mut pipeline_source = false;
    let mut non_readonly_seen = false;
    for element in &stmt.commands {
        let l = match element {
            PsElement::Expression { .. } => {
                pipeline_source = true;
                continue;
            }
            PsElement::Command(c) => c,
        };
        let prev = non_readonly_seen;
        if !RGG.contains(normalize_cmdlet(&l.name).as_str()) {
            non_readonly_seen = true;
        }
        if let Some(deny) = run_ps_command(l, ctx, &dirs, pipeline_source, prev, &mut ask) {
            return deny;
        }
    }

    // Nested commands (script blocks / control flow).
    for l in &stmt.nested_commands {
        if let Some(deny) = run_ps_command(l, ctx, &dirs, false, false, &mut ask) {
            return deny;
        }
    }

    // Redirections (nested + statement-level) → "create" containment.
    for l in &stmt.nested_commands {
        if let Some(deny) = check_redirections(&l.redirections, ctx, &dirs, &mut ask) {
            return deny;
        }
    }
    if let Some(deny) = check_redirections(&stmt.redirections, ctx, &dirs, &mut ask) {
        return deny;
    }

    ask.unwrap_or(PsContainmentResult::Passthrough)
}

/// Run one command's path checks (claude-code `xgg`'s per-command body). Returns
/// `Some(deny)` on a `Remove-Item` protected-path hit; otherwise updates the
/// first-ask accumulator.
fn run_ps_command(
    l: &PsCommand,
    ctx: &PsCtx,
    dirs: &[String],
    pipeline_source: bool,
    prev_non_readonly: bool,
    ask: &mut Option<PsContainmentResult>,
) -> Option<PsContainmentResult> {
    let roots = ctx.roots;
    let home = roots.home.as_deref().map(|p| p.to_string_lossy().into_owned());
    let extraction = extract_paths(l);
    let f = normalize_cmdlet(&l.name);
    let is_path_cmdlet = FKN.contains_key(f.as_str());

    if pipeline_source {
        set_first_ask(ask, format!(
            "{f} receives its path from a pipeline expression source that cannot be statically validated and requires manual approval"
        ));
    }
    if extraction.has_unvalidatable_path_arg {
        set_first_ask(ask, format!(
            "{f} uses a parameter or complex path expression (array literal, subexpression, unknown parameter, etc.) that cannot be statically validated and requires manual approval"
        ));
    }
    if extraction.operation_type != PsOperation::Read
        && !extraction.optional_write
        && extraction.paths.is_empty()
        && is_path_cmdlet
    {
        set_first_ask(ask, format!(
            "{f} is a write operation but no target path could be determined; requires manual approval"
        ));
        return None;
    }
    if prev_non_readonly && is_path_cmdlet {
        set_first_ask(ask, format!(
            "{f} may receive a path from an upstream pipeline command whose output cannot be statically validated and requires manual approval"
        ));
    }

    let is_remove = f == "remove-item";
    if is_remove && l.args.iter().any(|a| is_recurse_flag(a)) {
        let cwd_fold = casefold_path(&roots.cwd.to_string_lossy());
        for b in &extraction.paths {
            let v = expand_tilde(&short_name_expand(b, ctx.is_windows), home.as_deref())
                .replace('\\', "/");
            let resolved = crate::filesystem::expand_path(&v, roots);
            let x = casefold_path(&resolved.to_string_lossy());
            if x == cwd_fold
                || cwd_fold.starts_with(&format!("{x}/"))
                || cwd_fold.starts_with(&format!("{x}\\"))
            {
                set_first_ask(ask, format!(
                    "Remove-Item -Recurse targeting '{b}' would delete the working directory including .git and .claude \u{2014} requires manual approval"
                ));
                break;
            }
        }
    }

    for path in &extraction.paths {
        if is_remove && is_protected_removal_raw(path, home.as_deref(), ctx.is_macos) {
            return Some(deny_removal(path));
        }
        let outcome =
            check_ps_path(path, extraction.operation_type, roots, ctx.additional, ctx.is_windows);
        let resolved = match &outcome {
            PsPathOutcome::Allowed { resolved }
            | PsPathOutcome::AskReason { resolved, .. }
            | PsPathOutcome::AskContainment { resolved } => resolved.clone(),
        };
        if is_remove && is_protected_removal_resolved(&resolved, home.as_deref(), ctx.is_macos) {
            return Some(deny_removal(&resolved));
        }
        match outcome {
            PsPathOutcome::Allowed { .. } => {}
            PsPathOutcome::AskReason { reason, .. } => set_first_ask(ask, reason),
            PsPathOutcome::AskContainment { resolved } => {
                set_first_ask(ask, cmdlet_containment_message(&f, &resolved, dirs));
            }
        }
    }
    None
}

/// `Hwt` — a `Remove-Item` protected-path hard deny.
fn deny_removal(path: &str) -> PsContainmentResult {
    PsContainmentResult::Deny {
        message: remove_item_protected_message(path),
        reason: "Removal targets a protected system path".to_string(),
    }
}

/// Validate a command's/statement's output redirections against the working dirs
/// (claude-code: each `create`-op redirection target → the "Output redirection
/// to '…' … write to files in …" message). Returns `Some(deny)` on a rule/deny,
/// else updates the first-ask accumulator.
fn check_redirections(
    redirs: &[PsRedirection],
    ctx: &PsCtx,
    dirs: &[String],
    ask: &mut Option<PsContainmentResult>,
) -> Option<PsContainmentResult> {
    for r in redirs {
        if r.is_merging || r.target.is_empty() {
            continue;
        }
        match check_ps_path(&r.target, PsOperation::Create, ctx.roots, ctx.additional, ctx.is_windows) {
            PsPathOutcome::Allowed { .. } => {}
            PsPathOutcome::AskReason { reason, .. } => {
                if ask.is_none() {
                    *ask = Some(PsContainmentResult::Ask {
                        message: reason.clone(),
                        reason,
                    });
                }
            }
            PsPathOutcome::AskContainment { resolved } => {
                if ask.is_none() {
                    let message = redirection_containment_message(&resolved, dirs);
                    *ask = Some(PsContainmentResult::Ask {
                        reason: message.clone(),
                        message,
                    });
                }
            }
        }
    }
    None
}

#[cfg(test)]
#[path = "powershell_containment_test.rs"]
mod powershell_containment_test;
