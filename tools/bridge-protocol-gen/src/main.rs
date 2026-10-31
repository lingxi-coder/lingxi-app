//! Generate the top-level bridge DTOs directly from the pinned Rust wire roots.
//! Nested DTOs remain in protocol.ts. Fail closed on Serde features that this
//! generator does not understand; do not infer shapes from example JSON.
use proc_macro2::{Delimiter, TokenStream, TokenTree};
use quote::ToTokens;
use std::{collections::BTreeSet, env, fs, path::Path};
use syn::{
    parse::Parser, punctuated::Punctuated, Attribute, Fields, GenericArgument, Item, ItemEnum,
    ItemMacro, ItemStruct, LitStr, Meta, PathArguments, Token, Type, Variant,
};

type Result<T> = std::result::Result<T, String>;

#[derive(Default)]
struct Serde {
    rename: Option<String>,
    rename_all: Option<String>,
    tag: Option<String>,
    default: bool,
    skip: Option<String>,
    flatten: bool,
    deny_unknown_fields: bool,
    transparent: bool,
}

fn serde_attrs(attrs: &[Attribute]) -> Result<Serde> {
    let mut out = Serde::default();
    for attr in attrs {
        if attr.path().is_ident("doc")
            || attr.path().is_ident("derive")
            || attr.path().is_ident("allow")
            || attr.path().is_ident("non_exhaustive")
        {
            continue;
        }
        if attr.path().is_ident("cfg_attr") {
            let metas = attr
                .parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)
                .map_err(|e| e.to_string())?;
            if metas.len() != 2
                || metas[0].to_token_string() != "feature = \"uniffi\""
                || !(metas[1].path().is_ident("uniffi")
                    || matches!(&metas[1], Meta::List(list) if list.path.is_ident("derive") && matches!(list.tokens.to_string().as_str(), "uniffi :: Record" | "uniffi :: Enum")))
            {
                return Err("unsupported cfg_attr on protocol field or variant".into());
            }
            continue;
        }
        if !attr.path().is_ident("serde") {
            return Err(format!(
                "unsupported attribute {}",
                attr.path().segments.last().unwrap().ident
            ));
        }
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("rename") {
                out.rename = Some(meta.value()?.parse::<LitStr>()?.value());
            } else if meta.path.is_ident("rename_all") {
                out.rename_all = Some(meta.value()?.parse::<LitStr>()?.value());
            } else if meta.path.is_ident("tag") {
                out.tag = Some(meta.value()?.parse::<LitStr>()?.value());
            } else if meta.path.is_ident("default") {
                if meta.input.peek(Token![=]) || meta.input.peek(syn::token::Paren) {
                    return Err(meta.error("custom defaults are unsupported"));
                }
                out.default = true;
            } else if meta.path.is_ident("skip_serializing_if") {
                out.skip = Some(meta.value()?.parse::<LitStr>()?.value());
            } else if meta.path.is_ident("flatten") {
                out.flatten = true;
            } else if meta.path.is_ident("deny_unknown_fields") {
                out.deny_unknown_fields = true;
            } else if meta.path.is_ident("transparent") {
                out.transparent = true;
            } else {
                return Err(meta.error("unsupported serde attribute"));
            }
            Ok(())
        })
        .map_err(|e| e.to_string())?;
    }
    Ok(out)
}

// Avoid quote as a direct dependency; Meta implements ToTokens through syn's
// normal token representation. Only this fixed cfg predicate is accepted.
trait MetaText {
    fn to_token_string(&self) -> String;
}
impl MetaText for Meta {
    fn to_token_string(&self) -> String {
        match self {
            Meta::NameValue(v) if v.path.is_ident("feature") => match &v.value {
                syn::Expr::Lit(l) => match &l.lit {
                    syn::Lit::Str(s) => format!("feature = {:?}", s.value()),
                    _ => String::new(),
                },
                _ => String::new(),
            },
            _ => String::new(),
        }
    }
}

fn valid_key(name: &str) -> Result<()> {
    let mut chars = name.chars();
    if name == "__proto__"
        || !chars
            .next()
            .is_some_and(|c| c == '_' || c.is_ascii_alphabetic())
        || !chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
    {
        return Err(format!("unsupported wire name {name:?}"));
    }
    Ok(())
}

fn snake(name: &str) -> String {
    let mut result = String::new();
    for (index, c) in name.chars().enumerate() {
        if c.is_ascii_uppercase() {
            if index > 0 {
                result.push('_');
            }
            result.push(c.to_ascii_lowercase());
        } else {
            result.push(c);
        }
    }
    result
}

fn file(path: &Path) -> Result<syn::File> {
    let source = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    syn::parse_file(&source).map_err(|e| format!("{}: {e}", path.display()))
}
fn macro_item<'a>(file: &'a syn::File, name: &str, definition: bool) -> Result<&'a ItemMacro> {
    let found: Vec<_> = file
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Macro(m)
                if if definition {
                    m.ident.as_ref().is_some_and(|v| v == name)
                } else {
                    m.mac.path.is_ident(name)
                } =>
            {
                Some(m)
            }
            _ => None,
        })
        .collect();
    if found.len() != 1 {
        return Err(format!(
            "expected one {name} {}",
            if definition {
                "definition"
            } else {
                "invocation"
            }
        ));
    }
    Ok(found[0])
}

// Walk the compiler's token tree inside macro_rules!, find the emitted enum,
// and parse its real attributes with syn. Reject changes to the macro shape so
// updating the generated output cannot silently bless a different wire tag.
fn emitted_enum_attrs(stream: TokenStream, name: &str) -> Result<Option<Vec<Attribute>>> {
    let tokens: Vec<_> = stream.into_iter().collect();
    for (i, token) in tokens.iter().enumerate() {
        if matches!(token, TokenTree::Ident(id) if id == "enum")
            && matches!(tokens.get(i + 1), Some(TokenTree::Ident(id)) if id == name)
        {
            let body = match tokens.get(i + 2) {
                Some(TokenTree::Group(g)) if g.delimiter() == Delimiter::Brace => {
                    g.stream().to_string()
                }
                _ => return Err(format!("{name}: unexpected macro enum body")),
            };
            if body != "$ ($ variants) *" {
                return Err(format!("{name}: unexpected macro enum body: {body}"));
            }
            let mut start = i;
            if start > 0 && matches!(&tokens[start-1], TokenTree::Ident(id) if id == "pub") {
                start -= 1;
            }
            while start >= 2
                && matches!(&tokens[start-2], TokenTree::Punct(p) if p.as_char() == '#')
                && matches!(&tokens[start-1], TokenTree::Group(g) if g.delimiter() == Delimiter::Bracket)
            {
                start -= 2;
            }
            let attrs: TokenStream = tokens[start
                ..if i > 0 && matches!(&tokens[i-1], TokenTree::Ident(id) if id == "pub") {
                    i - 1
                } else {
                    i
                }]
                .iter()
                .cloned()
                .collect();
            return Attribute::parse_outer
                .parse2(attrs)
                .map(Some)
                .map_err(|e| e.to_string());
        }
        if let TokenTree::Group(g) = token {
            if let Some(found) = emitted_enum_attrs(g.stream(), name)? {
                return Ok(Some(found));
            }
        }
    }
    Ok(None)
}
fn require_derive(attrs: &[Attribute], name: &str, required: &str) -> Result<()> {
    let derives: Vec<_> = attrs
        .iter()
        .filter(|a| a.path().is_ident("derive"))
        .map(|a| a.parse_args_with(Punctuated::<syn::Path, Token![,]>::parse_terminated))
        .collect::<std::result::Result<_, _>>()
        .map_err(|e| e.to_string())?;
    if !derives
        .iter()
        .any(|paths| paths.iter().any(|path| path.is_ident(required)))
    {
        return Err(format!(
            "{name} must derive {required}; custom serialization requires review"
        ));
    }
    Ok(())
}
fn variants(file: &syn::File, macro_name: &str, enum_name: &str) -> Result<Vec<Variant>> {
    let definition = macro_item(file, macro_name, true)?;
    let attrs = emitted_enum_attrs(definition.mac.tokens.clone(), enum_name)?
        .ok_or_else(|| format!("missing emitted {enum_name}"))?;
    require_derive(&attrs, enum_name, "Serialize")?;
    let config = serde_attrs(&attrs)?;
    if config.tag.as_deref() != Some("type")
        || config.rename_all.as_deref() != Some("snake_case")
        || config.rename.is_some()
        || config.default
        || config.skip.is_some()
        || config.flatten
        || config.deny_unknown_fields
        || config.transparent
    {
        return Err(format!(
            "{enum_name}: unsupported enum serde representation"
        ));
    }
    let wire_name = format!("{enum_name}Wire");
    let attrs = emitted_enum_attrs(definition.mac.tokens.clone(), &wire_name)?
        .ok_or_else(|| format!("missing emitted {wire_name}"))?;
    require_derive(&attrs, &wire_name, "Deserialize")?;
    let config = serde_attrs(&attrs)?;
    if config.rename_all.as_deref() != Some("snake_case")
        || config.tag.is_some()
        || config.rename.is_some()
        || config.default
        || config.skip.is_some()
        || config.flatten
        || config.deny_unknown_fields
        || config.transparent
    {
        return Err(format!(
            "{wire_name}: unsupported decoder serde representation"
        ));
    }
    let invocation = macro_item(file, macro_name, false)?;
    let parsed = Punctuated::<Variant, Token![,]>::parse_terminated
        .parse2(invocation.mac.tokens.clone())
        .map_err(|e| e.to_string())?;
    if parsed.is_empty() {
        return Err(format!("{enum_name}: no variants"));
    }
    Ok(parsed.into_iter().collect())
}
fn struct_item<'a>(file: &'a syn::File, name: &str) -> Result<&'a ItemStruct> {
    let found: Vec<_> = file
        .items
        .iter()
        .filter_map(|i| match i {
            Item::Struct(s) if s.ident == name => Some(s),
            _ => None,
        })
        .collect();
    if found.len() != 1 {
        return Err(format!("expected one {name} struct"));
    }
    Ok(found[0])
}
fn enum_item<'a>(file: &'a syn::File, name: &str) -> Result<&'a ItemEnum> {
    let found: Vec<_> = file
        .items
        .iter()
        .filter_map(|i| match i {
            Item::Enum(s) if s.ident == name => Some(s),
            _ => None,
        })
        .collect();
    if found.len() != 1 {
        return Err(format!("expected one {name} enum"));
    }
    Ok(found[0])
}
fn item_serde(attrs: &[Attribute], name: &str) -> Result<Serde> {
    let out = serde_attrs(attrs).map_err(|e| format!("{name}: {e}"))?;
    if out.rename.is_some() || out.default || out.skip.is_some() || out.flatten || out.transparent {
        return Err(format!("{name}: unsupported container serde attribute"));
    }
    Ok(out)
}

fn inner<'a>(ty: &'a Type, expected: &str) -> Option<&'a Type> {
    let Type::Path(path) = ty else {
        return None;
    };
    let segment = path.path.segments.last()?;
    if segment.ident != expected {
        return None;
    }
    let PathArguments::AngleBracketed(args) = &segment.arguments else {
        return None;
    };
    if args.args.len() != 1 {
        return None;
    }
    match args.args.first()? {
        GenericArgument::Type(t) => Some(t),
        _ => None,
    }
}
fn ts_type(ty: &Type, imports: &mut BTreeSet<String>) -> Result<String> {
    let Type::Path(path) = ty else {
        return Err("unsupported non-path Rust type".into());
    };
    let segment = path.path.segments.last().ok_or("empty Rust type path")?;
    let name = segment.ident.to_string();
    match name.as_str() {
        "Option" => Ok(format!(
            "{} | null",
            ts_type(inner(ty, "Option").ok_or("invalid Option")?, imports)?
        )),
        "Vec" => Ok(format!(
            "Array<{}>",
            ts_type(inner(ty, "Vec").ok_or("invalid Vec")?, imports)?
        )),
        "HashMap" => {
            let PathArguments::AngleBracketed(args) = &segment.arguments else {
                return Err("invalid HashMap".into());
            };
            if args.args.len() != 2 {
                return Err("invalid HashMap".into());
            }
            let mut it = args.args.iter();
            let (Some(GenericArgument::Type(k)), Some(GenericArgument::Type(v))) =
                (it.next(), it.next())
            else {
                return Err("invalid HashMap types".into());
            };
            if ts_type(k, imports)? != "string" {
                return Err("non-string map keys are unsupported".into());
            }
            Ok(format!("Record<string, {}>", ts_type(v, imports)?))
        }
        "String" | "ClientTextDto" | "ProviderCredentialSecretDto" => Ok("string".into()),
        "AskUserQuestionAnswersDto" => Ok("Record<string, string>".into()),
        "bool" => Ok("boolean".into()),
        "u8" | "u16" | "u32" | "u64" | "i8" | "i16" | "i32" | "i64" | "f32" | "f64" => {
            Ok("number".into())
        }
        _ => {
            if !matches!(segment.arguments, PathArguments::None)
                || !name.ends_with("Dto") && name != "PermissionRequest"
            {
                return Err(format!("unsupported Rust type {name}"));
            }
            if !matches!(
                name.as_str(),
                "TaskRowDto" | "OpenAiOAuthSessionDto" | "CoordinatorWorkerDto" | "UiSurfaceDto"
            ) {
                imports.insert(name.clone());
            }
            Ok(name)
        }
    }
}
fn field(
    field: &syn::Field,
    command: bool,
    imports: &mut BTreeSet<String>,
) -> Result<(String, Option<String>)> {
    let serde = serde_attrs(&field.attrs)?;
    if serde.rename_all.is_some()
        || serde.tag.is_some()
        || serde.deny_unknown_fields
        || serde.transparent
    {
        return Err("unsupported field serde attribute".into());
    }
    let name = serde
        .rename
        .unwrap_or_else(|| field.ident.as_ref().expect("named field").to_string());
    valid_key(&name)?;
    if let Some(skip) = &serde.skip {
        if !matches!(
            skip.as_str(),
            "Option::is_none" | "Vec::is_empty" | "HashMap::is_empty" | "is_false"
        ) {
            return Err(format!("unsupported skip predicate {skip}"));
        }
        let correct = match skip.as_str() {
            "Option::is_none" => inner(&field.ty, "Option").is_some(),
            "Vec::is_empty" => inner(&field.ty, "Vec").is_some(),
            "HashMap::is_empty" => {
                matches!(&field.ty, Type::Path(p) if p.path.segments.last().is_some_and(|s| s.ident == "HashMap"))
            }
            "is_false" => matches!(&field.ty, Type::Path(p) if p.path.is_ident("bool")),
            _ => false,
        };
        if !correct {
            return Err(format!("{name}: skip predicate and field type disagree"));
        }
    }
    if serde.flatten {
        if serde.default || serde.skip.is_some() {
            return Err(format!("{name}: unsupported flatten attributes"));
        }
        return Ok((String::new(), Some(ts_type(&field.ty, imports)?)));
    }
    let option = inner(&field.ty, "Option");
    if option.is_some_and(|inner_ty| inner(inner_ty, "Option").is_some()) {
        return Err(format!("{name}: nested top-level Option is unsupported"));
    }
    let optional = if command {
        serde.default || option.is_some()
    } else {
        serde.skip.is_some()
    };
    let ty = if let Some(inner) = option {
        let base = ts_type(inner, imports)?;
        if command || serde.skip.is_none() {
            format!("{base} | null")
        } else {
            base
        }
    } else {
        ts_type(&field.ty, imports)?
    };
    Ok((
        format!("{name}{}: {ty}", if optional { "?" } else { "" }),
        None,
    ))
}
fn render_variants(
    name: &str,
    variants: &[Variant],
    command: bool,
    imports: &mut BTreeSet<String>,
) -> Result<(String, Vec<String>)> {
    let mut output = format!("export type {name} =\n");
    let mut names = Vec::new();
    for variant in variants {
        let serde = serde_attrs(&variant.attrs)?;
        if serde.rename_all.is_some()
            || serde.tag.is_some()
            || serde.default
            || serde.skip.is_some()
            || serde.flatten
            || serde.deny_unknown_fields
            || serde.transparent
        {
            return Err(format!(
                "{}: unsupported variant serde attributes",
                variant.ident
            ));
        }
        let wire = serde
            .rename
            .unwrap_or_else(|| snake(&variant.ident.to_string()));
        valid_key(&wire)?;
        if names.contains(&wire) {
            return Err(format!("duplicate variant {wire}"));
        }
        names.push(wire.clone());
        let mut fields = vec![format!("type: '{wire}'")];
        let mut flattened = Vec::new();
        match &variant.fields {
            Fields::Named(named) => {
                for f in &named.named {
                    let (part, flat) = field(f, command, imports)?;
                    if let Some(flat) = flat {
                        flattened.push(flat);
                    } else {
                        fields.push(part);
                    }
                }
            }
            Fields::Unit => (),
            Fields::Unnamed(_) => {
                return Err(format!("{}: tuple variants unsupported", variant.ident))
            }
        }
        output.push_str(&format!("  | {{ {} }}", fields.join("; ")));
        for flat in flattened {
            output.push_str(&format!(" & {flat}"));
        }
        output.push('\n');
    }
    output.push_str(";\n\n");
    Ok((output, names))
}
fn render_struct(
    item: &ItemStruct,
    imports: &mut BTreeSet<String>,
) -> Result<(String, Vec<String>)> {
    require_derive(&item.attrs, &item.ident.to_string(), "Serialize")?;
    require_derive(&item.attrs, &item.ident.to_string(), "Deserialize")?;
    let config = item_serde(&item.attrs, &item.ident.to_string())?;
    if config.tag.is_some() || config.rename_all.is_some() {
        return Err(format!(
            "{}: unsupported struct serde representation",
            item.ident
        ));
    }
    let Fields::Named(named) = &item.fields else {
        return Err(format!("{}: only named fields supported", item.ident));
    };
    let mut output = if config.deny_unknown_fields {
        String::from("/** Rust Serde rejects unknown fields when decoding this DTO. */\n")
    } else {
        String::new()
    };
    output.push_str(&format!("export interface {} {{\n", item.ident));
    let mut keys = Vec::new();
    for f in &named.named {
        let (part, flat) = field(f, false, imports)?;
        if flat.is_some() {
            return Err("struct flatten unsupported".into());
        }
        keys.push(
            part.split(':')
                .next()
                .unwrap()
                .trim_end_matches('?')
                .to_string(),
        );
        output.push_str(&format!("  {part};\n"));
    }
    output.push_str("}\n\n");
    Ok((output, keys))
}
fn render_string_enum(item: &ItemEnum) -> Result<String> {
    require_derive(&item.attrs, &item.ident.to_string(), "Serialize")?;
    require_derive(&item.attrs, &item.ident.to_string(), "Deserialize")?;
    let config = item_serde(&item.attrs, &item.ident.to_string())?;
    if config.tag.is_some() || config.rename_all.as_deref() != Some("lowercase") {
        return Err(format!(
            "{}: unexpected enum serde representation",
            item.ident
        ));
    }
    let mut names = Vec::new();
    for v in &item.variants {
        if !matches!(v.fields, Fields::Unit) {
            return Err("string enum has payload".into());
        }
        let serde = serde_attrs(&v.attrs)?;
        if serde.rename_all.is_some()
            || serde.tag.is_some()
            || serde.default
            || serde.skip.is_some()
            || serde.flatten
            || serde.deny_unknown_fields
            || serde.transparent
        {
            return Err("unexpected string enum serde attribute".into());
        }
        let wire = serde
            .rename
            .unwrap_or_else(|| v.ident.to_string().to_ascii_lowercase());
        valid_key(&wire)?;
        names.push(format!("'{wire}'"));
    }
    Ok(format!(
        "export type {} = {};\n\n",
        item.ident,
        names.join(" | ")
    ))
}
fn record(name: &str, type_name: &str, keys: &[String]) -> String {
    let mut output = format!("export const {name}: Record<{type_name}, true> = {{\n");
    for key in keys {
        output.push_str(&format!("  {key}: true,\n"));
    }
    output.push_str("};\n\n");
    output
}
fn validate_credential_wrapper(file: &syn::File) -> Result<()> {
    let item = struct_item(file, "ProviderCredentialSecretDto")?;
    let config = serde_attrs(&item.attrs)?;
    if !config.transparent
        || config.rename.is_some()
        || config.rename_all.is_some()
        || config.tag.is_some()
        || config.default
        || config.skip.is_some()
        || config.flatten
        || config.deny_unknown_fields
    {
        return Err("ProviderCredentialSecretDto must remain serde-transparent".into());
    }
    let derives: Vec<_> = item
        .attrs
        .iter()
        .filter(|a| a.path().is_ident("derive"))
        .map(|a| a.parse_args_with(Punctuated::<syn::Path, Token![,]>::parse_terminated))
        .collect::<std::result::Result<_, _>>()
        .map_err(|e| e.to_string())?;
    if !["Serialize", "Deserialize"].iter().all(|expected| {
        derives
            .iter()
            .any(|paths| paths.iter().any(|path| path.is_ident(expected)))
    }) {
        return Err("ProviderCredentialSecretDto must derive Serialize and Deserialize".into());
    }
    let Fields::Named(fields) = &item.fields else {
        return Err("credential wrapper must have one named String".into());
    };
    if fields.named.len() != 1 {
        return Err("credential wrapper must have one named String".into());
    }
    let value = fields.named.first().unwrap();
    if !matches!(&value.ty, Type::Path(path) if path.path.is_ident("String"))
        || value.attrs.iter().any(|attr| {
            attr.path().is_ident("serde")
                || attr.path().is_ident("cfg")
                || attr.path().is_ident("cfg_attr")
        })
    {
        return Err("credential wrapper wire field must remain String".into());
    }
    Ok(())
}

// Commands containing exact source text have explicit deserializer branches.
// Check those branches against the declaration rather than assuming that the
// derived private carrier handles every variant.
fn decoder_impl<'a>(file: &'a syn::File, name: &str) -> Result<&'a syn::ItemImpl> {
    let found: Vec<_> = file
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Impl(item)
                if item.trait_.as_ref().is_some_and(|(_, path, _)| {
                    path.segments
                        .last()
                        .is_some_and(|s| s.ident == "Deserialize")
                }) && matches!(&*item.self_ty, Type::Path(p) if p.path.is_ident(name)) =>
            {
                Some(item)
            }
            _ => None,
        })
        .collect();
    if found.len() != 1 {
        return Err(format!("expected one {name} deserializer"));
    }
    Ok(found[0])
}
fn validate_command_decoder(file: &syn::File, variants: &[Variant]) -> Result<()> {
    let implementation = decoder_impl(file, "ClientCommand")?;
    let method = implementation
        .items
        .iter()
        .find_map(|item| match item {
            syn::ImplItem::Fn(method) if method.sig.ident == "deserialize" => Some(method),
            _ => None,
        })
        .ok_or("missing ClientCommand deserialize method")?;
    let matches: Vec<_> = method
        .block
        .stmts
        .iter()
        .filter_map(|stmt| match stmt {
            syn::Stmt::Expr(syn::Expr::Match(expr), _) => Some(expr),
            _ => None,
        })
        .collect();
    if matches.len() != 1 || matches[0].expr.to_token_stream().to_string() != "tag . as_str ()" {
        return Err("unsupported ClientCommand tag dispatch".into());
    }
    let expected_fallback: syn::Expr = syn::parse_str(r#"{
        let json = command_raw_wire_json::<D::Error>(&tag, fields)?;
        let wire: ClientCommandWire = serde_json::from_str(&json).map_err(serde::de::Error::custom)?;
        Ok(wire.into())
    }"#).map_err(|e| format!("bad internal fallback template: {e}"))?;
    let mut seen = BTreeSet::new();
    let mut fallback = false;
    for arm in &matches[0].arms {
        if matches!(arm.pat, syn::Pat::Wild(_)) {
            if fallback
                || arm.body.to_token_stream().to_string()
                    != expected_fallback.to_token_stream().to_string()
            {
                return Err("unsupported ClientCommand fallback decoder".into());
            }
            fallback = true;
            continue;
        }
        let syn::Pat::Lit(pattern) = &arm.pat else {
            return Err("unsupported command decoder pattern".into());
        };
        let syn::Lit::Str(tag) = &pattern.lit else {
            return Err("non-string command decoder tag".into());
        };
        let tag = tag.value();
        if !seen.insert(tag.clone()) || arm.guard.is_some() {
            return Err(format!("duplicate or guarded command decoder {tag}"));
        }
        let syn::Expr::Call(call) = &*arm.body else {
            return Err(format!("unsupported command decoder branch {tag}"));
        };
        if !matches!(&*call.func, syn::Expr::Path(path) if path.path.is_ident("Ok"))
            || call.args.len() != 1
        {
            return Err(format!("unsupported command decoder result {tag}"));
        }
        let payload = call.args.first().unwrap();
        let (variant_name, fields) = match payload {
            syn::Expr::Path(path) => (path.path.segments.last().unwrap().ident.to_string(), None),
            syn::Expr::Struct(value) => (
                value.path.segments.last().unwrap().ident.to_string(),
                Some(&value.fields),
            ),
            _ => return Err(format!("unsupported command decoder payload {tag}")),
        };
        let variant = variants
            .iter()
            .find(|v| v.ident == variant_name)
            .ok_or_else(|| format!("unknown command decoder variant {variant_name}"))?;
        let attr = serde_attrs(&variant.attrs)?;
        if tag != attr.rename.unwrap_or_else(|| snake(&variant_name)) {
            return Err(format!("command decoder tag mismatch for {variant_name}"));
        }
        match (&variant.fields, fields) {
            (Fields::Unit, None) => (),
            (Fields::Named(expected), Some(actual)) => {
                if expected.named.len() != actual.len() {
                    return Err(format!("command decoder field count mismatch for {tag}"));
                }
                for field in &expected.named {
                    let ident = field.ident.as_ref().ok_or("unnamed command field")?;
                    let config = serde_attrs(&field.attrs)?;
                    let key = config.rename.unwrap_or_else(|| ident.to_string());
                    let option = inner(&field.ty, "Option").is_some();
                    if config.default && !option {
                        return Err(format!(
                            "custom decoder {tag}.{key} has unsupported default"
                        ));
                    }
                    let value = actual.iter().find(|value| matches!(&value.member, syn::Member::Named(name) if name == ident)).ok_or_else(|| format!("missing command decoder {tag}.{key}"))?;
                    let syn::Expr::Try(try_expr) = &value.expr else {
                        return Err(format!("unsupported command decoder {tag}.{key}"));
                    };
                    let syn::Expr::Call(call) = &*try_expr.expr else {
                        return Err(format!("unsupported command decoder {tag}.{key}"));
                    };
                    let helper = if option {
                        "command_raw_optional"
                    } else {
                        "command_raw_required"
                    };
                    if !matches!(&*call.func, syn::Expr::Path(path) if path.path.is_ident(helper))
                        || call.args.len() != 2
                        || call.args.first().unwrap().to_token_stream().to_string()
                            != "& mut fields"
                        || !matches!(call.args.iter().nth(1), Some(syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Str(s), .. })) if s.value() == key)
                    {
                        return Err(format!("command decoder field mismatch for {tag}.{key}"));
                    }
                }
            }
            _ => return Err(format!("command decoder payload shape mismatch for {tag}")),
        }
    }
    if !fallback {
        return Err("missing ClientCommand fallback decoder".into());
    }
    Ok(())
}

// Fingerprints make changes to custom Serde implementations visible to the
// regeneration check. The implementations still require human review; their
// behavior is not inferred from samples or from their Rust field declarations.
fn portable_source(path: &Path) -> Result<Vec<u8>> {
    // Git checkouts on Windows may translate line endings. Fingerprint source
    // content, not the checkout's CRLF convention.
    Ok(fs::read_to_string(path)
        .map_err(|e| format!("{}: {e}", path.display()))?
        .replace("\r\n", "\n")
        .into_bytes())
}

fn fingerprint(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325u64, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    })
}

// This is deliberately only a basic top-level check. Nested DTO contracts,
// semantic rules, and trust-boundary validation remain hand-maintained.
fn event_field_rules(variants: &[Variant]) -> Result<String> {
    let mut out = String::from("type BasicField = readonly [string, 'string' | 'boolean' | 'number' | 'integer' | 'unsigned' | 'array' | 'map' | 'opaque', boolean, boolean, boolean];\n\nconst CLIENT_EVENT_FIELDS: Record<ClientEvent['type'], readonly BasicField[]> = {\n");
    for variant in variants {
        let config = serde_attrs(&variant.attrs)?;
        let wire = config
            .rename
            .unwrap_or_else(|| snake(&variant.ident.to_string()));
        valid_key(&wire)?;
        out.push_str(&format!("  {wire}: [\n"));
        if let Fields::Named(fields) = &variant.fields {
            for field in &fields.named {
                let config = serde_attrs(&field.attrs)?;
                if config.flatten {
                    continue;
                }
                let name = config
                    .rename
                    .unwrap_or_else(|| field.ident.as_ref().expect("named field").to_string());
                valid_key(&name)?;
                let option = inner(&field.ty, "Option");
                let optional = config.skip.is_some();
                let nullable = option.is_some() && config.skip.is_none();
                let omit_null = option.is_some() && config.skip.is_some();
                let ty = option.unwrap_or(&field.ty);
                let Type::Path(path) = ty else {
                    return Err("unsupported event field type".into());
                };
                let name_of_type = path
                    .path
                    .segments
                    .last()
                    .ok_or("empty event field type")?
                    .ident
                    .to_string();
                let kind = match name_of_type.as_str() {
                    "String" | "ClientTextDto" | "ProviderCredentialSecretDto" => "string",
                    "bool" => "boolean",
                    "f32" | "f64" => "number",
                    "i8" | "i16" | "i32" | "i64" => "integer",
                    "u8" | "u16" | "u32" | "u64" => "unsigned",
                    "Vec" => "array",
                    "HashMap" | "AskUserQuestionAnswersDto" => "map",
                    _ => "opaque",
                };
                out.push_str(&format!(
                    "    ['{name}', '{kind}', {optional}, {nullable}, {omit_null}],\n"
                ));
            }
        }
        out.push_str("  ],\n");
    }
    out.push_str(r#"};

/** Check fields understood from Rust's top-level Serde declarations only.
 * Unknown future variants remain accepted. Nested DTOs require their own guards. */
export function validateBasicClientEventFields(input: Record<string, unknown>, type: string): Record<string, unknown> {
  if (!Object.prototype.hasOwnProperty.call(CLIENT_EVENT_FIELDS, type)) return input;
  const fields = CLIENT_EVENT_FIELDS[type as ClientEvent['type']];
  const result = { ...input };
  for (const [name, kind, optional, nullable, omitNull] of fields) {
    const present = Object.prototype.hasOwnProperty.call(input, name);
    const value = present ? input[name] : undefined;
    if (value === undefined && optional) continue;
    if (value === null && nullable) continue;
    if (value === null && omitNull) {
      delete result[name];
      continue;
    }
    let valid: boolean;
    switch (kind) {
      case 'string': valid = typeof value === 'string'; break;
      case 'boolean': valid = typeof value === 'boolean'; break;
      case 'number': valid = typeof value === 'number' && Number.isFinite(value); break;
      case 'integer': valid = typeof value === 'number' && Number.isInteger(value); break;
      case 'unsigned': valid = typeof value === 'number' && Number.isInteger(value) && value >= 0; break;
      case 'array': valid = Array.isArray(value); break;
      case 'map': valid = typeof value === 'object' && value !== null && !Array.isArray(value); break;
      case 'opaque': valid = value !== undefined && value !== null; break;
    }
    if (!valid) throw new Error(`invalid client event ${type} field ${name}`);
  }
  return result;
}

"#);
    Ok(out)
}
fn generate(root: &Path) -> Result<String> {
    let protocol = root.join("crates/client/src/protocol");
    let commands = file(&protocol.join("commands.rs"))?;
    let events = file(&protocol.join("events.rs"))?;
    let listings = file(&protocol.join("listings.rs"))?;
    let command_variants = variants(&commands, "define_client_command", "ClientCommand")?;
    let event_variants = variants(&events, "define_client_event", "ClientEvent")?;
    validate_command_decoder(&commands, &command_variants)?;
    validate_credential_wrapper(&commands)?;
    let mut imports = BTreeSet::new();
    let (command, command_keys) =
        render_variants("ClientCommand", &command_variants, true, &mut imports)?;
    let (event, event_keys) = render_variants("ClientEvent", &event_variants, false, &mut imports)?;
    let (task, _) = render_struct(struct_item(&listings, "TaskRowDto")?, &mut imports)?;
    let (oauth, _) = render_struct(struct_item(&events, "OpenAiOAuthSessionDto")?, &mut imports)?;
    let (worker, _) = render_struct(
        struct_item(&listings, "CoordinatorWorkerDto")?,
        &mut imports,
    )?;
    let surface = render_string_enum(enum_item(&commands, "UiSurfaceDto")?)?;
    let mut output = String::from("// Generated by tools/bridge-protocol-gen from the Cargo-locked Harness wire declarations.\n// Do not edit. Run scripts/checks/generate-client-protocol.py after updating the pin.\n// Nested DTOs and semantic validation remain in protocol.ts and validation.ts.\n\n");
    let command_decoder = decoder_impl(&commands, "ClientCommand")?
        .to_token_stream()
        .to_string();
    let event_decoder = decoder_impl(&events, "ClientEvent")?
        .to_token_stream()
        .to_string();
    let command_source = portable_source(&protocol.join("commands.rs"))?;
    let event_source = portable_source(&protocol.join("events.rs"))?;
    let text = portable_source(&protocol.join("text.rs"))?;
    let answers = portable_source(&protocol.join("ask_user_question.rs"))?;
    let credential = struct_item(&commands, "ProviderCredentialSecretDto")?
        .to_token_stream()
        .to_string();
    output.push_str(&format!("// Custom Serde fingerprints (review changes): command={:016x} event={:016x} text={:016x} answers={:016x} credential={:016x} command_source={:016x} event_source={:016x}\n\n", fingerprint(command_decoder.as_bytes()), fingerprint(event_decoder.as_bytes()), fingerprint(&text), fingerprint(&answers), fingerprint(credential.as_bytes()), fingerprint(&command_source), fingerprint(&event_source)));
    if !imports.is_empty() {
        output.push_str("import type {\n");
        for name in imports {
            output.push_str(&format!("  {name},\n"));
        }
        output.push_str("} from './protocol.js';\n\n");
    }
    output.push_str(&surface);
    output.push_str(&task);
    output.push_str(&oauth);
    output.push_str(&worker);
    output.push_str(&command);
    output.push_str(&event);
    output.push_str(&event_field_rules(&event_variants)?);
    output.push_str(&record(
        "ALL_CLIENT_COMMAND_TYPES",
        "ClientCommand['type']",
        &command_keys,
    ));
    output.push_str(&record(
        "ALL_CLIENT_EVENT_TYPES",
        "ClientEvent['type']",
        &event_keys,
    ));
    output.truncate(output.trim_end_matches('\n').len());
    output.push('\n');
    Ok(output)
}
fn main() {
    let args: Vec<_> = env::args().collect();
    if args.len() != 2 {
        eprintln!("usage: bridge-protocol-gen HARNESS_ROOT");
        std::process::exit(2);
    }
    match generate(Path::new(&args[1])) {
        Ok(output) => print!("{output}"),
        Err(error) => {
            eprintln!("bridge-protocol-gen: {error}");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(body: &str) -> syn::File {
        syn::parse_file(&format!(
            r#"
            macro_rules! define_client_event {{
                ($($variants:tt)*) => {{
                    #[derive(Serialize)]
                    #[serde(tag = "type", rename_all = "snake_case")]
                    pub enum ClientEvent {{ $($variants)* }}
                    #[derive(Deserialize)]
                    #[serde(rename_all = "snake_case")]
                    enum ClientEventWire {{ $($variants)* }}
                }};
            }}
            define_client_event! {{ {body} }}
        "#
        ))
        .unwrap()
    }

    #[test]
    fn variants_come_from_rust_with_rename_flatten_and_option_semantics() {
        let source = fixture(
            r#"
            #[serde(rename = "renamed_event")]
            Event {
                #[serde(rename = "actual_key")]
                original: ClientTextDto,
                #[serde(default, skip_serializing_if = "Option::is_none")]
                omitted: Option<String>,
                nullable: Option<String>,
                #[serde(default)]
                defaulted: bool,
                values: Vec<Option<String>>,
                event_json: String,
                #[serde(flatten)]
                row: SessionAgentMessageRowDto,
            },
            Unit,
        "#,
        );
        let variants = variants(&source, "define_client_event", "ClientEvent").unwrap();
        let (event, names) =
            render_variants("ClientEvent", &variants, false, &mut BTreeSet::new()).unwrap();
        assert_eq!(names, ["renamed_event", "unit"]);
        assert!(event.contains("type: 'renamed_event'; actual_key: string; omitted?: string; nullable: string | null; defaulted: boolean; values: Array<string | null>; event_json: string } & SessionAgentMessageRowDto"), "{event}");
        let (command, _) =
            render_variants("ClientCommand", &variants, true, &mut BTreeSet::new()).unwrap();
        assert!(
            command
                .contains("omitted?: string | null; nullable?: string | null; defaulted?: boolean"),
            "{command}"
        );
    }

    #[test]
    fn changes_to_variants_and_fields_change_generated_output() {
        let first = fixture("Old { value: String },");
        let second = fixture("Old { value: u64 }, Added,");
        let (old, old_names) = render_variants(
            "ClientEvent",
            &variants(&first, "define_client_event", "ClientEvent").unwrap(),
            false,
            &mut BTreeSet::new(),
        )
        .unwrap();
        let (new, new_names) = render_variants(
            "ClientEvent",
            &variants(&second, "define_client_event", "ClientEvent").unwrap(),
            false,
            &mut BTreeSet::new(),
        )
        .unwrap();
        assert!(old.contains("value: string"));
        assert!(new.contains("value: number"));
        assert_ne!(old, new);
        assert_eq!(old_names, ["old"]);
        assert_eq!(new_names, ["old", "added"]);
    }

    #[test]
    fn rejects_unknown_serde_and_cfg_instead_of_guessing() {
        for body in [
            r#"A { #[serde(with = "custom")] value: String },"#,
            r#"A { #[cfg(feature = "future")] value: String },"#,
            r#"A { #[cfg_attr(feature = "future", serde(rename = "other"))] value: String },"#,
            r#"A { #[serde(skip_serializing_if = "is_false")] value: String },"#,
            r#"#[serde(rename = "dash-name")] A,"#,
            r#"A { #[serde(skip_serializing_if = "Option::is_none")] value: Option<Option<String>> },"#,
        ] {
            let source = fixture(body);
            let variants = variants(&source, "define_client_event", "ClientEvent").unwrap();
            assert!(
                render_variants("ClientEvent", &variants, false, &mut BTreeSet::new()).is_err(),
                "accepted {body}"
            );
        }
    }

    #[test]
    fn rejects_changed_macro_tag_and_cfg_dependent_enum() {
        let file = fixture("A,");
        let mut definition = macro_item(&file, "define_client_event", true)
            .unwrap()
            .clone();
        definition.mac.tokens = definition
            .mac
            .tokens
            .to_string()
            .replace("tag = \"type\"", "tag = \"kind\"")
            .parse()
            .unwrap();
        let attrs = emitted_enum_attrs(definition.mac.tokens, "ClientEvent")
            .unwrap()
            .unwrap();
        assert_ne!(serde_attrs(&attrs).unwrap().tag.as_deref(), Some("type"));
        let missing_derive = syn::parse_file("struct S { value: String }").unwrap();
        let structure = struct_item(&missing_derive, "S").unwrap();
        assert!(require_derive(&structure.attrs, "S", "Serialize").is_err());
        let file = syn::parse_file(r#"#[cfg_attr(feature = "new", serde(rename_all = "camelCase"))] pub struct S { value: String }"#).unwrap();
        assert!(render_struct(struct_item(&file, "S").unwrap(), &mut BTreeSet::new()).is_err());
    }

    #[test]
    fn skipped_non_option_command_field_is_not_optional_without_default() {
        let source =
            fixture(r#"A { #[serde(skip_serializing_if = "Vec::is_empty")] ids: Vec<String> },"#);
        let variants = variants(&source, "define_client_event", "ClientEvent").unwrap();
        let (command, _) =
            render_variants("ClientCommand", &variants, true, &mut BTreeSet::new()).unwrap();
        let (event, _) =
            render_variants("ClientEvent", &variants, false, &mut BTreeSet::new()).unwrap();
        assert!(command.contains("ids: Array<string>"));
        assert!(event.contains("ids?: Array<string>"));
    }

    fn command_fixture(field_attr: &str, wire_key: &str, fallback: &str) -> syn::File {
        syn::parse_file(&format!(r#"
            macro_rules! define_client_command {{
                ($($variants:tt)*) => {{
                    #[derive(Serialize)]
                    #[serde(tag = "type", rename_all = "snake_case")]
                    pub enum ClientCommand {{ $($variants)* }}
                    #[derive(Deserialize)]
                    #[serde(rename_all = "snake_case")]
                    enum ClientCommandWire {{ $($variants)* }}
                }};
            }}
            define_client_command! {{ SendPrompt {{ {field_attr} images: Vec<String> }}, }}
            impl<'de> Deserialize<'de> for ClientCommand {{
                fn deserialize<D>() {{
                    match tag.as_str() {{
                        "send_prompt" => Ok(Self::SendPrompt {{ images: command_raw_required(&mut fields, "{wire_key}")? }}),
                        _ => {fallback},
                    }}
                }}
            }}
        "#)).unwrap()
    }

    #[test]
    fn custom_command_decoder_must_match_the_declared_field_contract() {
        let fallback = r#"{ let json = command_raw_wire_json::<D::Error>(&tag, fields)?;
            let wire: ClientCommandWire = serde_json::from_str(&json).map_err(serde::de::Error::custom)?;
            Ok(wire.into()) }"#;
        for (attribute, key, expected) in [
            ("", "images", true),
            ("#[serde(default)]", "images", false),
            ("#[serde(rename = \"other\")]", "images", false),
        ] {
            let source = command_fixture(attribute, key, fallback);
            let variants = variants(&source, "define_client_command", "ClientCommand").unwrap();
            assert_eq!(
                validate_command_decoder(&source, &variants).is_ok(),
                expected
            );
        }
        let source = command_fixture("", "images", "{ Ok(unchecked) }");
        let variants = variants(&source, "define_client_command", "ClientCommand").unwrap();
        assert!(validate_command_decoder(&source, &variants).is_err());
    }

    #[test]
    fn credential_wrapper_must_remain_transparent_string() {
        for (attribute, ty, expected) in [
            ("#[serde(transparent)]", "String", true),
            ("", "String", false),
            ("#[serde(transparent)]", "u64", false),
        ] {
            let source = syn::parse_file(&format!("#[derive(Serialize, Deserialize)] {attribute} struct ProviderCredentialSecretDto {{ value: {ty} }}")).unwrap();
            assert_eq!(validate_credential_wrapper(&source).is_ok(), expected);
        }
    }
}
