//! Derive macros for bevue.

use proc_macro::TokenStream;
use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::quote;
use syn::{
    Data, DeriveInput, Fields, Ident, Index, LitStr, Type, parse_macro_input, parse_quote, spanned::Spanned,
};

/// Derives `bevue::core::props::Props`.
///
/// - Structs with named fields become dicts keyed by field name.
/// - Tuple structs become lists.
/// - Enums with only unit variants become strings holding the variant name.
///
/// Type attributes:
/// - `#[props(name = "name")]` sets `Props::name`, the key the type is mounted at in template props
///   by default. Without it, the name is the type name in snake case: `PlayerInventory` is
///   `player_inventory`.
///
/// Field and variant attributes:
/// - `#[props(rename = "name")]` uses another key or variant string.
/// - `#[props(skip)]` leaves a field out; it must implement `Default`.
#[proc_macro_derive(Props, attributes(props))]
pub fn derive_props(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match expand(input) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

#[derive(Default)]
struct Attrs {
    skip: bool,
    rename: Option<String>,
}

fn parse_attrs(attrs: &[syn::Attribute]) -> syn::Result<Attrs> {
    let mut out = Attrs::default();
    for attr in attrs.iter().filter(|a| a.path().is_ident("props")) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("skip") {
                out.skip = true;
                Ok(())
            } else if meta.path.is_ident("rename") {
                out.rename = Some(meta.value()?.parse::<LitStr>()?.value());
                Ok(())
            } else {
                Err(meta.error("expected `skip` or `rename = \"...\"`"))
            }
        })?;
    }
    Ok(out)
}

/// Parses the attributes of the type itself, returning its `name` if set.
fn parse_container_attrs(attrs: &[syn::Attribute]) -> syn::Result<Option<LitStr>> {
    let mut name = None;
    for attr in attrs.iter().filter(|a| a.path().is_ident("props")) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("name") {
                let value = meta.value()?.parse::<LitStr>()?;
                if value.value().trim().is_empty() {
                    return Err(syn::Error::new(value.span(), "`name` cannot be empty"));
                }
                name = Some(value);
                Ok(())
            } else {
                Err(meta.error("expected `name = \"...\"`"))
            }
        })?;
    }
    Ok(name)
}

/// Converts a type name to snake case: `PlayerInventory` becomes `player_inventory`.
fn to_snake_case(name: &str) -> String {
    let chars: Vec<char> = name.chars().collect();
    let mut out = String::new();
    for (i, &c) in chars.iter().enumerate() {
        let previous = i.checked_sub(1).map(|p| chars[p]);
        let next_is_lower = chars.get(i + 1).is_some_and(|n| n.is_lowercase());
        let boundary = previous.is_some_and(|p| p.is_lowercase() || p.is_numeric() || (p.is_uppercase() && next_is_lower));
        if c.is_uppercase() && boundary {
            out.push('_');
        }
        out.extend(c.to_lowercase());
    }
    out
}

fn expand(mut input: DeriveInput) -> syn::Result<TokenStream2> {
    let props = quote!(::bevue::core::props);
    let value = quote!(::bevue::core::value::Value);

    let props_name = parse_container_attrs(&input.attrs)?
        .unwrap_or_else(|| LitStr::new(&to_snake_case(&input.ident.to_string()), input.ident.span()));

    let type_params: Vec<Ident> = input.generics.type_params().map(|p| p.ident.clone()).collect();
    let where_clause = input.generics.make_where_clause();
    for param in &type_params {
        where_clause.predicates.push(parse_quote!(#param: #props::Props));
    }
    let name = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    let body = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(fields) => {
                let mut keys = Vec::new();
                let mut idents = Vec::new();
                let mut types: Vec<&Type> = Vec::new();
                let mut skipped = Vec::new();
                for field in &fields.named {
                    let attrs = parse_attrs(&field.attrs)?;
                    let ident = field.ident.clone().unwrap();
                    if attrs.skip {
                        skipped.push(ident);
                        continue;
                    }
                    keys.push(LitStr::new(&attrs.rename.unwrap_or_else(|| ident.to_string()), ident.span()));
                    idents.push(ident);
                    types.push(&field.ty);
                }
                quote! {
                    fn to_value(&self) -> #value {
                        let mut map = ::bevue::__private::HashMap::<::std::string::String, #value>::new();
                        #( map.insert(::std::string::String::from(#keys), #props::Props::to_value(&self.#idents)); )*
                        #value::dict(map)
                    }
                    fn sync_value(&self, target: &mut #value) {
                        if target.as_dict().is_none() {
                            target.set(#props::Props::to_value(self));
                            return;
                        }
                        #( #props::sync_field(target, #keys, &self.#idents); )*
                    }
                    fn apply_value(&mut self, value: &#value) -> ::core::result::Result<(), #props::PropsError> {
                        let dict = value.as_dict().ok_or_else(|| #props::PropsError::expected("dict", value))?;
                        #(
                            if let ::core::option::Option::Some(v) = dict.get(#keys) {
                                #props::Props::apply_value(&mut self.#idents, v).map_err(|e| e.in_field(#keys))?;
                            }
                        )*
                        ::core::result::Result::Ok(())
                    }
                    fn from_value(value: &#value) -> ::core::result::Result<Self, #props::PropsError> {
                        ::core::result::Result::Ok(Self {
                            #(
                                #idents: <#types as #props::Props>::from_value(#props::require_field(value, #keys)?)
                                    .map_err(|e| e.in_field(#keys))?,
                            )*
                            #( #skipped: ::core::default::Default::default(), )*
                        })
                    }
                    fn kind(&self) -> #props::PropsKind {
                        #props::PropsKind::Struct
                    }
                    fn field(&self, name: &str) -> ::core::option::Option<&dyn #props::Props> {
                        match name {
                            #( #keys => ::core::option::Option::Some(&self.#idents), )*
                            _ => ::core::option::Option::None,
                        }
                    }
                    fn field_mut(&mut self, name: &str) -> ::core::option::Option<&mut dyn #props::Props> {
                        match name {
                            #( #keys => ::core::option::Option::Some(&mut self.#idents), )*
                            _ => ::core::option::Option::None,
                        }
                    }
                }
            }
            Fields::Unnamed(fields) => {
                let mut indices = Vec::new();
                let mut positions = Vec::new();
                let mut types: Vec<&Type> = Vec::new();
                for (i, field) in fields.unnamed.iter().enumerate() {
                    if parse_attrs(&field.attrs)?.skip {
                        return Err(syn::Error::new(field.span(), "`skip` is not supported on tuple struct fields"));
                    }
                    indices.push(Index::from(i));
                    positions.push(i);
                    types.push(&field.ty);
                }
                let len = indices.len();
                quote! {
                    fn to_value(&self) -> #value {
                        #value::list(::std::vec![ #( #props::Props::to_value(&self.#indices) ),* ])
                    }
                    fn sync_value(&self, target: &mut #value) {
                        if target.as_list().map(|l| l.len()) != ::core::option::Option::Some(#len) {
                            target.set(#props::Props::to_value(self));
                            return;
                        }
                        #( #props::sync_element(target, #positions, &self.#indices); )*
                    }
                    fn apply_value(&mut self, value: &#value) -> ::core::result::Result<(), #props::PropsError> {
                        let list = value.as_list().ok_or_else(|| #props::PropsError::expected("list", value))?;
                        #(
                            if let ::core::option::Option::Some(v) = list.get(#positions) {
                                #props::Props::apply_value(&mut self.#indices, v)
                                    .map_err(|e| e.in_field(::std::format!("[{}]", #positions)))?;
                            }
                        )*
                        ::core::result::Result::Ok(())
                    }
                    fn from_value(value: &#value) -> ::core::result::Result<Self, #props::PropsError> {
                        let list = value.as_list().ok_or_else(|| #props::PropsError::expected("list", value))?;
                        if list.len() != #len {
                            return ::core::result::Result::Err(#props::PropsError::TypeMismatch {
                                expected: "list of matching length",
                                found: ::std::string::ToString::to_string(value),
                            });
                        }
                        ::core::result::Result::Ok(Self(
                            #(
                                <#types as #props::Props>::from_value(&list[#positions])
                                    .map_err(|e| e.in_field(::std::format!("[{}]", #positions)))?,
                            )*
                        ))
                    }
                    fn kind(&self) -> #props::PropsKind {
                        #props::PropsKind::List
                    }
                    fn element(&self, index: usize) -> ::core::option::Option<&dyn #props::Props> {
                        match index {
                            #( #positions => ::core::option::Option::Some(&self.#indices), )*
                            _ => ::core::option::Option::None,
                        }
                    }
                    fn element_mut(&mut self, index: usize) -> ::core::option::Option<&mut dyn #props::Props> {
                        match index {
                            #( #positions => ::core::option::Option::Some(&mut self.#indices), )*
                            _ => ::core::option::Option::None,
                        }
                    }
                    fn element_count(&self) -> usize {
                        #len
                    }
                }
            }
            Fields::Unit => quote! {
                fn to_value(&self) -> #value {
                    #value::empty_dict()
                }
                fn apply_value(&mut self, _value: &#value) -> ::core::result::Result<(), #props::PropsError> {
                    ::core::result::Result::Ok(())
                }
                fn from_value(_value: &#value) -> ::core::result::Result<Self, #props::PropsError> {
                    ::core::result::Result::Ok(Self)
                }
                fn kind(&self) -> #props::PropsKind {
                    #props::PropsKind::Struct
                }
            },
        },
        Data::Enum(data) => {
            let mut variants = Vec::new();
            let mut names = Vec::new();
            for variant in &data.variants {
                if !matches!(variant.fields, Fields::Unit) {
                    return Err(syn::Error::new(
                        variant.span(),
                        "`Props` can only be derived for enums whose variants have no fields",
                    ));
                }
                let attrs = parse_attrs(&variant.attrs)?;
                names.push(LitStr::new(
                    &attrs.rename.unwrap_or_else(|| variant.ident.to_string()),
                    variant.ident.span(),
                ));
                variants.push(&variant.ident);
            }
            quote! {
                fn to_value(&self) -> #value {
                    #value::from(match self {
                        #( Self::#variants => #names, )*
                    })
                }
                fn apply_value(&mut self, value: &#value) -> ::core::result::Result<(), #props::PropsError> {
                    *self = <Self as #props::Props>::from_value(value)?;
                    ::core::result::Result::Ok(())
                }
                fn from_value(value: &#value) -> ::core::result::Result<Self, #props::PropsError> {
                    let name = value.as_string().ok_or_else(|| #props::PropsError::expected("string", value))?;
                    match name.as_str() {
                        #( #names => ::core::result::Result::Ok(Self::#variants), )*
                        other => ::core::result::Result::Err(#props::PropsError::UnknownVariant(
                            ::std::string::ToString::to_string(other),
                        )),
                    }
                }
            }
        }
        Data::Union(_) => {
            return Err(syn::Error::new(Span::call_site(), "`Props` cannot be derived for unions"));
        }
    };

    Ok(quote! {
        impl #impl_generics #props::Props for #name #ty_generics #where_clause {
            #body

            fn as_any(&self) -> &dyn ::core::any::Any {
                self
            }
            fn as_any_mut(&mut self) -> &mut dyn ::core::any::Any {
                self
            }
            fn name() -> &'static str {
                #props_name
            }
        }
    })
}
