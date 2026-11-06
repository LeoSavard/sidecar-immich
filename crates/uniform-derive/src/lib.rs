// SPDX-License-Identifier: Apache-2.0
//! `#[derive(Uniform)]` for a `#[repr(C)]` struct the GPU reads as a uniform. See `sidecar-uniform`.
//!
//! Each field's WGSL type follows from its Rust type: `f32`/`u32`/`i32` as themselves, `[S; 2…4]` as a vector,
//! `[[S; 4]; N]` as `array<vec4…, N>`, another `Uniform` struct (or an array of one) by its name. A field whose
//! name starts with `_` is padding, and an array of scalars there becomes one scalar per element (`_pad0`,
//! `_pad1`, …), because a vector would bring its own alignment. `#[uniform(wgsl = "mat3x3f")]` overrides
//! the type where the Rust spelling is ambiguous: a `mat3x3f` is three padded columns, `[[f32; 4]; 3]`.

use proc_macro2::TokenStream;
use quote::quote;
use syn::{Data, DeriveInput, Expr, Fields, Lit, Type, parse_macro_input, spanned::Spanned};

#[proc_macro_derive(Uniform, attributes(uniform))]
pub fn derive(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match expand(&input) {
        Ok(t) => t.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

fn scalar(ty: &Type) -> Option<(&'static str, &'static str)> {
    let Type::Path(p) = ty else { return None };
    match p.path.get_ident()?.to_string().as_str() {
        "f32" => Some(("f32", "f")),
        "u32" => Some(("u32", "u")),
        "i32" => Some(("i32", "i")),
        _ => None,
    }
}

fn literal_len(e: &Expr) -> Option<usize> {
    match e {
        Expr::Lit(l) => match &l.lit {
            Lit::Int(i) => i.base10_parse().ok(),
            _ => None,
        },
        _ => None,
    }
}

fn docs(attrs: &[syn::Attribute]) -> Vec<String> {
    attrs
        .iter()
        .filter(|a| a.path().is_ident("doc"))
        .filter_map(|a| match &a.meta {
            syn::Meta::NameValue(nv) => match &nv.value {
                Expr::Lit(syn::ExprLit {
                    lit: Lit::Str(s), ..
                }) => Some(
                    s.value()
                        .strip_prefix(' ')
                        .unwrap_or(&s.value())
                        .to_string(),
                ),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

fn wgsl_override(attrs: &[syn::Attribute]) -> syn::Result<Option<String>> {
    let mut out = None;
    for a in attrs.iter().filter(|a| a.path().is_ident("uniform")) {
        a.parse_nested_meta(|m| {
            if m.path.is_ident("wgsl") {
                out = Some(m.value()?.parse::<syn::LitStr>()?.value());
                Ok(())
            } else {
                Err(m.error("expected `wgsl = \"…\"`"))
            }
        })?;
    }
    Ok(out)
}

/// The WGSL type of `ty` as an expression producing a `String`, plus the nested uniform types it names.
fn wgsl_type(ty: &Type) -> syn::Result<(TokenStream, Vec<Type>)> {
    if let Some((s, _)) = scalar(ty) {
        return Ok((quote!(String::from(#s)), vec![]));
    }
    match ty {
        Type::Path(_) => Ok((
            quote!(String::from(<#ty as ::sidecar_uniform::Uniform>::NAME)),
            vec![ty.clone()],
        )),
        Type::Array(a) => {
            let len = &a.len;
            if let Some((_, suffix)) = scalar(&a.elem) {
                return match literal_len(len) {
                    Some(n @ 2..=4) => {
                        let v = format!("vec{n}{suffix}");
                        Ok((quote!(String::from(#v)), vec![]))
                    }
                    _ => Err(syn::Error::new(
                        a.span(),
                        "an array of scalars in a uniform must be 2…4 long (a vector); a longer one would \
                         need a 16-byte stride",
                    )),
                };
            }
            let (inner, nested) = wgsl_type(&a.elem)?;
            Ok((
                quote!(format!("array<{}, {}>", #inner, (#len) as usize)),
                nested,
            ))
        }
        _ => Err(syn::Error::new(ty.span(), "not a type a uniform can hold")),
    }
}

fn expand(input: &DeriveInput) -> syn::Result<TokenStream> {
    let name = &input.ident;
    let Data::Struct(data) = &input.data else {
        return Err(syn::Error::new(input.span(), "Uniform is for structs"));
    };
    let Fields::Named(fields) = &data.fields else {
        return Err(syn::Error::new(input.span(), "Uniform needs named fields"));
    };
    let mut lines = Vec::new();
    let mut members = Vec::new();
    let mut nested: Vec<Type> = Vec::new();
    for f in &fields.named {
        let ident = f.ident.as_ref().unwrap();
        let field = ident.to_string();
        let doc = docs(&f.attrs);
        let comment = quote! {
            let doc: &[&str] = &[#(#doc),*];
            for line in doc {
                if line.is_empty() { out.push_str("  //\n") } else { out.push_str(&format!("  // {line}\n")) }
            }
        };
        // Padding that is an array of scalars: one member per element.
        if let (true, Type::Array(a)) = (field.starts_with('_'), &f.ty)
            && let Some((s, _)) = scalar(&a.elem)
        {
            let elem = &a.elem;
            let len = &a.len;
            lines.push(quote! {
                #comment
                for i in 0..(#len) as usize { out.push_str(&format!("  {}{i}: {},\n", #field, #s)); }
            });
            members.push(quote! {
                for i in 0..(#len) as usize {
                    v.push(::sidecar_uniform::Field {
                        name: format!("{}{i}", #field),
                        offset: ::core::mem::offset_of!(#name, #ident) + i * ::core::mem::size_of::<#elem>(),
                    });
                }
            });
            continue;
        }
        let ty = match wgsl_override(&f.attrs)? {
            Some(w) => quote!(String::from(#w)),
            None => {
                let (t, n) = wgsl_type(&f.ty)?;
                nested.extend(n);
                t
            }
        };
        lines.push(quote! {
            #comment
            out.push_str(&format!("  {}: {},\n", #field, #ty));
        });
        members.push(quote! {
            v.push(::sidecar_uniform::Field {
                name: String::from(#field),
                offset: ::core::mem::offset_of!(#name, #ident),
            });
        });
    }
    let name_str = name.to_string();
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();
    Ok(quote! {
        impl #impl_generics ::sidecar_uniform::Uniform for #name #ty_generics #where_clause {
            const NAME: &'static str = #name_str;
            const SOURCE: &'static str = concat!(module_path!(), "::", #name_str);
            const SIZE: usize = ::core::mem::size_of::<Self>();
            fn fields() -> Vec<::sidecar_uniform::Field> {
                let mut v = Vec::new();
                #(#members)*
                v
            }
            fn declaration() -> String {
                let mut out = format!("struct {} {{\n", #name_str);
                #(#lines)*
                out.push_str("}\n");
                out
            }
            fn requires() -> Vec<::sidecar_uniform::Requirement> {
                vec![#(::sidecar_uniform::Requirement::of::<#nested>()),*]
            }
        }
    })
}
