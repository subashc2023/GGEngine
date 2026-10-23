//! The derives that `gg-engine` re-exports beside its traits of the same names: `Reflect`, which
//! describes a type to the engine's reflection, and `Component`, which makes it a component too.
//! Rust builds a derive as a crate of its own; games use them from `gg-engine`, as
//! `#[derive(Clone, gg_engine::Component)]`, and the code they write names everything through
//! `::gg_engine`.

use proc_macro2::{Literal, Span, TokenStream, TokenTree};
use quote::{ToTokens, format_ident, quote, quote_spanned};
use syn::ext::IdentExt;
use syn::meta::ParseNestedMeta;
use syn::spanned::Spanned;
use syn::{Attribute, Data, DeriveInput, Expr, Fields, Ident, Lit, LitStr, Member, Meta, Token};

/// Describes the type to the engine's reflection: `gg_engine::reflect::Reflect` says how.
#[proc_macro_derive(Reflect, attributes(reflect))]
pub fn derive_reflect(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let input = syn::parse_macro_input!(input as DeriveInput);
    let expanded = expand(&input, false);
    expanded
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// Makes the type a component, which `Reflect` describes: `gg_engine::Component` says how.
#[proc_macro_derive(Component, attributes(reflect))]
pub fn derive_component(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let input = syn::parse_macro_input!(input as DeriveInput);
    let expanded = expand(&input, true);
    expanded
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// Where everything the derives write is.
fn reflect() -> TokenStream {
    quote!(::gg_engine::reflect)
}

fn expand(input: &DeriveInput, component: bool) -> syn::Result<TokenStream> {
    let derive = if component { "Component" } else { "Reflect" };
    if let Some(param) = input.generics.params.first() {
        return Err(syn::Error::new_spanned(
            param,
            format!(
                "`#[derive({derive})]` doesn't take generic types: reflection describes each \
                 type once, by name, and a generic type is a type for each of its arguments; \
                 use a type without parameters for each"
            ),
        ));
    }
    if let Data::Union(data) = &input.data {
        return Err(syn::Error::new(
            data.union_token.span,
            format!(
                "`#[derive({derive})]` doesn't take unions: reflection can't tell which of a \
                 union's fields holds its value; use an enum"
            ),
        ));
    }
    let hints = TypeHints::parse(&input.attrs)?;
    let ident = &input.ident;
    let name = match &hints.name {
        Some(name) => name.value(),
        None => ident.unraw().to_string(),
    };
    let doc = doc(&input.attrs);
    let mut checks = Vec::new();
    let methods = match (&input.data, hints.opaque) {
        (data, Some(hint)) => {
            unread(data)?;
            opaque(&name, &doc, hint)
        }
        (Data::Struct(data), None) => structure(&name, &doc, &data.fields, &mut checks)?,
        (Data::Enum(data), None) => enumeration(&name, &doc, &data.variants, &mut checks)?,
        (Data::Union(_), None) => unreachable!("unions are refused above"),
    };
    let r = reflect();
    let component = component.then(|| quote!(impl ::gg_engine::Component for #ident {}));
    Ok(quote! {
        #(#checks)*

        impl #r::Reflect for #ident {
            #methods

            fn set_value(&mut self, value: &#r::Value) -> #r::Result<()> {
                *self = <Self as #r::Reflect>::with_value(self, value)?;
                ::std::result::Result::Ok(())
            }
        }

        #component
    })
}

/// Refuses hints on the fields and variants of an opaque type, which reflection doesn't read.
fn unread(data: &Data) -> syn::Result<()> {
    let why = "an opaque type isn't read, so its fields and variants take no hints";
    let unhinted = |fields: &Fields| fields.iter().try_for_each(|f| no_hints(&f.attrs, why));
    match data {
        Data::Struct(data) => unhinted(&data.fields),
        Data::Enum(data) => data.variants.iter().try_for_each(|variant| {
            no_hints(&variant.attrs, why)?;
            unhinted(&variant.fields)
        }),
        Data::Union(_) => Ok(()),
    }
}

/// Refuses a `#[reflect(...)]` among `attrs`, at it, saying `why`.
fn no_hints(attrs: &[Attribute], why: &str) -> syn::Result<()> {
    match attrs.iter().find(|attr| attr.path().is_ident("reflect")) {
        Some(attr) => Err(syn::Error::new_spanned(attr, why)),
        None => Ok(()),
    }
}

/// The methods of a type marked `#[reflect(opaque)]`, by `hint`, which reflection keeps but
/// doesn't read.
fn opaque(name: &str, doc: &str, hint: Span) -> TokenStream {
    let r = reflect();
    // At the hint, so that a type that isn't `Clone` is reported there.
    let at = Span::call_site().located_at(hint);
    let keep = quote_spanned!(at=> ::gg_engine::reflect::keep(self, value));
    quote! {
        fn describe(types: &mut #r::Types) -> #r::Result<#r::Type> {
            types.add::<Self>(
                ::std::borrow::Cow::Borrowed(#name),
                ::std::borrow::Cow::Borrowed(#doc),
                |_| ::std::result::Result::Ok(#r::Shape::Opaque),
            )
        }

        fn to_value(&self) -> #r::Value {
            #r::Value::Opaque
        }

        fn from_value(value: &#r::Value) -> #r::Result<Self> {
            #r::make_opaque(value)
        }

        fn with_value(&self, value: &#r::Value) -> #r::Result<Self> {
            #keep
        }

        fn part(&self, _: ::core::primitive::u32) -> ::std::option::Option<&dyn #r::Reflect> {
            ::std::option::Option::None
        }

        fn part_mut(
            &mut self,
            _: ::core::primitive::u32,
        ) -> ::std::option::Option<&mut dyn #r::Reflect> {
            ::std::option::Option::None
        }
    }
}

/// A struct's methods: its fields described, read, made and kept, by name, and reached by index.
fn structure(
    name: &str,
    doc: &str,
    fields: &Fields,
    checks: &mut Vec<TokenStream>,
) -> syn::Result<TokenStream> {
    let r = reflect();
    let tuple = matches!(fields, Fields::Unnamed(_));
    let fields = Field::all(fields, checks)?;
    let types = types_binding(&fields);
    let described = fields.iter().map(Field::describe);
    let values = fields.iter().map(|field| {
        let member = &field.member;
        field.to_value(&quote!(&self.#member))
    });
    let bindings: Vec<_> = fields.iter().map(|field| &field.binding).collect();
    let names: Vec<_> = fields.iter().map(|field| &field.name).collect();
    let members: Vec<_> = fields.iter().map(|field| &field.member).collect();
    let made = fields.iter().map(Field::make);
    let kept = fields.iter().map(|field| {
        let member = &field.member;
        field.keep(&quote!(&self.#member))
    });
    let read: Vec<_> = (0u32..)
        .zip(&fields)
        .filter(|(_, field)| field.hints.opaque.is_none())
        .collect();
    let indices: Vec<_> = read
        .iter()
        .map(|(index, _)| Literal::u32_unsuffixed(*index))
        .collect();
    // At the field's type, as its tuple index is, so that a type that isn't reflected is reported
    // there for these as for its other uses, rather than at the derive.
    let parts = read.iter().map(|(_, field)| {
        let member = &field.member;
        quote_spanned!(at_type(field.ty)=> &self.#member as &dyn #r::Reflect)
    });
    let parts_mut = read.iter().map(|(_, field)| {
        let member = &field.member;
        quote_spanned!(at_type(field.ty)=> &mut self.#member as &mut dyn #r::Reflect)
    });
    Ok(quote! {
        fn describe(types: &mut #r::Types) -> #r::Result<#r::Type> {
            types.add::<Self>(
                ::std::borrow::Cow::Borrowed(#name),
                ::std::borrow::Cow::Borrowed(#doc),
                |#types| ::std::result::Result::Ok(#r::Shape::Struct {
                    fields: ::std::vec![#(#described),*],
                    tuple: #tuple,
                }),
            )
        }

        fn to_value(&self) -> #r::Value {
            #r::Value::Struct(::std::vec![#(#values),*])
        }

        fn from_value(value: &#r::Value) -> #r::Result<Self> {
            let [#(#bindings),*] = #r::fields(value, [#(#names),*])?;
            ::std::result::Result::Ok(Self { #(#members: #made),* })
        }

        fn with_value(&self, value: &#r::Value) -> #r::Result<Self> {
            let [#(#bindings),*] = #r::fields(value, [#(#names),*])?;
            ::std::result::Result::Ok(Self { #(#members: #kept),* })
        }

        fn part(
            &self,
            index: ::core::primitive::u32,
        ) -> ::std::option::Option<&dyn #r::Reflect> {
            match index {
                #(#indices => ::std::option::Option::Some(#parts),)*
                _ => ::std::option::Option::None,
            }
        }

        fn part_mut(
            &mut self,
            index: ::core::primitive::u32,
        ) -> ::std::option::Option<&mut dyn #r::Reflect> {
            match index {
                #(#indices => ::std::option::Option::Some(#parts_mut),)*
                _ => ::std::option::Option::None,
            }
        }
    })
}

/// An enum's methods: its variants described, read and made by name, and kept while a value keeps
/// its variant. An enum is set whole, so it has no parts.
fn enumeration(
    name: &str,
    doc: &str,
    variants: &syn::punctuated::Punctuated<syn::Variant, Token![,]>,
    checks: &mut Vec<TokenStream>,
) -> syn::Result<TokenStream> {
    let r = reflect();
    let mut described = Vec::new();
    let mut read = Vec::new();
    let mut made = Vec::new();
    let mut kept = Vec::new();
    let mut names = Vec::new();
    let mut described_fields = Vec::new();
    for variant in variants {
        no_hints(
            &variant.attrs,
            "a variant takes no hints: they go on its fields",
        )?;
        let ident = &variant.ident;
        let name = ident.unraw().to_string();
        let tuple = matches!(variant.fields, Fields::Unnamed(_));
        let fields = Field::all(&variant.fields, checks)?;
        let members: Vec<_> = fields.iter().map(|field| &field.member).collect();
        let bindings: Vec<_> = fields.iter().map(|field| &field.binding).collect();
        let field_names: Vec<_> = fields.iter().map(|field| &field.name).collect();

        let fields_described = fields.iter().map(Field::describe);
        described.push(quote! {
            #r::Variant {
                name: ::std::borrow::Cow::Borrowed(#name),
                fields: ::std::vec![#(#fields_described),*],
                tuple: #tuple,
            }
        });

        let patterns = fields.iter().map(|field| match field.hints.opaque {
            Some(_) => quote!(_),
            None => {
                let binding = &field.binding;
                quote!(ref #binding)
            }
        });
        let values = fields.iter().map(|field| field.to_value(&field.binding));
        read.push(quote! {
            Self::#ident { #(#members: #patterns),* } => #r::Value::Variant(
                ::std::borrow::Cow::Borrowed(#name),
                ::std::vec![#(#values),*],
            ),
        });

        let making = fields.iter().map(Field::make);
        made.push(quote! {
            #name => {
                let [#(#bindings),*] = #r::pick(given, [#(#field_names),*])?;
                ::std::result::Result::Ok(Self::#ident { #(#members: #making),* })
            }
        });

        if !fields.is_empty() {
            let current: Vec<_> = (0..fields.len())
                .map(|i| format_ident!("__self{}", i))
                .collect();
            let keeping = fields
                .iter()
                .zip(&current)
                .map(|(field, current)| field.keep(&current.to_token_stream()));
            kept.push(quote! {
                (Self::#ident { #(#members: #current),* }, #name) => {
                    let [#(#bindings),*] = #r::pick(given, [#(#field_names),*])?;
                    ::std::result::Result::Ok(Self::#ident { #(#members: #keeping),* })
                }
            });
        }
        names.push(name);
        described_fields.extend(fields);
    }
    let types = types_binding(&described_fields);
    let with_value = if kept.is_empty() {
        quote!(<Self as #r::Reflect>::from_value(value))
    } else {
        quote! {
            let (name, given) = #r::variant(value)?;
            match (self, name) {
                #(#kept)*
                _ => <Self as #r::Reflect>::from_value(value),
            }
        }
    };
    Ok(quote! {
        fn describe(types: &mut #r::Types) -> #r::Result<#r::Type> {
            types.add::<Self>(
                ::std::borrow::Cow::Borrowed(#name),
                ::std::borrow::Cow::Borrowed(#doc),
                |#types| ::std::result::Result::Ok(#r::Shape::Enum(::std::vec![#(#described),*])),
            )
        }

        fn to_value(&self) -> #r::Value {
            match *self {
                #(#read)*
            }
        }

        fn from_value(value: &#r::Value) -> #r::Result<Self> {
            let (name, given) = #r::variant(value)?;
            match name {
                #(#made)*
                _ => ::std::result::Result::Err(#r::no_variant(name, &[#(#names),*])),
            }
        }

        fn with_value(&self, value: &#r::Value) -> #r::Result<Self> {
            #with_value
        }

        fn part(&self, _: ::core::primitive::u32) -> ::std::option::Option<&dyn #r::Reflect> {
            ::std::option::Option::None
        }

        fn part_mut(
            &mut self,
            _: ::core::primitive::u32,
        ) -> ::std::option::Option<&mut dyn #r::Reflect> {
            ::std::option::Option::None
        }
    })
}

/// A span at `ty`, for an error about it to be reported there, but resolved where the derive is,
/// as everything it writes is: so that `self` is the method's own, even for a type that a macro
/// wrote.
fn at_type(ty: &syn::Type) -> Span {
    Span::call_site().located_at(ty.span())
}

/// What `describe`'s closure calls the types it's given: `types` if a field describes its own
/// type with them, and `_` otherwise, so that the closure has no unused variable.
fn types_binding(fields: &[Field]) -> TokenStream {
    if fields.iter().any(|field| field.hints.opaque.is_none()) {
        quote!(types)
    } else {
        quote!(_)
    }
}

/// A field of a struct or a variant, as the derives write about it.
struct Field<'a> {
    /// How its struct or variant names it: by its name, or its index.
    member: Member,
    /// Its name in descriptions and values: its own, or its index, such as `0`.
    name: String,
    ty: &'a syn::Type,
    /// What the code written binds its value to.
    binding: Ident,
    hints: FieldHints,
    doc: String,
}

impl<'a> Field<'a> {
    /// `fields`, each with its hints, whose checks of its type go in `checks`.
    fn all(fields: &'a Fields, checks: &mut Vec<TokenStream>) -> syn::Result<Vec<Self>> {
        let mut all = Vec::new();
        for (i, field) in fields.iter().enumerate() {
            let hints = FieldHints::parse(&field.attrs)?;
            hints.check(&field.ty, checks);
            let (member, name) = match &field.ident {
                Some(ident) => (Member::Named(ident.clone()), ident.unraw().to_string()),
                None => {
                    let index = syn::Index {
                        index: i as u32,
                        span: at_type(&field.ty),
                    };
                    (Member::Unnamed(index), i.to_string())
                }
            };
            all.push(Self {
                member,
                name,
                ty: &field.ty,
                binding: format_ident!("__field{}", i),
                hints,
                doc: doc(&field.attrs),
            });
        }
        Ok(all)
    }

    /// Its description, as a `Field`.
    fn describe(&self) -> TokenStream {
        let r = reflect();
        let Self { name, ty, doc, .. } = self;
        let ty = match self.hints.opaque {
            Some(_) => quote!(#r::Type::Opaque),
            None => quote!(<#ty as #r::Reflect>::describe(types)?),
        };
        let bound = |bound: &Option<Bound>| match bound {
            Some(bound) => {
                let tokens = &bound.tokens;
                quote!(::std::option::Option::Some(#tokens))
            }
            None => quote!(::std::option::Option::None),
        };
        let (min, max) = (bound(&self.hints.min), bound(&self.hints.max));
        let show = if self.hints.degrees.is_some() {
            quote!(#r::Show::Degrees)
        } else if self.hints.color.is_some() {
            quote!(#r::Show::Color)
        } else {
            quote!(#r::Show::Plain)
        };
        quote! {
            #r::Field {
                name: ::std::borrow::Cow::Borrowed(#name),
                ty: #ty,
                meta: #r::Meta {
                    min: #min,
                    max: #max,
                    show: #show,
                    doc: ::std::borrow::Cow::Borrowed(#doc),
                },
            }
        }
    }

    /// Its name and its value, read from `place`, a reference to it.
    fn to_value(&self, place: &impl ToTokens) -> TokenStream {
        let r = reflect();
        let Self { name, ty, .. } = self;
        let value = match self.hints.opaque {
            Some(_) => quote!(#r::Value::Opaque),
            None => quote!(<#ty as #r::Reflect>::to_value(#place)),
        };
        quote!((::std::borrow::Cow::Borrowed(#name), #value))
    }

    /// It, made from its value, which `binding` holds.
    fn make(&self) -> TokenStream {
        let r = reflect();
        let Self {
            name, ty, binding, ..
        } = self;
        let made = match self.hints.opaque {
            Some(_) => quote!(#r::make_opaque(#binding)),
            None => quote!(<#ty as #r::Reflect>::from_value(#binding)),
        };
        quote!(#made.map_err(|error| #r::within(error, #name))?)
    }

    /// It, as `place`, a reference to it, is with its value, which `binding` holds.
    fn keep(&self, place: &TokenStream) -> TokenStream {
        let r = reflect();
        let Self {
            name, ty, binding, ..
        } = self;
        let kept = match self.hints.opaque {
            // At its type, so that a type that isn't `Clone` is reported there.
            Some(_) => {
                let place = respanned(place.clone(), ty.span());
                quote_spanned!(at_type(ty)=> ::gg_engine::reflect::keep(#place, #binding))
            }
            None => quote!(<#ty as #r::Reflect>::with_value(#place, #binding)),
        };
        quote!(#kept.map_err(|error| #r::within(error, #name))?)
    }
}

/// A type's `#[reflect(...)]` hints, with where `opaque` is.
#[derive(Default)]
struct TypeHints {
    name: Option<LitStr>,
    opaque: Option<Span>,
}

impl TypeHints {
    fn parse(attrs: &[Attribute]) -> syn::Result<Self> {
        let mut hints = Self::default();
        for attr in attrs.iter().filter(|attr| attr.path().is_ident("reflect")) {
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("name") {
                    let name = meta.value()?.parse()?;
                    once(&mut hints.name, name, &meta)
                } else if meta.path.is_ident("opaque") {
                    once(&mut hints.opaque, meta.path.span(), &meta)
                } else {
                    Err(meta.error(
                        "unknown hint: a type takes `name = \"...\"`, which names it in place of \
                         its identifier, and `opaque`, which reflection keeps but doesn't read",
                    ))
                }
            })?;
        }
        Ok(hints)
    }
}

/// Sets `hint`, the one `meta` gives, to `value`, refusing it if it's given twice.
fn once<T>(hint: &mut Option<T>, value: T, meta: &ParseNestedMeta) -> syn::Result<()> {
    match hint.replace(value) {
        Some(_) => Err(meta.error(format!(
            "`{}` is given twice, but a hint is given once",
            meta.path.to_token_stream()
        ))),
        None => Ok(()),
    }
}

/// A field's `#[reflect(...)]` hints, with where each is.
#[derive(Default)]
struct FieldHints {
    min: Option<Bound>,
    max: Option<Bound>,
    degrees: Option<Span>,
    color: Option<Span>,
    opaque: Option<Span>,
}

impl FieldHints {
    fn parse(attrs: &[Attribute]) -> syn::Result<Self> {
        let mut hints = Self::default();
        for attr in attrs.iter().filter(|attr| attr.path().is_ident("reflect")) {
            attr.parse_nested_meta(|meta| {
                let span = meta.path.span();
                if meta.path.is_ident("min") {
                    let bound = Bound::parse(&meta)?;
                    once(&mut hints.min, bound, &meta)
                } else if meta.path.is_ident("max") {
                    let bound = Bound::parse(&meta)?;
                    once(&mut hints.max, bound, &meta)
                } else if meta.path.is_ident("degrees") {
                    once(&mut hints.degrees, span, &meta)
                } else if meta.path.is_ident("color") {
                    once(&mut hints.color, span, &meta)
                } else if meta.path.is_ident("opaque") {
                    once(&mut hints.opaque, span, &meta)
                } else {
                    Err(meta.error(
                        "unknown hint: a field takes `min = ..` and `max = ..`, which bound it, \
                         `degrees` for an angle in radians, `color` for a linear color, and \
                         `opaque`, which reflection keeps but doesn't read",
                    ))
                }
            })?;
        }
        if let (Some(_), Some(color)) = (hints.degrees, hints.color) {
            return Err(syn::Error::new(
                color,
                "a field is shown in degrees or as a color, not both",
            ));
        }
        let shown = hints.min.is_some()
            || hints.max.is_some()
            || hints.degrees.is_some()
            || hints.color.is_some();
        if let Some(opaque) = hints.opaque
            && shown
        {
            return Err(syn::Error::new(
                opaque,
                "an opaque field isn't read, so it takes no other hint",
            ));
        }
        if let (Some(min), Some(max)) = (&hints.min, &hints.max)
            && min.value.above(max.value)
        {
            return Err(syn::Error::new(max.span, "`max` is below `min`"));
        }
        Ok(hints)
    }

    /// Adds to `checks` a check that each hint suits `ty`, at the hint, so that `degrees` on a
    /// `Vec3` fails to compile where it's written.
    fn check(&self, ty: &syn::Type, checks: &mut Vec<TokenStream>) {
        let bounded = self.min.as_ref().or(self.max.as_ref());
        let hints = [
            (bounded.map(|bound| bound.span), "Ranged"),
            (self.degrees, "Angle"),
            (self.color, "Color"),
        ];
        for (span, marker) in hints {
            let Some(span) = span else { continue };
            let ty = respanned(ty.to_token_stream(), span);
            let span = Span::call_site().located_at(span);
            let marker = Ident::new(marker, span);
            checks.push(quote_spanned! {span=>
                const _: () = {
                    fn hint<T: ?::std::marker::Sized + ::gg_engine::reflect::#marker>() {}
                    let _ = hint::<#ty>;
                };
            });
        }
    }
}

/// `min = ..` or `max = ..`: a number, which may start with `-`.
struct Bound {
    /// The `Bound` it is.
    tokens: TokenStream,
    value: Number,
    span: Span,
}

#[derive(Clone, Copy)]
enum Number {
    Int(i128),
    Float(f64),
}

impl Number {
    fn above(self, other: Self) -> bool {
        match (self, other) {
            (Self::Int(a), Self::Int(b)) => a > b,
            (a, b) => a.float() > b.float(),
        }
    }

    fn float(self) -> f64 {
        match self {
            Self::Int(n) => n as f64,
            Self::Float(x) => x,
        }
    }
}

impl Bound {
    fn parse(meta: &ParseNestedMeta) -> syn::Result<Self> {
        let r = reflect();
        let input = meta.value()?;
        let minus: Option<Token![-]> = input.parse()?;
        let lit: Lit = input.parse()?;
        let span = lit.span();
        let (tokens, value) = match &lit {
            Lit::Int(int) => {
                let magnitude: i128 = int.base10_parse()?;
                let number = Literal::i128_unsuffixed(magnitude);
                let value = if minus.is_some() {
                    -magnitude
                } else {
                    magnitude
                };
                (quote!(#r::Bound::Int(#minus #number)), Number::Int(value))
            }
            Lit::Float(float) => {
                let magnitude: f64 = float.base10_parse()?;
                if !magnitude.is_finite() {
                    return Err(syn::Error::new(span, "a bound is a finite number"));
                }
                let number = Literal::f64_unsuffixed(magnitude);
                let value = if minus.is_some() {
                    -magnitude
                } else {
                    magnitude
                };
                (
                    quote!(#r::Bound::Float(#minus #number)),
                    Number::Float(value),
                )
            }
            _ => {
                return Err(syn::Error::new(
                    span,
                    "a bound is a number, such as `0`, `1.5` or `-1`",
                ));
            }
        };
        Ok(Self {
            tokens,
            value,
            span,
        })
    }
}

/// `tokens`, each located at `span`, so that an error in them is reported there, but resolved as
/// they were.
fn respanned(tokens: TokenStream, span: Span) -> TokenStream {
    tokens
        .into_iter()
        .map(|mut token| {
            if let TokenTree::Group(group) = &token {
                let mut inner =
                    proc_macro2::Group::new(group.delimiter(), respanned(group.stream(), span));
                inner.set_span(group.span().located_at(span));
                token = TokenTree::Group(inner);
            } else {
                token.set_span(token.span().located_at(span));
            }
            token
        })
        .collect()
}

/// The text of `attrs`' doc comments, as paragraphs: each one's lines joined by spaces, and a
/// blank line between them.
fn doc(attrs: &[Attribute]) -> String {
    let mut text = String::new();
    let mut paragraph_ended = false;
    for attr in attrs.iter().filter(|attr| attr.path().is_ident("doc")) {
        let Meta::NameValue(pair) = &attr.meta else {
            continue;
        };
        let Expr::Lit(syn::ExprLit {
            lit: Lit::Str(lit), ..
        }) = &pair.value
        else {
            continue;
        };
        // Split, not cut into lines, so that an empty attribute, which a bare `///` is, ends a
        // paragraph.
        let value = lit.value();
        let lines: Vec<&str> = value.split('\n').map(str::trim).collect();
        // A block comment's lines after its first may each start with a `*`, which rustdoc
        // leaves out.
        let starred = lines.len() > 1
            && lines[1..]
                .iter()
                .all(|line| line.is_empty() || line.starts_with('*'));
        for line in lines {
            let line = match line.strip_prefix('*') {
                Some(rest) if starred => rest.trim_start(),
                _ => line,
            };
            if line.is_empty() {
                paragraph_ended = !text.is_empty();
                continue;
            }
            if !text.is_empty() {
                text.push_str(if paragraph_ended { "\n\n" } else { " " });
            }
            text.push_str(line);
            paragraph_ended = false;
        }
    }
    text
}
