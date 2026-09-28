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
//! Result<(), client::protocol::ClientError>` (defined ONCE in `engine-mobile`,
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

    use uniffi_bindgen::bindings::{KotlinBindingGenerator, SwiftBindingGenerator};
    use uniffi_bindgen::interface::ComponentInterface;
    use uniffi_bindgen::macro_metadata;
    use uniffi_bindgen::{BindingGenerator, Component, GenerationSettings};
    use uniffi_meta::{
        create_metadata_groups, group_metadata, ExternalKind, Metadata, MetadataGroup, Type,
    };

    /// The binding backend this invocation targets. Swift is the original iOS
    /// path; Kotlin (T2.4) reuses the SAME external-throws patch + `--library`
    /// pipeline so the Android `.so`'s `MobileEngineHandle::submit` cross-crate
    /// `ClientError` throw renders under the offline-pinned 0.28.3 bindgen too.
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Language {
        Swift,
        Kotlin,
    }

    /// Minimal arg model. Original iOS invocation:
    /// `generate --library <path> --language swift --out-dir <dir>`.
    /// T2.4 adds `--language kotlin` (+ optional `--config <uniffi.toml>` so the
    /// Kotlin backend honors `package_name`). `--no-format` / `--crate <name>`
    /// are unchanged; `--metadata-no-deps` is accepted-and-ignored (we never run
    /// `cargo metadata`).
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
        match args.language {
            Language::Swift => generate(&args, SwiftBindingGenerator),
            Language::Kotlin => generate(&args, KotlinBindingGenerator),
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

    /// Generic over the binding backend (`SwiftBindingGenerator` /
    /// `KotlinBindingGenerator`) — both share the SAME external-throws-patched
    /// `--library` component discovery. Swift ships no `uniffi.toml` (empty
    /// config), Kotlin loads `--config <uniffi.toml>` for `package_name`.
    fn generate<G>(args: &Args, generator: G) -> Result<()>
    where
        G: BindingGenerator,
    {
        let groups = find_patched_groups(&args.library_path)?;

        // T3.x fix — for Kotlin, MERGE the multi-crate metadata into ONE
        // `ComponentInterface` so the backend emits ONE file with ONE runtime.
        //
        // In `--library` mode the merged `libandroid_aar.so` carries the UniFFI
        // metadata of FOUR crates (client-protocol, client-adapter, engine-mobile,
        // android-aar), so discovery yields four groups. The Kotlin backend emits
        // ONE self-contained `.kt` file PER component, EACH re-declaring the
        // shared runtime (`RustBuffer`, `UniffiLib`,
        // `UniffiRustCallStatusErrorHandler`, every `FfiConverter*`, …). Four such
        // files in one package collide (`Redeclaration`); four files in FOUR
        // packages instead cannot share a runtime — e.g. `engine-mobile`'s
        // `submit` throws `client-protocol::ClientError`, so its call hands
        // `client_protocol.ClientException.ErrorHandler` (a
        // `client_protocol.UniffiRustCallStatusErrorHandler`) to
        // `engine_mobile.uniffiRustCallAsync` (which wants an
        // `engine_mobile.UniffiRustCallStatusErrorHandler`), and the
        // `RustBuffer`/`FfiConverter` types in that handler's signature are
        // themselves package-private duplicates — the runtime can't be deduped
        // type-by-type without cascading across the whole helper surface.
        //
        // The clean fix is the model that already works for Swift (one scope, one
        // file, one runtime): merge the groups into a single `ComponentInterface`
        // so all cross-crate references collapse to same-file references. The
        // domain type names do NOT collide across the four crates (verified —
        // only the runtime helpers do, and those fold to one copy), so the merge
        // is sound. Swift keeps its per-group component flow UNCHANGED.
        let components_ci: Vec<ComponentInterface> = match args.language {
            Language::Kotlin => {
                // Before merging four namespaces into one `TypeUniverse`, rewrite
                // every cross-crate `Type::External` reference back to the LOCAL
                // type the defining crate uses (`Enum`/`Record`/`Object`). Without
                // this the universe sees the SAME type under two tags (e.g.
                // `ClientEvent` as `Type::Enum` in its own group but `Type::External`
                // where another crate references it) and the
                // `TypeUniverse::add_known_type` consistency assertion fails. Once
                // everything is one namespace, nothing is truly external.
                let groups = de_externalize_groups(groups);
                vec![merge_groups_into_ci(groups)?]
            }
            Language::Swift => groups
                .into_iter()
                .map(ComponentInterface::from_metadata)
                .collect::<Result<Vec<_>>>()?,
        };

        // Root TOML for `new_config`: the `--config` file if given (Kotlin's
        // `[bindings.kotlin] package_name`), else an empty table (Swift's
        // historical behavior — no per-crate `uniffi.toml`).
        let root_toml: toml::Value = match &args.config_path {
            Some(path) => {
                let text = std::fs::read_to_string(path)
                    .with_context(|| format!("reading config {path}"))?;
                toml::from_str(&text).with_context(|| format!("parsing config {path}"))?
            }
            None => toml::Value::Table(toml::value::Table::default()),
        };

        let mut components: Vec<Component<G::Config>> = components_ci
            .into_iter()
            .map(|ci| {
                let config = generator.new_config(&root_toml)?;
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

        generator.update_component_configs(&settings, &mut components)?;
        std::fs::create_dir_all(&args.out_dir)?;
        generator.write_bindings(&settings, &components)?;

        // T3.x — fix the lone `message`-field error-variant codegen defect in the
        // single merged Kotlin file (see [`fix_message_field_error_variants`]).
        if args.language == Language::Kotlin {
            for c in &components {
                let file = kotlin_file_path(&args.out_dir, c.ci.namespace());
                fix_message_field_error_variants(&file)?;
            }
        }
        Ok(())
    }

    /// Merge every discovered metadata group into a SINGLE `ComponentInterface`
    /// so the Kotlin backend emits ONE file with ONE shared runtime. Each group
    /// is re-namespaced onto a common namespace first (the `add_metadata` merge
    /// guard rejects a namespace mismatch), then folded in. The merged CI's
    /// namespace name drives the output `.kt` filename and the `uniffi_<ns>`
    /// cdylib fallback, so it is set to the real cdylib crate (`android_aar`); the
    /// actual `cdylib` name is still taken from the library path by
    /// `GenerationSettings`. The fold order is deterministic (groups sorted by
    /// crate name) so the output is stable across runs.
    fn merge_groups_into_ci(mut groups: Vec<MetadataGroup>) -> Result<ComponentInterface> {
        use uniffi_meta::NamespaceMetadata;

        let merged_ns = NamespaceMetadata {
            crate_name: "android_aar".to_string(),
            name: "android_aar".to_string(),
        };

        groups.sort_by(|a, b| a.namespace.crate_name.cmp(&b.namespace.crate_name));

        let mut merged_ci: Option<ComponentInterface> = None;
        for mut group in groups {
            group.namespace = merged_ns.clone();
            // Only the first group keeps a docstring slot; clear the rest so the
            // merge guard's docstring handling stays a no-op.
            group.namespace_docstring = None;
            match &mut merged_ci {
                None => merged_ci = Some(ComponentInterface::from_metadata(group)?),
                Some(acc) => acc.add_metadata(group)?,
            }
        }

        merged_ci.context("no UniFFI metadata groups found in the library")
    }

    /// Fix the 0.28.3 Kotlin codegen defect where an ERROR-enum struct variant
    /// has a field literally named `message`. The non-flat `ErrorTemplate.kt`
    /// emits BOTH a primary-constructor property `val \`message\`` AND an
    /// `override val message get() = "message=…"`, which Kotlin rejects as an
    /// overload-resolution ambiguity (two `message` properties on one class).
    /// Our frozen `client::protocol::ClientError` (and the speech errors) carry a
    /// `message: String` per variant by §0.4/F1-07 contract, so we cannot rename
    /// the field in Rust without breaking the wire snapshots — we disambiguate in
    /// the generated Kotlin instead.
    ///
    /// The fix collapses the two `message` properties into one: the constructor
    /// property becomes `override val \`message\`` (so it legally overrides
    /// `Throwable.message`, and the FFI converters' `value.message` reads the RAW
    /// field — exactly what round-trips), and the redundant formatted getter is
    /// dropped. Applied ONLY to the exact single-`message`-field shape the
    /// template emits, so multi-field variants (whose getter formats other fields
    /// too) and non-error records (`ClientEvent.Error`, which has no override)
    /// are untouched. Swift never hits this (no `override`/`message` collision).
    fn fix_message_field_error_variants(file: &Utf8Path) -> Result<()> {
        let text =
            std::fs::read_to_string(file).with_context(|| format!("reading generated {file}"))?;

        // The exact getter block the template emits for a lone `message` field.
        const GETTER: &str =
            "        override val message\n            get() = \"message=${ `message` }\"\n";
        // The constructor property line + its closing `) : <Error>() {` form the
        // anchor that distinguishes an ERROR variant (own-line `) :`) from the
        // `ClientEvent.Error` record (`…kotlin.String) : ClientEvent()` inline).
        const CTOR_PROP: &str = "        val `message`: kotlin.String\n        ) : ";
        const CTOR_PROP_FIXED: &str = "        override val `message`: kotlin.String\n        ) : ";

        if !text.contains(GETTER) {
            return Ok(()); // no colliding variant in this file
        }
        let patched = text.replace(GETTER, "").replace(CTOR_PROP, CTOR_PROP_FIXED);
        std::fs::write(file, patched).with_context(|| format!("writing patched {file}"))?;
        Ok(())
    }

    /// The `<out_dir>/com/lingxi/code/bindings/<namespace>.kt` file the Kotlin
    /// backend wrote for `namespace` (package = `com.lingxi.code.bindings` from
    /// `apps/android-aar/uniffi.toml`, so the file sits directly under the package
    /// path — NO per-crate sub-dir, since all crates merge into one component).
    fn kotlin_file_path(out_dir: &Utf8Path, namespace: &str) -> Utf8PathBuf {
        out_dir
            .join("com/lingxi/code/bindings")
            .join(format!("{namespace}.kt"))
    }

    /// Faithful re-implementation of the metadata-grouping half of
    /// `uniffi_bindgen::library_mode::find_components` (we ship NO UDL files, so
    /// the UDL branch is omitted), with ONE added pass: [`patch_external_throws`]
    /// on each grouped namespace. Returns the PATCHED groups; the caller builds
    /// `ComponentInterface`s (one per group for Swift, or one merged for Kotlin).
    fn find_patched_groups(library_path: &Utf8Path) -> Result<Vec<MetadataGroup>> {
        let items = macro_metadata::extract_from_library(library_path)
            .context("extracting UniFFI metadata from the host library")?;

        let mut metadata_groups = create_metadata_groups(&items);
        // `group_metadata` runs `fixup_external_type`, which is what turns the
        // cross-crate `ClientError` throw type into `Type::External`.
        group_metadata(&mut metadata_groups, items)?;

        Ok(metadata_groups
            .into_values()
            .map(|mut group| {
                patch_external_throws(&mut group);
                group
            })
            .collect())
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

    // ----- Kotlin merge: de-externalize cross-crate references -----------------

    use std::collections::HashMap;
    use uniffi_meta::{EnumMetadata, FieldMetadata, FnParamMetadata, ObjectImpl, VariantMetadata};

    /// What LOCAL `Type` a user-defined name resolves to once all four crates
    /// share one namespace (so we can undo the `group_metadata` externalization).
    #[derive(Clone, Copy)]
    enum LocalKind {
        Enum,
        Record,
        Object,
    }

    /// Rewrite every `Type::External` reference in every group back to the local
    /// `Type` its DEFINING item uses, keyed by the type name (names are unique
    /// across our four merged crates — verified; only runtime helpers overlap and
    /// those aren't user types). Run BEFORE merging groups into one CI.
    fn de_externalize_groups(groups: Vec<MetadataGroup>) -> Vec<MetadataGroup> {
        // 1. Build name → local kind from the DEFINING items across all groups.
        let mut kinds: HashMap<String, LocalKind> = HashMap::new();
        for group in &groups {
            for item in &group.items {
                match item {
                    Metadata::Enum(e) => {
                        kinds.insert(e.name.clone(), LocalKind::Enum);
                    }
                    Metadata::Record(r) => {
                        kinds.insert(r.name.clone(), LocalKind::Record);
                    }
                    Metadata::Object(o) => {
                        kinds.insert(o.name.clone(), LocalKind::Object);
                    }
                    _ => {}
                }
            }
        }

        // 2. Rewrite every type position in every item.
        groups
            .into_iter()
            .map(|mut group| {
                let items = std::mem::take(&mut group.items);
                for item in items {
                    group.items.insert(de_ext_item(item, &kinds));
                }
                group
            })
            .collect()
    }

    fn de_ext_item(item: Metadata, kinds: &HashMap<String, LocalKind>) -> Metadata {
        match item {
            Metadata::Func(mut m) => {
                m.inputs = de_ext_params(m.inputs, kinds);
                m.return_type = m.return_type.map(|t| de_ext_type(t, kinds));
                m.throws = m.throws.map(|t| de_ext_type(t, kinds));
                Metadata::Func(m)
            }
            Metadata::Method(mut m) => {
                m.inputs = de_ext_params(m.inputs, kinds);
                m.return_type = m.return_type.map(|t| de_ext_type(t, kinds));
                m.throws = m.throws.map(|t| de_ext_type(t, kinds));
                Metadata::Method(m)
            }
            Metadata::TraitMethod(mut m) => {
                m.inputs = de_ext_params(m.inputs, kinds);
                m.return_type = m.return_type.map(|t| de_ext_type(t, kinds));
                m.throws = m.throws.map(|t| de_ext_type(t, kinds));
                Metadata::TraitMethod(m)
            }
            Metadata::Constructor(mut m) => {
                m.inputs = de_ext_params(m.inputs, kinds);
                m.throws = m.throws.map(|t| de_ext_type(t, kinds));
                Metadata::Constructor(m)
            }
            Metadata::Record(mut m) => {
                m.fields = de_ext_fields(m.fields, kinds);
                Metadata::Record(m)
            }
            Metadata::Enum(m) => Metadata::Enum(de_ext_enum(m, kinds)),
            other => other,
        }
    }

    fn de_ext_params(
        params: Vec<FnParamMetadata>,
        kinds: &HashMap<String, LocalKind>,
    ) -> Vec<FnParamMetadata> {
        params
            .into_iter()
            .map(|p| FnParamMetadata {
                ty: de_ext_type(p.ty, kinds),
                ..p
            })
            .collect()
    }

    fn de_ext_fields(
        fields: Vec<FieldMetadata>,
        kinds: &HashMap<String, LocalKind>,
    ) -> Vec<FieldMetadata> {
        fields
            .into_iter()
            .map(|f| FieldMetadata {
                ty: de_ext_type(f.ty, kinds),
                ..f
            })
            .collect()
    }

    fn de_ext_enum(e: EnumMetadata, kinds: &HashMap<String, LocalKind>) -> EnumMetadata {
        EnumMetadata {
            variants: e
                .variants
                .into_iter()
                .map(|v| VariantMetadata {
                    fields: de_ext_fields(v.fields, kinds),
                    ..v
                })
                .collect(),
            ..e
        }
    }

    /// Mirror of `uniffi_meta`'s `convert_type`, but INVERSE: turn each
    /// `Type::External` (and `Custom` builtins / container inners) back into the
    /// LOCAL `Type` its defining item uses. `DataClass` → `Enum` or `Record` by
    /// the name→kind map; `Interface`/`Trait` → `Object`. Recurses through the
    /// structural containers exactly as the forward converter does.
    fn de_ext_type(ty: Type, kinds: &HashMap<String, LocalKind>) -> Type {
        match ty {
            Type::External {
                module_path,
                name,
                kind,
                ..
            } => match kind {
                ExternalKind::Interface | ExternalKind::Trait => Type::Object {
                    module_path,
                    name,
                    imp: ObjectImpl::Struct,
                },
                ExternalKind::DataClass => match kinds.get(&name) {
                    Some(LocalKind::Record) => Type::Record { module_path, name },
                    Some(LocalKind::Object) => Type::Object {
                        module_path,
                        name,
                        imp: ObjectImpl::Struct,
                    },
                    // Default DataClass → Enum (covers enums + any unmapped name;
                    // an enum tag is what `throws_name`/error rendering expects).
                    _ => Type::Enum { module_path, name },
                },
            },
            Type::Optional { inner_type } => Type::Optional {
                inner_type: Box::new(de_ext_type(*inner_type, kinds)),
            },
            Type::Sequence { inner_type } => Type::Sequence {
                inner_type: Box::new(de_ext_type(*inner_type, kinds)),
            },
            Type::Map {
                key_type,
                value_type,
            } => Type::Map {
                key_type: Box::new(de_ext_type(*key_type, kinds)),
                value_type: Box::new(de_ext_type(*value_type, kinds)),
            },
            Type::Custom {
                module_path,
                name,
                builtin,
            } => Type::Custom {
                module_path,
                name,
                builtin: Box::new(de_ext_type(*builtin, kinds)),
            },
            other => other,
        }
    }
}
