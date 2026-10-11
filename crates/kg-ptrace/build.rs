//! Generates one forwarder per `extern "C"` function of the ABI, from the
//! bindgen AST rather than from a list anyone maintains (`abi.md` §14.3).
//! An argument type the table below does not know is a build error, never a
//! silently opaque rendering.

use std::fmt::Write as _;

use quote::ToTokens;

/// How one C type is rendered on the trace line.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Node,
    CStr,
    Scope,
    UInt,
    Int,
    F32,
    LocPtr,
    Loc,
    Options,
}

fn classify(ty: &syn::Type, enums: &std::collections::HashMap<String, Kind>, fn_name: &str) -> Kind {
    let text = ty.to_token_stream().to_string();
    let text = text.replace(' ', "");
    match text.as_str() {
        "ParseTreeNodePtr" => Kind::Node,
        "*const::std::os::raw::c_char" => Kind::CStr,
        "ParseNamespaceScopePtr" | "ParseQualifiedNamePtr" => Kind::Scope,
        "::std::os::raw::c_uint" | "BOOL" => Kind::UInt,
        "::std::os::raw::c_int" => Kind::Int,
        "f32" => Kind::F32,
        "*constLocation" => Kind::LocPtr,
        "Location" => Kind::Loc,
        "*constOptions" => Kind::Options,
        // `typedef enum {…} Name;` is `pub mod Name { pub type Type … }` plus
        // `pub use Name::Type as Name`, so a parameter reads either way.
        _ => match enums.get(text.strip_suffix("::Type").unwrap_or(&text)) {
            Some(k) => *k,
            None => panic!("unknown ABI type `{text}` on {fn_name}; extend the table in kg-ptrace/build.rs"),
        },
    }
}

fn main() {
    let bindings = kg_abi_gen::builder().generate().expect("bindgen").to_string();
    let file: syn::File = syn::parse_str(&bindings).expect("bindgen output parses");

    // `pub mod ParseTreeBinaryOpType { pub type Type = c_uint; … }`
    let mut enums = std::collections::HashMap::new();
    for item in &file.items {
        if let syn::Item::Mod(m) = item {
            for inner in m.content.iter().flat_map(|(_, items)| items) {
                if let syn::Item::Type(t) = inner
                    && t.ident == "Type"
                {
                    let base = t.ty.to_token_stream().to_string().replace(' ', "");
                    let kind = match base.as_str() {
                        "::std::os::raw::c_uint" => Kind::UInt,
                        "::std::os::raw::c_int" => Kind::Int,
                        other => panic!("enum {} over {other}", m.ident),
                    };
                    // `typedef enum {…} Name;` is `pub mod _Name` plus
                    // `pub use self::_Name::Type as Name`: both spellings.
                    let ident = m.ident.to_string();
                    enums.insert(ident.trim_start_matches('_').to_owned(), kind);
                    enums.insert(ident, kind);
                }
            }
        }
    }

    let mut out = String::new();
    let mut signatures = Vec::new();
    let mut names = Vec::new();
    for item in &file.items {
        let syn::Item::ForeignMod(fm) = item else { continue };
        if fm.abi.name.as_ref().map(|n| n.value()) != Some("C".to_owned()) {
            continue;
        }
        for fi in &fm.items {
            let syn::ForeignItem::Fn(f) = fi else { continue };
            let name = f.sig.ident.to_string();
            signatures.push(f.sig.to_token_stream().to_string());
            if kg_abi_gen::UNDEFINED_SYMBOLS.contains(&name.as_str()) {
                continue;
            }
            names.push(name.clone());
            let mut params = Vec::new();
            for (i, input) in f.sig.inputs.iter().enumerate() {
                let syn::FnArg::Typed(pt) = input else { panic!("{name}: self parameter") };
                let ty = pt.ty.to_token_stream().to_string();
                params.push((format!("a{i}"), ty, classify(&pt.ty, &enums, &name)));
            }
            let ret = match &f.sig.output {
                syn::ReturnType::Default => None,
                syn::ReturnType::Type(_, ty) => {
                    Some((ty.to_token_stream().to_string(), classify(ty, &enums, &name)))
                }
            };
            let arg_list = params.iter().map(|(n, t, _)| format!("{n}: {t}")).collect::<Vec<_>>().join(", ");
            let ty_list = params.iter().map(|(_, t, _)| t.clone()).collect::<Vec<_>>().join(", ");
            let call_list = params.iter().map(|(n, _, _)| n.clone()).collect::<Vec<_>>().join(", ");
            let ret_ty = ret.as_ref().map(|(t, _)| format!(" -> {t}")).unwrap_or_default();
            let renders = params
                .iter()
                .map(|(n, _, k)| match k {
                    Kind::Node => format!("crate::arg::node({n})"),
                    Kind::CStr => format!("unsafe {{ crate::arg::cstr({n}) }}"),
                    Kind::Scope => format!("unsafe {{ crate::arg::scope({n}) }}"),
                    Kind::UInt => format!("crate::arg::uint({n} as u64)"),
                    Kind::Int => format!("crate::arg::int({n} as i64)"),
                    Kind::F32 => format!("crate::arg::f32({n})"),
                    Kind::LocPtr => format!("unsafe {{ crate::arg::loc_ptr({n}) }}"),
                    Kind::Loc => format!("crate::arg::loc({n})"),
                    Kind::Options => format!("unsafe {{ crate::arg::options({n}) }}"),
                })
                .collect::<Vec<_>>()
                .join(", ");
            let ret_render = match ret.as_ref().map(|(_, k)| k) {
                None => "crate::arg::void()".to_owned(),
                Some(Kind::Node) => "crate::arg::node(r)".to_owned(),
                Some(Kind::UInt) => "crate::arg::uint(r as u64)".to_owned(),
                Some(Kind::Int) => "crate::arg::int(r as i64)".to_owned(),
                Some(k) => panic!("{name}: unexpected result kind {k:?}"),
            };
            // A void function has no `r` to bind or return.
            let (bind, give) = if ret.is_some() { ("let r = ", "\n    r") } else { ("", "") };
            let _ = write!(
                out,
                r#"
#[unsafe(no_mangle)]
pub unsafe extern "C" fn {name}({arg_list}){ret_ty} {{
    let call = crate::Call::enter("{name}");
    let real: unsafe extern "C" fn({ty_list}){ret_ty} = unsafe {{ crate::real("{name}") }};
    {bind}unsafe {{ real({call_list}) }};
    let args = [{renders}];
    crate::record(call, &args, {ret_render});{give}
}}
"#
            );
        }
    }
    // §14.1's abi-hash: over every signature, defined or not.
    let mut h: u64 = 0xcbf29ce484222325;
    for s in &signatures {
        for b in s.bytes() {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x100000001b3);
        }
    }
    let _ = write!(
        out,
        "\npub const ABI_HASH: &str = \"{h:016x}\";\npub const FORWARDERS: &[&str] = &[{}];\n",
        names.iter().map(|n| format!("\"{n}\"")).collect::<Vec<_>>().join(", ")
    );
    let dir = std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    std::fs::write(dir.join("forwarders.rs"), out).expect("write forwarders.rs");
}
