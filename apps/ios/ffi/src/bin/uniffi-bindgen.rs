//! Native bindings generator using the pinned UniFFI 0.32 public pipeline.
//!
//! Swift keeps one module scope; Kotlin uses each component's configured package.
//! Cross-component errors and Rust buffers use UniFFI's native converters.

#[cfg(feature = "cli")]
fn main() {
    if let Err(error) = cli::run() {
        eprintln!("uniffi-bindgen error: {error:?}");
        std::process::exit(1);
    }
}

#[cfg(not(feature = "cli"))]
fn main() {
    eprintln!("uniffi-bindgen requires --features cli");
    std::process::exit(1);
}

#[cfg(feature = "cli")]
mod cli {
    use anyhow::{bail, Context, Result};
    use camino::Utf8PathBuf;
    use uniffi_bindgen::bindings::{generate, GenerateOptions, TargetLanguage};

    enum Language {
        Swift,
        Kotlin,
    }
    struct Args {
        library_path: Utf8PathBuf,
        out_dir: Utf8PathBuf,
        no_format: bool,
        crate_name: Option<String>,
        language: Language,
        config_path: Option<Utf8PathBuf>,
    }

    pub fn run() -> Result<()> {
        let args = parse_args()?;
        let kotlin = matches!(args.language, Language::Kotlin);
        let out_dir = args.out_dir.clone();
        generate(GenerateOptions {
            languages: vec![match args.language {
                Language::Swift => TargetLanguage::Swift,
                Language::Kotlin => TargetLanguage::Kotlin,
            }],
            source: args.library_path,
            out_dir: args.out_dir,
            config_override: args.config_path,
            format: !args.no_format,
            crate_filter: args.crate_name,
            metadata_no_deps: true,
        })?;
        if kotlin {
            repair_kotlin_errors(out_dir.as_std_path())?;
        }
        Ok(())
    }

    // Match the adjacent error getter so ordinary message records are preserved.
    fn repair_kotlin_errors(directory: &std::path::Path) -> Result<()> {
        for entry in std::fs::read_dir(directory)? {
            let path = entry?.path();
            if path.is_dir() {
                repair_kotlin_errors(&path)?;
            } else if path.extension().is_some_and(|ext| ext == "kt") {
                let text = std::fs::read_to_string(&path)?;
                let repaired = repair_callback_buffer(repair_message_properties(&text));
                if repaired != text {
                    std::fs::write(path, repaired)?;
                }
            }
        }
        Ok(())
    }

    // The cross-component async callback returns a raw C RustBuffer. Rewrap
    // its fields in this component's JNA struct; ownership stays with Rust.
    fn repair_callback_buffer(text: String) -> String {
        text.replace(
            "UniffiForeignFutureResultRustBuffer.UniffiByValue(\n                    FfiConverterTypeAudioOperationResultDto.lower(returnValue),",
            "UniffiForeignFutureResultRustBuffer.UniffiByValue(\n                    FfiConverterTypeAudioOperationResultDto.lower(returnValue).let { buffer ->\n                        RustBuffer.ByValue().apply {\n                            capacity = buffer.capacity\n                            len = buffer.len\n                            data = buffer.data\n                        }\n                    },",
        )
    }

    fn repair_message_properties(text: &str) -> String {
        const PROP: &str = "        val `message`: kotlin.String\n        ) : ";
        const FIXED: &str = "        override val `message`: kotlin.String\n        ) : ";
        const GETTER: &str =
            "        override val message\n            get() = \"message=${ `message` }\"\n";
        let mut result = String::new();
        let mut cursor = 0;
        while let Some(offset) = text[cursor..].find(GETTER) {
            let getter = cursor + offset;
            let end = getter + GETTER.len();
            let property = text[..getter].rfind(PROP).filter(|start| *start >= cursor);
            let property = property.filter(|start| {
                let declaration = text[*start + PROP.len()..getter].trim_end();
                declaration.strip_suffix("() {").is_some_and(|name| {
                    !name.is_empty()
                        && name
                            .chars()
                            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '.')
                })
            });
            if let Some(start) = property {
                result.push_str(&text[cursor..start]);
                result.push_str(FIXED);
                result.push_str(&text[start + PROP.len()..getter]);
            } else {
                result.push_str(&text[cursor..end]);
            }
            cursor = end;
        }
        result.push_str(&text[cursor..]);
        result
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn audio_callback_repair_preserves_buffer_fields_and_other_results() {
            let audio = "UniffiForeignFutureResultRustBuffer.UniffiByValue(\n                    FfiConverterTypeAudioOperationResultDto.lower(returnValue),";
            let other = "UniffiForeignFutureResultRustBuffer.UniffiByValue(\n                    FfiConverterTypeOtherResultDto.lower(returnValue),";
            let repaired = super::repair_callback_buffer(format!("{audio}\n{other}"));
            assert!(repaired.ends_with(other));
            assert_eq!(repaired.matches("RustBuffer.ByValue().apply").count(), 1);
            for field in ["capacity", "len", "data"] {
                assert!(repaired.contains(&format!("{field} = buffer.{field}")));
            }
            assert_eq!(super::repair_callback_buffer(repaired.clone()), repaired);
        }

        #[test]
        fn error_message_repair_preserves_records_and_raw_field() {
            let record =
                "        val `message`: kotlin.String\n        ) : ClientEvent() {\n    }\n";
            let error = "        val `message`: kotlin.String\n        ) : ClientException() {\n        override val message\n            get() = \"message=${ `message` }\"\n    }\n";
            let repaired = super::repair_message_properties(&format!("{record}{error}"));
            assert!(repaired.starts_with(record));
            assert_eq!(repaired.matches("override val `message`").count(), 1);
            assert!(!repaired.contains("get() ="));
            assert_eq!(super::repair_message_properties(&repaired), repaired);
        }
    }

    fn parse_args() -> Result<Args> {
        let mut raw = std::env::args().skip(1);

        match raw.next().as_deref() {
            Some("generate") => {}
            other => bail!(
                "this uniffi-bindgen only supports `generate --library … --language swift`; \
                 got subcommand {other:?}"
            ),
        }

        let mut library_path: Option<Utf8PathBuf> = None;
        let mut out_dir: Option<Utf8PathBuf> = None;
        let mut languages: Vec<String> = Vec::new();
        let mut no_format = false;
        let mut crate_name: Option<String> = None;
        let mut config_path: Option<Utf8PathBuf> = None;

        while let Some(arg) = raw.next() {
            match arg.as_str() {
                "--library" => {
                    library_path = Some(Utf8PathBuf::from(
                        raw.next().context("--library needs a value")?,
                    ));
                }
                "--out-dir" | "-o" => {
                    out_dir = Some(Utf8PathBuf::from(
                        raw.next().context("--out-dir needs a value")?,
                    ));
                }
                "--language" | "-l" => {
                    languages.push(raw.next().context("--language needs a value")?);
                }
                "--crate" => {
                    crate_name = Some(raw.next().context("--crate needs a value")?);
                }
                "--config" => {
                    config_path = Some(Utf8PathBuf::from(
                        raw.next().context("--config needs a value")?,
                    ));
                }
                "--no-format" | "-n" => no_format = true,
                // Accepted-and-ignored: we never invoke `cargo metadata`.
                "--metadata-no-deps" => {}
                other => bail!("unsupported argument: {other}"),
            }
        }

        // Exactly one backend per invocation: swift (default) or kotlin (T2.4).
        let language = match languages.as_slice() {
            [] => Language::Swift,
            [one] if one.eq_ignore_ascii_case("swift") => Language::Swift,
            [one] if one.eq_ignore_ascii_case("kotlin") => Language::Kotlin,
            [one] => bail!("only `--language swift|kotlin` is supported here; got {one:?}"),
            _ => bail!("only a single `--language` is supported per invocation"),
        };

        Ok(Args {
            library_path: library_path.context("--library <path> is required")?,
            out_dir: out_dir.context("--out-dir <dir> is required")?,
            no_format,
            crate_name,
            language,
            config_path,
        })
    }
}
