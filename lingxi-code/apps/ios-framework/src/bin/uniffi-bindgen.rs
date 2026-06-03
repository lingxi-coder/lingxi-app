//! M10-P1b — the offline `UniFFI` bindings generator binary (Swift, library mode).
//!
//! This is the standalone `uniffi-bindgen` the iOS build script invokes in
//! `--library` mode to emit the Swift bindings (`.swift` + `*FFI.h` header +
//! `*FFI.modulemap`) from a host `cdylib` build of this crate:
//!
//! ```sh
//! cargo build -p ios-framework --features uniffi            # host cdylib
//! cargo run  -p ios-framework --features cli --bin uniffi-bindgen -- \
//!     generate --library <target/debug/libios_framework.dylib> \
//!     --language swift --out-dir <gen>
//! ```
//!
//! ## Why this is NOT just `uniffi::uniffi_bindgen_main()`
//!
//! The stock `uniffi-bindgen 0.28.3` `--library` pipeline PANICS on our surface
//! (`interface/mod.rs:1126` — `unknown throw type`). Root cause: the shared
//! mobile-host async export `MobileEngineHandle::submit(ClientCommand) ->
//! Result<(), client_protocol::ClientError>` (defined ONCE in `engine-mobile`,
//! re-exported here + by `android-aar`) throws an error type that lives in a
//! DIFFERENT crate (`client-protocol`). In `--library` mode `UniFFI` builds one
//! `ComponentInterface` per crate-namespace, so a cross-crate throw type is
//! recorded as `Type::External { kind: DataClass, .. }`. The 0.28.3
//! `throws_name()` helper only matches `Type::Enum` / `Type::Object` and
//! `panic!`s on `Type::External` — a known bindgen limitation for cross-crate
//! `Result` error types. `UniFFI` 0.28.3 is the offline-pinned version (F3-00); no
//! newer bindgen is available offline, and no `--metadata-no-deps` / config
//! switch avoids the panic (the throw type is in the dylib metadata, not config).
//!
//! ## The fix — wrap at the FFI boundary, in the bindgen only
//!
//! This is a packaging-only fix: we touch NO engine semantics and NO
//! `engine-mobile` / `android-aar` / shared-crate source. We reimplement the
//! `generate --library --language swift` path over `UniFFI`'s PUBLIC APIs and
//! insert ONE surgical step — after `group_metadata` has resolved cross-crate
//! references to `Type::External`, we rewrite the THROWS position of every
//! function/method/constructor whose error is an external data-class
//! (`Type::External { kind: DataClass, .. }`) back into a `Type::Enum`
//! reference (same `module_path` + `name`). That is exactly the shape
//! `throws_name()` accepts, so the panic is avoided. The error enum itself is
//! still DEFINED once, in its owning crate's generated module
//! (`client_protocol.swift`), and the throwing method just references it by name
//! — Swift bindings put every generated namespace in one importer scope, so the
//! reference resolves. The byte-level engine behavior, the `submit` routing, and
//! the wire DTOs are all unchanged; only the bindgen's view of the throw type's
//! `Type` tag is adjusted so 0.28.3 can render it.
//!
//! We ship no per-crate `uniffi.toml`, so each component is built with the
//! default (empty) TOML config and the bindgen never shells out to `cargo
//! metadata` — sidestepping the pinned-cargo edition-2024 manifest-parse failure
//! that the old `--metadata-no-deps` flag was previously papering over.

#[cfg(not(feature = "cli"))]
fn main() {
    eprintln!(
        "uniffi-bindgen requires the `cli` feature; \
         build with `--features cli` (see crate docs)."
    );
    std::process::exit(1);
}

#[cfg(feature = "cli")]
fn main() {
    if let Err(e) = cli::run() {
        eprintln!("uniffi-bindgen error: {e:?}");
        std::process::exit(1);
    }
}

#[cfg(feature = "cli")]
mod cli {
    use anyhow::{bail, Context, Result};
    use camino::{Utf8Path, Utf8PathBuf};

    use uniffi_bindgen::bindings::SwiftBindingGenerator;
    use uniffi_bindgen::interface::ComponentInterface;
    use uniffi_bindgen::macro_metadata;
    use uniffi_bindgen::{BindingGenerator, Component, GenerationSettings};
    use uniffi_meta::{
        create_metadata_groups, group_metadata, ExternalKind, Metadata, MetadataGroup, Type,
    };

    /// Minimal arg model — we only support the one invocation the iOS build
    /// script uses: `generate --library <path> --language swift --out-dir <dir>`
    /// with optional `--no-format` / `--crate <name>`. (`--metadata-no-deps` is
    /// accepted-and-ignored for backward compatibility with the prior script,
    /// since we never run `cargo metadata` anymore.)
    struct Args {
        library_path: Utf8PathBuf,
        out_dir: Utf8PathBuf,
        no_format: bool,
        crate_name: Option<String>,
    }

    pub fn run() -> Result<()> {
        let args = parse_args()?;
        generate_swift(&args)
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

        while let Some(arg) = raw.next() {
            match arg.as_str() {
                "--library" => {
                    library_path =
                        Some(Utf8PathBuf::from(raw.next().context("--library needs a value")?));
                }
                "--out-dir" | "-o" => {
                    out_dir =
                        Some(Utf8PathBuf::from(raw.next().context("--out-dir needs a value")?));
                }
                "--language" | "-l" => {
                    languages.push(raw.next().context("--language needs a value")?);
                }
                "--crate" => {
                    crate_name = Some(raw.next().context("--crate needs a value")?);
                }
                "--no-format" | "-n" => no_format = true,
                // Accepted-and-ignored: we never invoke `cargo metadata`.
                "--metadata-no-deps" => {}
                other => bail!("unsupported argument: {other}"),
            }
        }

        // Swift is the only backend this packager emits.
        for lang in &languages {
            if !lang.eq_ignore_ascii_case("swift") {
                bail!("only `--language swift` is supported here; got {lang:?}");
            }
        }

        Ok(Args {
            library_path: library_path.context("--library <path> is required")?,
            out_dir: out_dir.context("--out-dir <dir> is required")?,
            no_format,
            crate_name,
        })
    }

    fn generate_swift(args: &Args) -> Result<()> {
        let components = find_components_patched(&args.library_path)?;

        let mut components: Vec<Component<<SwiftBindingGenerator as BindingGenerator>::Config>> =
            components
                .into_iter()
                .map(|Component { ci, config }| {
                    let config = SwiftBindingGenerator.new_config(&config.into())?;
                    Ok(Component { ci, config })
                })
                .collect::<Result<Vec<_>>>()?;

        // Optionally narrow to one crate (mirrors stock `--crate`).
        if let Some(crate_name) = &args.crate_name {
            let kept: Vec<_> = components
                .drain(..)
                .filter(|c| c.ci.crate_name() == crate_name)
                .collect();
            match kept.len() {
                0 => bail!("crate {crate_name} not found in {}", args.library_path),
                _ => components = kept,
            }
        }

        let settings = GenerationSettings {
            out_dir: args.out_dir.clone(),
            try_format_code: !args.no_format,
            cdylib: uniffi_bindgen::library_mode::calc_cdylib_name(&args.library_path)
                .map(ToOwned::to_owned),
        };

        SwiftBindingGenerator.update_component_configs(&settings, &mut components)?;
        std::fs::create_dir_all(&args.out_dir)?;
        SwiftBindingGenerator.write_bindings(&settings, &components)?;
        Ok(())
    }

    /// Faithful re-implementation of `uniffi_bindgen::library_mode::find_components`
    /// (we ship NO UDL files, so the UDL branch is omitted), with ONE added pass:
    /// [`patch_external_throws`] on each grouped namespace BEFORE building its
    /// `ComponentInterface`. Returns components with the default (empty) TOML
    /// config — we ship no `uniffi.toml`, so an empty config is exact.
    fn find_components_patched(
        library_path: &Utf8Path,
    ) -> Result<Vec<Component<toml::value::Table>>> {
        let items = macro_metadata::extract_from_library(library_path)
            .context("extracting UniFFI metadata from the host library")?;

        let mut metadata_groups = create_metadata_groups(&items);
        // `group_metadata` runs `fixup_external_type`, which is what turns the
        // cross-crate `ClientError` throw type into `Type::External`.
        group_metadata(&mut metadata_groups, items)?;

        metadata_groups
            .into_values()
            .map(|mut group| {
                patch_external_throws(&mut group);
                let ci = ComponentInterface::from_metadata(group)?;
                Ok(Component {
                    ci,
                    config: toml::value::Table::default(),
                })
            })
            .collect()
    }

    /// The surgical fix. Walks a grouped namespace's items and rewrites the
    /// THROWS position of every fn/method/constructor whose error type is an
    /// external data-class (`Type::External { kind: DataClass, .. }`) into a
    /// `Type::Enum { module_path, name }` reference. This is the exact shape the
    /// 0.28.3 `throws_name()` helper accepts; the error enum stays DEFINED in its
    /// owning crate's generated Swift module and is merely referenced by name
    /// from the throwing namespace.
    fn patch_external_throws(group: &mut MetadataGroup) {
        // `items` is a `BTreeSet<Metadata>`; drain, patch, re-insert.
        let items = std::mem::take(&mut group.items);
        for item in items {
            group.items.insert(patch_item(item));
        }
    }

    fn patch_item(item: Metadata) -> Metadata {
        match item {
            Metadata::Func(mut m) => {
                m.throws = m.throws.map(localize_external_error);
                Metadata::Func(m)
            }
            Metadata::Method(mut m) => {
                m.throws = m.throws.map(localize_external_error);
                Metadata::Method(m)
            }
            Metadata::TraitMethod(mut m) => {
                m.throws = m.throws.map(localize_external_error);
                Metadata::TraitMethod(m)
            }
            Metadata::Constructor(mut m) => {
                m.throws = m.throws.map(localize_external_error);
                Metadata::Constructor(m)
            }
            other => other,
        }
    }

    /// `Type::External { kind: DataClass, .. }` (a cross-crate enum/record used
    /// as an error) → `Type::Enum { module_path, name }`. Every other throw type
    /// is left untouched (a same-crate `Type::Enum`/`Type::Object` already works;
    /// an external INTERFACE error would surface as `kind: Interface` and is not
    /// rewritten — we have none).
    fn localize_external_error(ty: Type) -> Type {
        match ty {
            Type::External {
                module_path,
                name,
                kind: ExternalKind::DataClass,
                ..
            } => Type::Enum { module_path, name },
            other => other,
        }
    }
}
