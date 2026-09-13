//! Procedural macros for Rango: the `Model` derive that turns a struct into a stored row, plus the
//! `form`, `input`, `template`, and `main` attribute macros that wire serde, Askama, and tokio.

#![warn(missing_docs)]

use proc_macro::TokenStream;
use quote::quote;
use syn::{Data, DeriveInput, Expr, ExprLit, Fields, ItemFn, LitStr};

fn with_serde(mut input: DeriveInput, default: bool) -> TokenStream {
    let derives = if default {
        quote!(rango::prelude::Deserialize, Default)
    } else {
        quote!(rango::prelude::Deserialize)
    };
    input.attrs.push(syn::parse_quote!(#[derive(#derives)]));
    input
        .attrs
        .push(syn::parse_quote!(#[serde(crate = "rango::serde")]));
    quote!(#input).into()
}

/// Derives `Deserialize` and `Default` for a form struct, using `rango::serde`.
///
/// Usage: `#[rango::form]` on a struct.
#[proc_macro_attribute]
pub fn form(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(item as DeriveInput);
    with_serde(input, true)
}

/// Derives only `Deserialize` for an input struct, using `rango::serde`.
///
/// Usage: `#[rango::input]` on a struct.
#[proc_macro_attribute]
pub fn input(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(item as DeriveInput);
    with_serde(input, false)
}

/// Runs an `async fn main` on a multi-threaded tokio runtime with all features enabled.
///
/// Usage: `#[rango::main]` on `async fn main() -> ExitCode`.
#[proc_macro_attribute]
pub fn main(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let func = syn::parse_macro_input!(item as ItemFn);
    if func.sig.asyncness.is_none() {
        return syn::Error::new_spanned(&func.sig, "expected `async fn main`")
            .to_compile_error()
            .into();
    }
    let output = &func.sig.output;
    let body = &func.block;
    let attrs = &func.attrs;
    let vis = &func.vis;
    quote! {
        #(#attrs)*
        #vis fn main() #output {
            rango::tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("failed to build tokio runtime")
                .block_on(async #body)
        }
    }
    .into()
}

/// Derives `Template` via Askama, forwarding the attribute arguments unchanged.
///
/// Usage: `#[rango::template(path = "index.html")]`.
#[proc_macro_attribute]
pub fn template(attr: TokenStream, item: TokenStream) -> TokenStream {
    let attr: proc_macro2::TokenStream = attr.into();
    let mut input = syn::parse_macro_input!(item as DeriveInput);
    input
        .attrs
        .push(syn::parse_quote!(#[derive(rango::prelude::Template)]));
    input
        .attrs
        .push(syn::parse_quote!(#[template(#attr, askama = rango::askama)]));
    quote!(#input).into()
}

/// Implements `Model` for a struct, mapping each field to a column.
///
/// Type-level `#[model(table = "messages", actions = "duplicate")]` sets the table name
/// (defaults to the lowercased type name plus `s`) and lists row-action functions.
/// Field attributes: `#[key]`, `#[references("table.column")]`, `#[via("through.mine.theirs")]`,
/// `#[default(value)]`.
#[proc_macro_derive(Model, attributes(model, key, references, via, default))]
pub fn derive_model(input: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(input as DeriveInput);
    match expand(&input) {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

fn expand(input: &DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let name = &input.ident;
    let (table, deeds) = parse_model(input)?;

    let fields = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(fields) => &fields.named,
            _ => return Err(syn::Error::new_spanned(input, "only named fields")),
        },
        _ => return Err(syn::Error::new_spanned(input, "only structs")),
    };

    let mut field_defs = Vec::new();
    let mut write_parts = Vec::new();
    let mut read_parts = Vec::new();
    let mut key_ident = None;

    for field in fields.into_iter() {
        let ident = field.ident.as_ref().unwrap();
        let attrs = &field.attrs;
        let ty = &field.ty;

        let is_key = attrs.iter().any(|a| a.path().is_ident("key"));
        let references = parse_references(attrs)?;
        let references = references.as_deref();
        let via = parse_via(attrs)?;
        let default = parse_default(attrs)?;

        let (field_type, is_optional) = unpack_option(ty);
        let (cell_type, is_many) = unpack_vec(field_type);
        if is_many && is_optional {
            return Err(syn::Error::new_spanned(
                field,
                "optional lists need plain Vec",
            ));
        }
        if is_many && is_key {
            return Err(syn::Error::new_spanned(field, "key needs a single value"));
        }
        let name = ident.to_string();
        let is_id = is_key && name == "id" && !is_many;

        let field_def = if is_id {
            quote! { rango::Field::id() }
        } else if is_key {
            let mut def = quote! { rango::Field::key::<#cell_type>(#name) };
            if let Some(ref_path) = references {
                def = quote! { #def.references(#ref_path) };
            }
            def
        } else if is_many {
            let mut def = quote! { rango::Field::many(#name) };
            if let Some((through, mine, theirs)) = via {
                def = quote! {
                    #def.link(rango::Link::Via(
                        rango::Table(#through),
                        rango::Name(#mine),
                        rango::Name(#theirs)
                    ))
                };
            }
            def
        } else {
            let mut def = quote! { rango::Field::cell::<#cell_type>(#name) };
            if is_optional {
                def = quote! { #def.optional() };
            }
            if let Some(ref_path) = references {
                def = quote! { #def.references(#ref_path) };
            }
            if let Some(default_val) = default {
                def = quote! { #def.default_value(#default_val) };
            }
            def
        };

        field_defs.push(field_def);

        if is_many {
            read_parts.push(quote! { #ident: Vec::new() });
            continue;
        }
        if is_key {
            key_ident = Some(ident);
        }
        if !is_id {
            write_parts.push(quote! { rango::Storable::put(&self.#ident, w); });
        }
        read_parts.push(quote! { #ident: rango::Storable::take(r)? });
    }

    let write_id = match &key_ident {
        Some(ident) => quote! { rango::Storable::put(&self.#ident, w); },
        None => quote! { w.nothing(); },
    };
    let read_id = match &key_ident {
        Some(ident) => quote! {
            self.#ident = rango::Storable::take(r)?;
            Ok(())
        },
        None => quote! { Ok(()) },
    };
    let actions = if deeds.is_empty() {
        quote! {}
    } else {
        quote! {
            fn actions() -> Vec<rango::Action> {
                vec![#(#deeds()),*, rango::Action::wipe()]
            }
        }
    };

    Ok(quote! {
        impl rango::Model for #name {
            fn table() -> rango::Table {
                rango::Table(#table)
            }

            fn fields() -> Vec<rango::Field> {
                vec![#(#field_defs),*]
            }

            fn write(&self, w: &mut dyn rango::Writer) {
                #(#write_parts)*
            }

            fn read(r: &mut dyn rango::Reader) -> Result<Self, rango::StoreError> {
                Ok(Self {
                    #(#read_parts),*
                })
            }

            fn write_id(&self, w: &mut dyn rango::Writer) {
                #write_id
            }

            fn read_id(&mut self, r: &mut dyn rango::Reader) -> Result<(), rango::StoreError> {
                #read_id
            }

            #actions
        }
    })
}

fn parse_model(input: &DeriveInput) -> syn::Result<(String, Vec<syn::Path>)> {
    let mut table = None;
    let mut deeds = Vec::new();
    for attr in &input.attrs {
        if !attr.path().is_ident("model") {
            continue;
        }
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("table") {
                let value = meta.value()?;
                let lit: LitStr = value.parse()?;
                table = Some(lit.value());
                Ok(())
            } else if meta.path.is_ident("actions") {
                let value = meta.value()?;
                let lit: LitStr = value.parse()?;
                for raw in lit.value().split(',') {
                    let raw = raw.trim();
                    if raw.is_empty() {
                        continue;
                    }
                    deeds.push(
                        syn::parse_str::<syn::Path>(raw)
                            .map_err(|_| meta.error("expected action paths"))?,
                    );
                }
                Ok(())
            } else {
                Err(meta.error("expected `table = \"...\"` or `actions = \"...\"`"))
            }
        })?;
    }
    let name = input.ident.to_string().to_lowercase();
    Ok((table.unwrap_or(format!("{name}s")), deeds))
}

fn parse_references(attrs: &[syn::Attribute]) -> syn::Result<Option<String>> {
    for attr in attrs {
        if !attr.path().is_ident("references") {
            continue;
        }
        let lit: LitStr = attr.parse_args()?;
        return Ok(Some(lit.value()));
    }
    Ok(None)
}

fn parse_via(attrs: &[syn::Attribute]) -> syn::Result<Option<(String, String, String)>> {
    for attr in attrs {
        if !attr.path().is_ident("via") {
            continue;
        }
        let lit: LitStr = attr.parse_args()?;
        let text = lit.value();
        let parts: Vec<&str> = text.split('.').collect();
        if let [through, mine, theirs] = parts.as_slice() {
            return Ok(Some((
                through.to_string(),
                mine.to_string(),
                theirs.to_string(),
            )));
        }
        return Err(syn::Error::new_spanned(
            attr,
            "expected `#[via(\"through.mine.theirs\")]`",
        ));
    }
    Ok(None)
}

fn parse_default(attrs: &[syn::Attribute]) -> syn::Result<Option<proc_macro2::TokenStream>> {
    for attr in attrs {
        if !attr.path().is_ident("default") {
            continue;
        }
        let expr: Expr = attr.parse_args()?;
        if let Expr::Lit(ExprLit {
            lit: syn::Lit::Str(s),
            ..
        }) = expr
        {
            let val = s.value();
            return Ok(Some(quote! { String::from(#val) }));
        }
        return Ok(Some(quote! { #expr }));
    }
    Ok(None)
}

fn unpack_option(ty: &syn::Type) -> (&syn::Type, bool) {
    if let syn::Type::Path(tp) = ty
        && let Some(segment) = tp.path.segments.last()
        && segment.ident == "Option"
        && let syn::PathArguments::AngleBracketed(args) = &segment.arguments
        && let Some(syn::GenericArgument::Type(inner)) = args.args.first()
    {
        return (inner, true);
    }
    (ty, false)
}

fn unpack_vec(ty: &syn::Type) -> (&syn::Type, bool) {
    if let syn::Type::Path(tp) = ty
        && let Some(segment) = tp.path.segments.last()
        && segment.ident == "Vec"
        && let syn::PathArguments::AngleBracketed(args) = &segment.arguments
        && let Some(syn::GenericArgument::Type(inner)) = args.args.first()
    {
        return (inner, true);
    }
    (ty, false)
}
