use proc_macro::TokenStream;
use quote::quote;
use syn::{Data, DeriveInput, Expr, ExprLit, Fields, LitStr};

#[proc_macro_derive(Model, attributes(model, key, references, default))]
pub fn derive_model(input: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(input as DeriveInput);
    match expand(&input) {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

fn expand(input: &DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let name = &input.ident;
    let table = parse_table(input)?;

    let fields = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(fields) => &fields.named,
            _ => return Err(syn::Error::new_spanned(input, "only named fields")),
        },
        _ => return Err(syn::Error::new_spanned(input, "only structs")),
    };

    let mut field_defs = Vec::new();
    let mut row_exprs = Vec::new();
    let mut from_fields = Vec::new();
    let mut set_id_stmt = None;
    let mut id_expr = None;

    for (idx, field) in fields.into_iter().enumerate() {
        let ident = field.ident.as_ref().unwrap();
        let attrs = &field.attrs;
        let ty = &field.ty;

        let is_key = attrs.iter().any(|a| a.path().is_ident("key"));
        let references = parse_references(attrs)?;
        let default = parse_default(attrs)?;

        let (field_type, is_optional) = unpack_option(ty);
        let type_path = type_string(field_type);

        let kind = kind_of(&type_path);
        let name = ident.to_string();
        let is_id = is_key && name == "id" && kind == Kind::Int;

        let field_def = if is_id {
            quote! { rango::Field::id() }
        } else if is_key {
            quote! { rango::Field::key(#name) }
        } else {
            let mut def = match kind {
                Kind::Str | Kind::Other => {
                    quote! { rango::Field::new(#name, rango::Type::Str) }
                }
                Kind::Int => quote! { rango::Field::new(#name, rango::Type::Int) },
                Kind::Float => quote! { rango::Field::new(#name, rango::Type::Float) },
                Kind::Bool => quote! { rango::Field::new(#name, rango::Type::Bool) },
                Kind::Date => quote! { rango::Field::new(#name, rango::Type::DateTime) },
                Kind::Decimal => quote! { rango::Field::new(#name, rango::Type::Decimal) },
            };
            if is_optional {
                def = quote! { #def.optional() };
            }
            if let Some(ref_path) = references {
                def = quote! { #def.references(#ref_path) };
            }
            if let Some(default_val) = default {
                def = quote! { #def.default(#default_val) };
            }
            def
        };

        let field_idx = syn::Index::from(idx);

        let row_expr = match kind {
            Kind::Str => {
                if is_optional {
                    quote! { rango::Value::from(&self.#ident) }
                } else {
                    quote! { rango::Value::str(&self.#ident) }
                }
            }
            Kind::Other => {
                if is_optional {
                    quote! {
                        match &self.#ident {
                            Some(v) => rango::Value::str(v),
                            None => rango::Value::Null,
                        }
                    }
                } else {
                    quote! { rango::Value::str(&self.#ident) }
                }
            }
            Kind::Int => {
                if is_optional {
                    quote! {
                        match self.#ident {
                            Some(v) => rango::Value::int(v as i64),
                            None => rango::Value::Null,
                        }
                    }
                } else {
                    quote! { rango::Value::int(self.#ident as i64) }
                }
            }
            Kind::Float => {
                if is_optional {
                    quote! {
                        match self.#ident {
                            Some(v) => rango::Value::float(v as f64),
                            None => rango::Value::Null,
                        }
                    }
                } else {
                    quote! { rango::Value::float(self.#ident as f64) }
                }
            }
            Kind::Bool => {
                if is_optional {
                    quote! {
                        match self.#ident {
                            Some(v) => rango::Value::bool(v),
                            None => rango::Value::Null,
                        }
                    }
                } else {
                    quote! { rango::Value::bool(self.#ident) }
                }
            }
            Kind::Date => {
                if is_optional {
                    quote! {
                        match self.#ident {
                            Some(v) => rango::Value::datetime(v),
                            None => rango::Value::Null,
                        }
                    }
                } else {
                    quote! { rango::Value::datetime(self.#ident) }
                }
            }
            Kind::Decimal => {
                if is_optional {
                    quote! {
                        match self.#ident {
                            Some(v) => rango::Value::decimal(v),
                            None => rango::Value::Null,
                        }
                    }
                } else {
                    quote! { rango::Value::decimal(self.#ident) }
                }
            }
        };

        let from_expr = if is_key {
            match kind {
                Kind::Int => quote! { row.int(#field_idx)? as _ },
                _ => quote! { row.str(#field_idx)? },
            }
        } else if is_optional {
            match kind {
                Kind::Str | Kind::Other => quote! { row.opt_str(#field_idx) },
                Kind::Int => {
                    quote! {
                        match row.values.get(#field_idx) {
                            Some(rango::Value::Int(v)) => Some(*v as _),
                            _ => None,
                        }
                    }
                }
                Kind::Float => {
                    quote! {
                        match row.values.get(#field_idx) {
                            Some(rango::Value::Float(v)) => Some(*v as _),
                            _ => None,
                        }
                    }
                }
                Kind::Bool => {
                    quote! {
                        match row.values.get(#field_idx) {
                            Some(rango::Value::Bool(v)) => Some(*v),
                            _ => None,
                        }
                    }
                }
                Kind::Date => {
                    quote! {
                        match row.values.get(#field_idx) {
                            Some(rango::Value::DateTime(v)) => Some(*v),
                            _ => None,
                        }
                    }
                }
                Kind::Decimal => {
                    quote! {
                        match row.values.get(#field_idx) {
                            Some(rango::Value::Decimal(v)) => Some(*v),
                            _ => None,
                        }
                    }
                }
            }
        } else {
            match kind {
                Kind::Str | Kind::Other => quote! { row.str(#field_idx)? },
                Kind::Int => quote! { row.int(#field_idx)? as _ },
                Kind::Float => quote! { row.float(#field_idx)? as _ },
                Kind::Bool => quote! { row.bool(#field_idx)? },
                Kind::Date => quote! { row.datetime(#field_idx)? },
                Kind::Decimal => quote! { row.decimal(#field_idx)? },
            }
        };

        if is_key {
            match kind {
                Kind::Int => {
                    set_id_stmt = Some(quote! {
                        if let rango::Value::Int(id) = id {
                            self.#ident = id as _;
                        }
                    });
                    id_expr = Some(quote! { rango::Value::int(self.#ident as i64) });
                }
                _ => {
                    set_id_stmt = Some(quote! {
                        if let rango::Value::Str(id) = id {
                            self.#ident = id;
                        }
                    });
                    id_expr = Some(quote! { rango::Value::str(&self.#ident) });
                }
            }
        }

        field_defs.push(field_def);
        if !is_id {
            row_exprs.push(row_expr);
        }
        from_fields.push(quote! { #ident: #from_expr });
    }

    let set_id = set_id_stmt.unwrap_or_else(|| quote! { let _ = id; });
    let id = id_expr.unwrap_or_else(|| quote! { rango::Value::Null });

    Ok(quote! {
        impl rango::Model for #name {
            fn table() -> &'static str {
                #table
            }

            fn fields() -> Vec<rango::Field> {
                vec![#(#field_defs),*]
            }

            fn row(&self) -> Vec<rango::Value> {
                vec![#(#row_exprs),*]
            }

            fn from_row(row: &rango::Row) -> Result<Self, rango::StoreError> {
                Ok(Self { #(#from_fields),* })
            }

            fn set_id(&mut self, id: rango::Value) {
                #set_id
            }

            fn id(&self) -> rango::Value {
                #id
            }
        }
    })
}

fn parse_table(input: &DeriveInput) -> syn::Result<String> {
    for attr in &input.attrs {
        if !attr.path().is_ident("model") {
            continue;
        }
        let mut table = None;
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("table") {
                let value = meta.value()?;
                let lit: LitStr = value.parse()?;
                table = Some(lit.value());
                Ok(())
            } else {
                Err(meta.error("expected `table = \"...\"`"))
            }
        })?;
        if let Some(t) = table {
            return Ok(t);
        }
    }
    let name = input.ident.to_string().to_lowercase();
    Ok(format!("{name}s"))
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
            return Ok(Some(quote! { rango::Value::str(#val) }));
        }
        return Ok(Some(quote! { rango::Value::from(#expr) }));
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

fn type_string(ty: &syn::Type) -> String {
    if let syn::Type::Path(tp) = ty {
        let segments: Vec<String> = tp
            .path
            .segments
            .iter()
            .map(|s| s.ident.to_string())
            .collect();
        segments.join("::")
    } else {
        "String".into()
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Str,
    Int,
    Float,
    Bool,
    Date,
    Decimal,
    Other,
}

fn kind_of(name: &str) -> Kind {
    match name {
        "String" | "str" => Kind::Str,
        "i64" | "i32" | "i16" | "i8" | "u64" | "u32" | "u16" | "u8" | "isize" | "usize" => {
            Kind::Int
        }
        "f64" | "f32" => Kind::Float,
        "bool" => Kind::Bool,
        "DateTime" | "DateTime<Utc>" => Kind::Date,
        "Decimal" => Kind::Decimal,
        _ => Kind::Other,
    }
}
