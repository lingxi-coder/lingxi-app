use super::{normalise_preamble, reorder_help_sections};

/// clap renders subcommand help description-first with Commands before
/// Options; commander renders Usage-first with
/// Arguments -> Options -> Commands.
const CLAP_STYLE: &str = "\
Configure and manage MCP servers

Usage: lingxi-cli mcp [COMMAND]

Commands:
  add   Add a server
  list  List servers

Options:
  -h, --help  Print help
";

#[test]
fn usage_moves_ahead_of_the_description() {
    let out = normalise_preamble(CLAP_STYLE);
    let first = out.lines().next().unwrap();
    assert!(first.starts_with("Usage:"), "{out}");
    assert!(out.contains("Configure and manage MCP servers"), "{out}");
}

#[test]
fn a_wrapped_usage_block_moves_whole() {
    // Taking only the first line would strip the wrapped tail onto the
    // wrong side of the description.
    let input =
        "Some description\n\nUsage: cli foo [OPTIONS]\n           [EXTRA]\n\nOptions:\n  -h\n";
    let out = normalise_preamble(input);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines[0], "Usage: cli foo [OPTIONS]");
    assert_eq!(lines[1], "           [EXTRA]");
}

#[test]
fn sections_are_reordered_to_commander_order() {
    let out = reorder_help_sections(CLAP_STYLE);
    let opts = out.find("Options:").expect("options");
    let cmds = out.find("Commands:").expect("commands");
    assert!(opts < cmds, "Options must precede Commands:\n{out}");
}

#[test]
fn arguments_precede_options() {
    let input = "Usage: cli x\n\nOptions:\n  -h\n\nArguments:\n  [target]  A target\n";
    let out = reorder_help_sections(input);
    let args = out.find("Arguments:").expect("arguments");
    let opts = out.find("Options:").expect("options");
    assert!(args < opts, "Arguments must precede Options:\n{out}");
}

#[test]
fn a_command_without_positionals_gets_no_arguments_header() {
    // The reason this is a text transform and not a help_template: a
    // template spelling the sections out would print a bare `Arguments:`
    // for the ~40 subcommands that have none.
    let out = reorder_help_sections(CLAP_STYLE);
    assert!(!out.contains("Arguments:"), "{out}");
}

#[test]
fn exactly_one_blank_line_precedes_each_header() {
    let out = reorder_help_sections(CLAP_STYLE);
    for header in ["Options:", "Commands:"] {
        let at = out.find(header).expect(header);
        assert!(out[..at].ends_with("\n\n"), "{header} spacing:\n{out:?}");
        assert!(!out[..at].ends_with("\n\n\n"), "{header} doubled:\n{out:?}");
    }
}

#[test]
fn an_unrecognised_section_is_not_dropped() {
    let input = "Usage: cli x\n\nOptions:\n  -h\n\nExamples:\n  cli x --yes\n";
    let out = reorder_help_sections(input);
    assert!(out.contains("Examples:"), "{out}");
    assert!(out.contains("cli x --yes"), "{out}");
}

#[test]
fn help_without_any_section_is_returned_unchanged() {
    let input = "Usage: cli x\n\njust a description\n";
    assert_eq!(reorder_help_sections(input), input);
}
